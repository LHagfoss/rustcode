import Darwin
import Foundation
import SwiftTerm

/// Limits that keep a terminal's memory bounded however much it prints.
public enum TerminalLimits {
    /// Lines kept above the screen. Older lines are dropped.
    public static let scrollbackLines = 10_000
    /// Output kept while no terminal view is attached to a session (it is
    /// replayed when one is); older bytes are dropped.
    public static let detachedOutputBytes = 1 << 20
    /// Terminals one project can have open at once.
    public static let sessionsPerProject = 8
    /// How long a closed or quitting shell has to exit after SIGHUP before
    /// it and its processes are killed.
    public static let terminationGrace: TimeInterval = 1.5

    /// Options for the emulator that shows a session.
    public static func emulatorOptions(columns: Int = 80, rows: Int = 24) -> TerminalOptions {
        TerminalOptions(cols: columns, rows: rows, scrollback: scrollbackLines)
    }
}

/// One shell on a PTY, started in a project's folder.
///
/// The session belongs to the project it was opened in for its whole life:
/// switching projects never moves it or sends it input. Its output goes to
/// `output` (the terminal view); input comes only from that view. Nothing
/// here talks to the engine.
@MainActor
public final class TerminalSession: ObservableObject, Identifiable {
    public enum Status: Equatable, Sendable {
        case notStarted
        case running(pid: Int32)
        /// The shell ended by itself. `code` is its exit status, or 128 plus
        /// the signal that ended it.
        case exited(code: Int32)
        /// The user closed the session, or the app quit.
        case terminated
        /// The shell could not be started.
        case failed(String)
    }

    public nonisolated let id = UUID()
    /// The project folder the shell starts in, standardized.
    public let project: String
    /// Its number among the project's terminals, for its default title.
    public let ordinal: Int
    public let launch: TerminalLaunch

    @Published public private(set) var status: Status = .notStarted
    /// The title the shell or program set (OSC 0/2), if any.
    @Published public var title: String?
    /// The directory the shell reported (OSC 7), if it does.
    @Published public var reportedDirectory: String?
    /// How many times a shell was started here.
    @Published public private(set) var launchCount = 0

    public private(set) var columns = 80
    public private(set) var rows = 24

    /// Where output goes. While nil, output is kept (bounded) and handed to
    /// the next sink set.
    public var output: ((ArraySlice<UInt8>) -> Void)? {
        didSet { flushDetachedOutput() }
    }
    private var detachedOutput: [UInt8] = []
    private let detachedOutputLimit: Int

    private var process: LocalProcess?
    private var link: ProcessLink?

    public init(project: String, ordinal: Int, launch: TerminalLaunch,
                detachedOutputLimit: Int = TerminalLimits.detachedOutputBytes) {
        self.project = project
        self.ordinal = ordinal
        self.launch = launch
        self.detachedOutputLimit = detachedOutputLimit
    }

    public var isRunning: Bool {
        if case .running = status { return true }
        return false
    }

    public var pid: pid_t? {
        if case .running(let pid) = status { return pid }
        return nil
    }

    /// Something besides the shell runs in it, such as a dev server.
    public var hasRunningChildren: Bool {
        guard let pid else { return false }
        return !ProcessTree.children(of: pid).isEmpty
    }

    /// The shell has ended, so it can be started again.
    public var canRelaunch: Bool {
        switch status {
        case .exited, .failed, .notStarted: return true
        case .running, .terminated: return false
        }
    }

    public var displayTitle: String {
        if let title, !title.trimmingCharacters(in: .whitespaces).isEmpty { return title }
        return ordinal == 1 ? "Terminal" : "Terminal \(ordinal)"
    }

    /// Output held while detached, for tests and diagnostics.
    public var detachedOutputCount: Int { detachedOutput.count }

    // MARK: - Lifecycle

    /// Start the shell in the project folder. A folder that no longer exists
    /// fails rather than starting the shell somewhere else.
    public func start() {
        guard !isRunning, status != .terminated else { return }
        var isDirectory: ObjCBool = false
        guard FileManager.default.fileExists(atPath: project, isDirectory: &isDirectory), isDirectory.boolValue else {
            status = .failed("The project folder \(project) does not exist.")
            return
        }
        let link = ProcessLink(session: self)
        let process = LocalProcess(delegate: link)
        self.link = link
        self.process = process
        process.startProcess(executable: launch.executable, args: launch.arguments,
                             environment: launch.environmentList(currentDirectory: project),
                             execName: launch.execName, currentDirectory: project)
        guard process.running, process.shellPid > 0 else {
            status = .failed("Could not start \(launch.executable).")
            return
        }
        launchCount += 1
        status = .running(pid: process.shellPid)
    }

    /// Start a new shell after the last one exited, in the same folder.
    public func relaunch() {
        guard canRelaunch else { return }
        title = nil
        reportedDirectory = nil
        start()
    }

    /// End the shell and its processes: SIGHUP, then SIGKILL after `grace`.
    /// With `wait`, returns once they are gone; otherwise that happens in
    /// the background. The session cannot be restarted afterwards.
    public func terminate(grace: TimeInterval = TerminalLimits.terminationGrace, wait: Bool = false) {
        guard let termination = beginTermination() else { return }
        let deadline = Date().addingTimeInterval(grace)
        if wait {
            termination.finish(by: deadline)
        } else {
            DispatchQueue.global(qos: .utility).async { termination.finish(by: deadline) }
        }
    }

    /// Signals the shell and closes the PTY; the caller waits for the exit.
    func beginTermination() -> ShellTermination? {
        guard case .running(let pid) = status, let process else {
            if status != .terminated { status = .terminated }
            return nil
        }
        let termination = ShellTermination(shell: pid, ptyDescriptor: process.childfd)
        termination.hangUp()
        // Closes the PTY and stops SwiftTerm watching the shell; it is
        // reaped by `ShellTermination.finish`.
        process.terminate()
        status = .terminated
        return termination
    }

    // MARK: - I/O

    public func send(_ bytes: ArraySlice<UInt8>) {
        guard isRunning, let process else { return }
        process.send(data: bytes)
    }

    public func send(text: String) {
        send(Array(text.utf8)[...])
    }

    /// The view's size in character cells; the shell gets SIGWINCH.
    public func resize(columns: Int, rows: Int) {
        guard columns > 0, rows > 0 else { return }
        self.columns = min(columns, Int(UInt16.max))
        self.rows = min(rows, Int(UInt16.max))
        guard isRunning, let process, process.childfd >= 0 else { return }
        var size = windowSize
        _ = PseudoTerminalHelpers.setWinSize(masterPtyDescriptor: process.childfd, windowSize: &size)
    }

    var windowSize: winsize {
        winsize(ws_row: UInt16(rows), ws_col: UInt16(columns), ws_xpixel: 0, ws_ypixel: 0)
    }

    fileprivate func receive(_ bytes: ArraySlice<UInt8>, from source: ProcessLink) {
        guard source === link else { return }
        if let output {
            output(bytes)
            return
        }
        detachedOutput.append(contentsOf: bytes)
        if detachedOutput.count > detachedOutputLimit {
            detachedOutput.removeFirst(detachedOutput.count - detachedOutputLimit)
        }
    }

    fileprivate func processExited(rawStatus: Int32?, from source: ProcessLink) {
        guard source === link, isRunning else { return }
        status = .exited(code: Self.exitCode(rawStatus: rawStatus ?? 0))
    }

    private func flushDetachedOutput() {
        guard let output, !detachedOutput.isEmpty else { return }
        let held = detachedOutput
        detachedOutput = []
        output(held[...])
    }

    /// A `waitpid` status as a shell reports it: the exit code, or 128 plus
    /// the signal.
    nonisolated static func exitCode(rawStatus: Int32) -> Int32 {
        let signal = rawStatus & 0x7f
        return signal == 0 ? (rawStatus >> 8) & 0xff : 128 + signal
    }
}

/// SwiftTerm's process delegate, one per launch so a relaunched session
/// ignores the previous shell. SwiftTerm calls it on the main queue.
private final class ProcessLink: LocalProcessDelegate {
    weak var session: TerminalSession?

    init(session: TerminalSession) {
        self.session = session
    }

    func processTerminated(_ source: LocalProcess, exitCode: Int32?) {
        MainActor.assumeIsolated { session?.processExited(rawStatus: exitCode, from: self) }
    }

    func dataReceived(slice: ArraySlice<UInt8>) {
        MainActor.assumeIsolated { session?.receive(slice, from: self) }
    }

    func getWindowSize() -> winsize {
        MainActor.assumeIsolated { session?.windowSize ?? winsize(ws_row: 24, ws_col: 80, ws_xpixel: 0, ws_ypixel: 0) }
    }
}
