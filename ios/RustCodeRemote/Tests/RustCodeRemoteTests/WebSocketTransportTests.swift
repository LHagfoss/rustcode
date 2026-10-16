import Foundation
import Network
import Synchronization
import Testing
@testable import RustCodeRemote

/// A loopback WebSocket server on Network.framework that answers `text` with `echo:text`.
private final class EchoServer: Sendable {
    private let listener: NWListener

    init() throws {
        let parameters = NWParameters.tcp
        let options = NWProtocolWebSocket.Options()
        options.autoReplyPing = true
        parameters.defaultProtocolStack.applicationProtocols.insert(options, at: 0)
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
            listener.newConnectionHandler = { connection in
                connection.start(queue: .global())
                Self.echo(on: connection)
            }
            listener.start(queue: .global())
        }
    }

    func stop() {
        listener.cancel()
    }

    private static func echo(on connection: NWConnection) {
        connection.receiveMessage { data, context, _, error in
            guard error == nil, let data,
                  let metadata = context?.protocolMetadata(definition: NWProtocolWebSocket.definition)
                    as? NWProtocolWebSocket.Metadata
            else { return }
            if metadata.opcode == .text {
                let reply = NWConnection.ContentContext(
                    identifier: "echo",
                    metadata: [NWProtocolWebSocket.Metadata(opcode: .text)]
                )
                connection.send(content: Data("echo:".utf8) + data, contentContext: reply, completion: .idempotent)
            }
            echo(on: connection)
        }
    }
}

@Suite struct URLSessionWebSocketTransportTests {
    @Test func exchangesTextFramesWithARealWebSocketServer() async throws {
        let server = try EchoServer()
        let port = try await server.start()
        defer { server.stop() }

        let transport = URLSessionWebSocketTransport()
        try await transport.connect(to: URL(string: "ws://127.0.0.1:\(port)/")!, headers: [:])
        try await transport.send("hello")
        try await transport.send("again")

        var frames: [String] = []
        for try await frame in transport.incoming {
            frames.append(frame)
            if frames.count == 2 { break }
        }
        #expect(frames == ["echo:hello", "echo:again"])
        await transport.close()
    }

    @Test func connectingToAClosedPortThrows() async throws {
        let server = try EchoServer()
        let port = try await server.start()
        server.stop()
        try await Task.sleep(for: .milliseconds(100))

        let transport = URLSessionWebSocketTransport()
        await #expect(throws: (any Error).self) {
            try await transport.connect(to: URL(string: "ws://127.0.0.1:\(port)/")!, headers: [:])
        }
    }
}

/// Accepts one TCP connection and hands back the bytes of the HTTP upgrade request.
private final class UpgradeRequestRecorder: Sendable {
    private let listener: NWListener
    private let requests: AsyncStream<String>
    private let captured: AsyncStream<String>.Continuation

    init() throws {
        let parameters = NWParameters.tcp
        parameters.requiredLocalEndpoint = .hostPort(host: "127.0.0.1", port: .any)
        listener = try NWListener(using: parameters)
        (requests, captured) = AsyncStream.makeStream(of: String.self)
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
            listener.newConnectionHandler = { [captured] connection in
                connection.start(queue: .global())
                connection.receive(minimumIncompleteLength: 1, maximumLength: 16 * 1024) { data, _, _, _ in
                    captured.yield(String(decoding: data ?? Data(), as: UTF8.self))
                    connection.cancel()
                }
            }
            listener.start(queue: .global())
        }
    }

    func firstRequest() async -> String {
        for await request in requests { return request }
        return ""
    }

    func stop() {
        listener.cancel()
    }
}

@Suite struct UpgradeRequestTests {
    /// The gateway answers 403 to any upgrade that carries an `Origin` header, so that a web
    /// page cannot reach it. A native client must not send one.
    @Test func theUpgradeRequestCarriesNoOriginHeader() async throws {
        let recorder = try UpgradeRequestRecorder()
        let port = try await recorder.start()
        defer { recorder.stop() }

        let endpoint = try WireCodec().endpoint(for: HostAddress(host: "127.0.0.1", port: Int(port)))
        let transport = URLSessionWebSocketTransport()
        // The recorder never completes the handshake, so connecting fails; only the request matters.
        try? await transport.connect(to: endpoint.url, headers: endpoint.headers)
        await transport.close()

        let request = await recorder.firstRequest()
        let headers = request.split(whereSeparator: \.isNewline).map { $0.lowercased() }
        #expect(headers.first?.hasPrefix("get / http/1.1") == true)
        #expect(headers.contains { $0.hasPrefix("upgrade: websocket") })
        #expect(!headers.contains { $0.hasPrefix("origin:") }, "URLSession sent an Origin header: \(request)")
    }
}
