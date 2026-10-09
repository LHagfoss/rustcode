import Foundation

/// An in-memory `RemoteTransport` for tests: records what was sent and plays back frames
/// pushed by the test or produced by a responder.
public actor ScriptedTransport: RemoteTransport {
    public typealias Responder = @Sendable (String) -> [String]

    public nonisolated let incoming: AsyncThrowingStream<String, any Error>

    public private(set) var sent: [String] = []
    public private(set) var connectedURL: URL?
    public private(set) var connectedHeaders: [String: String] = [:]
    public private(set) var isClosed = false

    private let continuation: AsyncThrowingStream<String, any Error>.Continuation
    private let connectError: (any Error)?
    private var responder: Responder?

    /// - Parameters:
    ///   - connectError: thrown from `connect` to simulate an unreachable host.
    ///   - responder: called for every sent frame; the frames it returns arrive as incoming.
    public init(connectError: (any Error)? = nil, responder: Responder? = nil) {
        self.connectError = connectError
        self.responder = responder
        (incoming, continuation) = AsyncThrowingStream.makeStream(of: String.self)
    }

    public func connect(to url: URL, headers: [String: String]) async throws {
        if let connectError {
            continuation.finish(throwing: connectError)
            throw connectError
        }
        connectedURL = url
        connectedHeaders = headers
    }

    public func send(_ text: String) async throws {
        guard connectedURL != nil, !isClosed else { throw RemoteError.notConnected }
        sent.append(text)
        for frame in responder?(text) ?? [] {
            continuation.yield(frame)
        }
    }

    public func close() async {
        isClosed = true
        continuation.finish()
    }

    // MARK: Test controls

    public func setResponder(_ responder: Responder?) {
        self.responder = responder
    }

    /// Delivers a frame as if the host had sent it.
    public func push(_ frame: String) {
        continuation.yield(frame)
    }

    /// Ends the incoming stream, as a dropped connection (with an error) or a clean close.
    public func drop(throwing error: (any Error)? = nil) {
        isClosed = true
        continuation.finish(throwing: error)
    }
}
