import Foundation

/// The local RustCode engine running as `rustcode --acp`.
///
/// Lifecycle contract:
/// - stdout carries ACP only; it is framed and fed, in order, to `connection`.
/// - stderr is kept as a bounded tail for diagnostics, never parsed.
/// - `stop()` closes stdin first, which the engine treats as the end of the
///   ACP connection: it cancels active turns and stops the sessions'
///   background processes. Only if it does not exit in time is it sent
///   SIGTERM, then SIGKILL.
/// - Any exit closes `connection` (failing outstanding requests) after the
///   stdout already read has been delivered, then calls `onExit` once.
public final class EngineProcess: @unchecked Sendable {
    public struct Exit: Equatable, Sendable {
        public var status: Int32
        public var uncaughtSignal: Bool
        /// True when the exit followed `stop()`.
        public var requested: Bool
    }

    public let executable: URL
    public let connection: ACPConnection

    private let process = Process()
    private let stdinPipe = Pipe()
    private let stdoutPipe = Pipe()
    private let stderrPipe = Pipe()
    private let lock = NSLock()
    private var framer = LineFramer()
    private var stderrTail = Data()
    private var stopRequested = false
    private let exitSignal = OneShot<Exit>()
    private let exitHandler: @Sendable (Exit) -> Void
    private let lines: AsyncStream<Data>
    private let lineSink: AsyncStream<Data>.Continuation

    static let stderrTailBytes = 64 * 1024

    /// - Parameters:
    ///   - executable: the `rustcode` binary.
    ///   - workingDirectory: the engine's process cwd. Sessions carry their own
    ///     `cwd`; this only affects where the engine itself starts.
    ///   - onExit: called once, off the main thread, when the process ends.
    public init(
        executable: URL,
        workingDirectory: URL,
        environment: [String: String] = ProcessInfo.processInfo.environment,
        onExit: @escaping @Sendable (Exit) -> Void
    ) {
        self.executable = executable
        self.exitHandler = onExit
        let stdinHandle = stdinPipe.fileHandleForWriting
        self.connection = ACPConnection(writer: { data in
            try stdinHandle.write(contentsOf: data)
        })
        var sink: AsyncStream<Data>.Continuation!
        self.lines = AsyncStream(bufferingPolicy: .unbounded) { sink = $0 }
        self.lineSink = sink
        process.executableURL = executable
        // Never `--yolo`: every tool permission the engine's policy requires
        // is asked for through `session/request_permission`.
        process.arguments = ["--acp"]
        process.currentDirectoryURL = workingDirectory
        process.environment = environment
        process.standardInput = stdinPipe
        process.standardOutput = stdoutPipe
        process.standardError = stderrPipe
    }

    public func start() throws {
        let connection = self.connection
        let lineSink = self.lineSink
        stdoutPipe.fileHandleForReading.readabilityHandler = { [weak self] handle in
            let chunk = handle.availableData
            guard let self, !chunk.isEmpty else {
                handle.readabilityHandler = nil
                lineSink.finish()
                return
            }
            do {
                for line in try self.withLock({ try self.framer.append(chunk) }) {
                    lineSink.yield(line)
                }
            } catch {
                handle.readabilityHandler = nil
                lineSink.finish()
                Task { await connection.close(reason: "engine sent an oversized message") }
            }
        }
        stderrPipe.fileHandleForReading.readabilityHandler = { [weak self] handle in
            let chunk = handle.availableData
            guard let self, !chunk.isEmpty else {
                handle.readabilityHandler = nil
                return
            }
            self.withLock {
                self.stderrTail.append(chunk)
                if self.stderrTail.count > Self.stderrTailBytes {
                    self.stderrTail.removeFirst(self.stderrTail.count - Self.stderrTailBytes)
                }
            }
        }
        let exitSignal = self.exitSignal
        process.terminationHandler = { [weak self] process in
            let requested = self?.withLock { self?.stopRequested ?? false } ?? false
            let exit = Exit(
                status: process.terminationStatus,
                uncaughtSignal: process.terminationReason == .uncaughtSignal,
                requested: requested)
            Task {
                await exitSignal.set(exit)
                // A grandchild that inherited stdout could hold it open after
                // the engine is gone; stop waiting for EOF after a moment.
                try? await Task.sleep(for: .seconds(1))
                lineSink.finish()
            }
        }
        do {
            try process.run()
        } catch {
            stdoutPipe.fileHandleForReading.readabilityHandler = nil
            stderrPipe.fileHandleForReading.readabilityHandler = nil
            lineSink.finish()
            throw error
        }
        // Lines read before this point are buffered by the stream, so
        // nothing is lost and delivery stays in arrival order.
        let lines = self.lines
        let exitHandler = self.exitHandler
        Task.detached {
            for await line in lines {
                await connection.receive(line: line)
            }
            let exit = await exitSignal.wait()
            let reason = exit.requested
                ? "engine stopped"
                : "engine exited (\(exit.uncaughtSignal ? "signal" : "status") \(exit.status))"
            await connection.close(reason: reason)
            exitHandler(exit)
        }
    }

    public var isRunning: Bool { process.isRunning }

    /// Recent engine stderr, for error banners and the log sheet.
    public var diagnostics: String {
        withLock { String(decoding: stderrTail, as: UTF8.self) }
    }

    /// Graceful shutdown: EOF on stdin, then SIGTERM, then SIGKILL. Blocks the
    /// caller for at most `grace + 2s`; safe to call more than once.
    public func stop(grace: TimeInterval = 3) {
        withLock { stopRequested = true }
        try? stdinPipe.fileHandleForWriting.close()
        guard process.isRunning else { return }
        if waitForExit(timeout: grace) { return }
        process.terminate()
        if waitForExit(timeout: 1.5) { return }
        kill(process.processIdentifier, SIGKILL)
        _ = waitForExit(timeout: 0.5)
    }

    private func waitForExit(timeout: TimeInterval) -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        while process.isRunning && Date() < deadline {
            Thread.sleep(forTimeInterval: 0.05)
        }
        return !process.isRunning
    }

    private func withLock<T>(_ body: () throws -> T) rethrows -> T {
        lock.lock()
        defer { lock.unlock() }
        return try body()
    }
}

/// A value set once and awaited any number of times.
actor OneShot<Value: Sendable> {
    private var value: Value?
    private var waiters: [CheckedContinuation<Value, Never>] = []

    func set(_ newValue: Value) {
        guard value == nil else { return }
        value = newValue
        for waiter in waiters { waiter.resume(returning: newValue) }
        waiters.removeAll()
    }

    func wait() async -> Value {
        if let value { return value }
        return await withCheckedContinuation { waiters.append($0) }
    }
}
