import Foundation

/// One WebSocket connection carrying text frames. An instance is used for a single
/// connection: connect once, exchange frames, close.
public protocol RemoteTransport: Sendable {
    /// Opens the connection. Returns once the socket is usable or throws.
    func connect(to url: URL, headers: [String: String]) async throws

    func send(_ text: String) async throws

    /// Incoming text frames. Finishes when the peer closes, and throws when the connection fails.
    /// Intended for a single consumer.
    nonisolated var incoming: AsyncThrowingStream<String, any Error> { get }

    func close() async
}
