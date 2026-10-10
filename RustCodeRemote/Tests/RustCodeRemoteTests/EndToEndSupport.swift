import Darwin
import Foundation
import Network
import Synchronization
@testable import RustCodeRemote

/// A stand-in for a model provider: an OpenAI-style chat-completions endpoint on loopback
/// that streams a fixed reply, slowly enough for a client to act while the turn runs.
final class FakeModelServer: Sendable {
    static let reply = ["Hello ", "from ", "the ", "fake ", "model. ", "This ", "turn ", "ran ", "in ", "a ", "real ", "terminal."]

    private let listener: NWListener
    private let chunkDelay: Duration
    private let askQuestions: Bool
    private let hasAsked = Mutex(false)

    init(chunkDelay: Duration = .milliseconds(120), askQuestions: Bool = false) throws {
        self.askQuestions = askQuestions
        self.chunkDelay = chunkDelay
        let parameters = NWParameters.tcp
        parameters.requiredLocalEndpoint = .hostPort(host: "127.0.0.1", port: .any)
        listener = try NWListener(using: parameters)
    }

    func start() async throws -> UInt16 {
        let resumed = Mutex(false)
        return try await withCheckedThrowingContinuation { continuation in
            listener.stateUpdateHandler = { [listener] state in
                let result: Result<UInt16, any Error>
                switch state {
                case .ready: result = .success(listener.port?.rawValue ?? 0)
                case .failed(let error): result = .failure(error)
                default: return
                }
                let isFirst = resumed.withLock { done in
                    defer { done = true }
                    return !done
                }
                if isFirst { continuation.resume(with: result) }
            }
            listener.newConnectionHandler = { [weak self, chunkDelay, askQuestions] connection in
                guard let self else { connection.cancel(); return }
                connection.start(queue: .global())
                Self.readRequest(on: connection, received: Data()) { body in
                    let request = (try? JSONSerialization.jsonObject(with: Data(body.utf8))) as? [String: Any]
                    let tools = request?["tools"] as? [[String: Any]] ?? []
                    let advertisesQuestions = tools.contains { ($0["function"] as? [String: Any])?["name"] as? String == "ask_question" }
                    let asksNow = self.hasAsked.withLock { asked in
                        guard askQuestions, advertisesQuestions, !asked else { return false }
                        asked = true
                        return true
                    }
                    Task { await Self.streamReply(on: connection, delay: chunkDelay, askQuestions: asksNow) }
                }
            }
            listener.start(queue: .global())
        }
    }

    func stop() {
        listener.cancel()
    }

    /// Reads until the headers and the announced body are in.
    private static func readRequest(on connection: NWConnection, received: Data, then respond: @escaping @Sendable (String) -> Void) {
        connection.receive(minimumIncompleteLength: 1, maximumLength: 1 << 20) { data, _, isComplete, error in
            var received = received
            received.append(data ?? Data())
            let text = String(decoding: received, as: UTF8.self)
            if let headerEnd = text.range(of: "\r\n\r\n") {
                let headers = text[..<headerEnd.lowerBound].lowercased()
                let length = headers.split(separator: "\r\n")
                    .first { $0.hasPrefix("content-length:") }
                    .flatMap { Int($0.dropFirst("content-length:".count).trimmingCharacters(in: .whitespaces)) } ?? 0
                if text[headerEnd.upperBound...].utf8.count >= length {
                    respond(String(text[headerEnd.upperBound...]))
                    return
                }
            }
            guard error == nil, !isComplete else {
                connection.cancel()
                return
            }
            readRequest(on: connection, received: received, then: respond)
        }
    }

    private static func streamReply(on connection: NWConnection, delay: Duration, askQuestions: Bool) async {
        func send(_ text: String) async {
            await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
                connection.send(content: Data(text.utf8), completion: .contentProcessed { _ in continuation.resume() })
            }
        }
        func chunk(_ delta: String, finish: String = "null") -> String {
            "data: {\"id\":\"fake\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"fake\","
                + "\"choices\":[{\"index\":0,\"delta\":\(delta),\"finish_reason\":\(finish)}]}\n\n"
        }
        await send("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n")
        if askQuestions {
            let arguments: [String: Any] = ["questions": (1...3).map { ["header": "Question \($0)", "question": "Pick option for question \($0)", "options": ["First", "Second"]] }]
            let argumentsString = String(decoding: try! JSONSerialization.data(withJSONObject: arguments), as: UTF8.self)
            let delta: [String: Any] = ["role": "assistant", "tool_calls": [["index": 0, "id": "call_questions", "type": "function", "function": ["name": "ask_question", "arguments": argumentsString]]]]
            await send(chunk(String(decoding: try! JSONSerialization.data(withJSONObject: delta), as: UTF8.self)))
            await send(chunk("{}", finish: "\"tool_calls\""))
            await send("data: [DONE]\n\n")
            connection.cancel()
            return
        }
        for (index, word) in reply.enumerated() {
            let role = index == 0 ? "\"role\":\"assistant\"," : ""
            await send(chunk("{\(role)\"content\":\"\(word)\"}"))
            try? await Task.sleep(for: delay)
        }
        await send(chunk("{}", finish: "\"stop\""))
        await send("data: [DONE]\n\n")
        connection.cancel()
    }
}

/// A real `rustcode` terminal session running in a pseudo-terminal, driven by typing.
final class TerminalSession: Sendable {
    private let process: Process
    private let master: Int32
    private let output = OutputBuffer()

    /// - Parameters:
    ///   - configuration: `RUSTCODE_CONFIG_DIR` shared with the gateway.
    ///   - workspace: the directory the session works in.
    init(binary: String, configuration: URL, workspace: URL) throws {
        var master: Int32 = 0
        var slave: Int32 = 0
        var size = winsize(ws_row: 40, ws_col: 120, ws_xpixel: 0, ws_ypixel: 0)
        guard openpty(&master, &slave, nil, nil, &size) == 0 else {
            throw RemoteError.transport("could not open a pseudo-terminal")
        }
        self.master = master

        process = Process()
        process.executableURL = URL(fileURLWithPath: binary)
        process.currentDirectoryURL = workspace
        process.environment = [
            "RUSTCODE_CONFIG_DIR": configuration.path,
            "TERM": "xterm-256color",
            "PATH": "/usr/bin:/bin",
            "HOME": configuration.path,
        ]
        let terminal = FileHandle(fileDescriptor: slave, closeOnDealloc: false)
        process.standardInput = terminal
        process.standardOutput = terminal
        process.standardError = terminal
        try process.run()
        close(slave)

        // Keep the terminal drained, and answer the cursor-position query a TUI asks at start.
        let thread = Thread { [master, output] in
            var buffer = [UInt8](repeating: 0, count: 65536)
            while true {
                let count = read(master, &buffer, buffer.count)
                guard count > 0 else { return }
                let data = Data(buffer[0..<count])
                output.append(data)
                if data.range(of: Data("\u{1b}[6n".utf8)) != nil {
                    let answer = Array("\u{1b}[1;1R".utf8)
                    _ = write(master, answer, answer.count)
                }
            }
        }
        thread.start()
    }

    /// Types text into the session. `\r` is Enter.
    func type(_ text: String) {
        let bytes = Array(text.utf8)
        _ = write(master, bytes, bytes.count)
    }

    /// What the terminal has printed so far, without escape sequences.
    var screenText: String {
        let raw = String(decoding: output.data, as: UTF8.self)
        return raw.replacing(/\u{1b}\[[0-9;?]*[A-Za-z]/, with: " ")
    }

    func stop() {
        process.terminate()
        close(master)
    }
}

private final class OutputBuffer: Sendable {
    private let storage = Mutex(Data())

    var data: Data { storage.withLock { $0 } }

    func append(_ data: Data) {
        storage.withLock { all in
            all.append(data)
            if all.count > 1 << 20 { all.removeFirst(all.count - (1 << 19)) }
        }
    }
}

/// Forwards to a real WebSocket and keeps every frame, so that a mismatch with the gateway
/// can be reported with the exact text. Set `RUSTCODE_E2E_FRAMES` to a path to write them out.
actor RecordingTransport: RemoteTransport {
    nonisolated let incoming: AsyncThrowingStream<String, any Error>
    private let continuation: AsyncThrowingStream<String, any Error>.Continuation
    private let inner = URLSessionWebSocketTransport()
    private let log: FrameLog
    private var pump: Task<Void, Never>?

    init(log: FrameLog) {
        self.log = log
        (incoming, continuation) = AsyncThrowingStream.makeStream(of: String.self)
    }

    func connect(to url: URL, headers: [String: String]) async throws {
        try await inner.connect(to: url, headers: headers)
        pump = Task { [inner, continuation, log] in
            do {
                for try await frame in inner.incoming {
                    log.record("<", frame)
                    continuation.yield(frame)
                }
                continuation.finish()
            } catch {
                continuation.finish(throwing: error)
            }
        }
    }

    func send(_ text: String) async throws {
        log.record(">", text)
        try await inner.send(text)
    }

    func close() async {
        await inner.close()
        pump?.cancel()
        continuation.finish()
    }
}

final class FrameLog: Sendable {
    private let lines = Mutex<[String]>([])

    func record(_ direction: String, _ frame: String) {
        // Secrets never leave the test process, not even into a scratch file.
        var frame = frame
        for key in ["token", "secret", "credential"] {
            frame = frame.replacing(try! Regex("\"\(key)\":\"[^\"]*\""), with: "\"\(key)\":\"<redacted>\"")
        }
        lines.withLock { $0.append("\(direction) \(frame)") }
    }

    var all: [String] { lines.withLock { $0 } }

    func flush() {
        guard let path = ProcessInfo.processInfo.environment["RUSTCODE_E2E_FRAMES"] else { return }
        try? all.joined(separator: "\n").write(toFile: path, atomically: true, encoding: .utf8)
    }
}
