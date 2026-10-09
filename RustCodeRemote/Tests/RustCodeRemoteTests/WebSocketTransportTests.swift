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
