import Foundation
import Synchronization

/// `RemoteTransport` on top of `URLSessionWebSocketTask`.
public actor URLSessionWebSocketTransport: RemoteTransport {
    public nonisolated let incoming: AsyncThrowingStream<String, any Error>

    private let continuation: AsyncThrowingStream<String, any Error>.Continuation
    private let session: URLSession
    private let maximumMessageSize: Int
    private var task: URLSessionWebSocketTask?
    private var receiveTask: Task<Void, Never>?
    private var keepAliveTask: Task<Void, Never>?
    private let keepAliveInterval: Duration

    /// - Parameter maximumMessageSize: frames above this size fail the connection. The protocol
    ///   bounds every frame, so this is a guard, not a limit the host is expected to reach.
    /// - Parameter keepAliveInterval: how often to ping. The gateway drops a connection it
    ///   has not heard from for 90 seconds, and an idle phone sends nothing else.
    public init(
        configuration: URLSessionConfiguration = .ephemeral,
        maximumMessageSize: Int = 4 * 1024 * 1024,
        keepAliveInterval: Duration = .seconds(25)
    ) {
        self.keepAliveInterval = keepAliveInterval
        configuration.waitsForConnectivity = false
        configuration.timeoutIntervalForRequest = 15
        self.session = URLSession(configuration: configuration)
        self.maximumMessageSize = maximumMessageSize
        (incoming, continuation) = AsyncThrowingStream.makeStream(of: String.self)
    }

    public func connect(to url: URL, headers: [String: String]) async throws {
        guard task == nil else { throw RemoteError.transport("This transport has already been used.") }
        var request = URLRequest(url: url)
        for (name, value) in headers {
            request.setValue(value, forHTTPHeaderField: name)
        }
        let task = session.webSocketTask(with: request)
        task.maximumMessageSize = maximumMessageSize
        self.task = task
        task.resume()

        // `resume()` reports nothing about the handshake. A ping only completes once the
        // socket is open, so it doubles as the "connected" signal.
        do {
            try await Self.ping(task)
        } catch {
            task.cancel(with: .abnormalClosure, reason: nil)
            continuation.finish(throwing: error)
            throw error
        }

        keepAliveTask = Task { [keepAliveInterval] in
            while !Task.isCancelled {
                try? await Task.sleep(for: keepAliveInterval)
                guard !Task.isCancelled else { return }
                // A failed ping also fails the receive loop, which reports the error.
                try? await Self.ping(task)
            }
        }

        receiveTask = Task { [continuation] in
            do {
                while !Task.isCancelled {
                    switch try await task.receive() {
                    case .string(let text):
                        continuation.yield(text)
                    case .data:
                        throw RemoteError.invalidFrame("binary frame")
                    @unknown default:
                        throw RemoteError.invalidFrame("unknown frame type")
                    }
                }
                continuation.finish()
            } catch {
                if task.closeCode == .normalClosure || task.closeCode == .goingAway {
                    continuation.finish()
                } else {
                    continuation.finish(throwing: error)
                }
            }
        }
    }

    public func send(_ text: String) async throws {
        guard let task else { throw RemoteError.notConnected }
        try await task.send(.string(text))
    }

    public func close() async {
        receiveTask?.cancel()
        keepAliveTask?.cancel()
        task?.cancel(with: .normalClosure, reason: nil)
        session.invalidateAndCancel()
        continuation.finish()
    }

    private static func ping(_ task: URLSessionWebSocketTask) async throws {
        // The pong handler can fire more than once when the task is cancelled; resume once.
        let resumed = Mutex(false)
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, any Error>) in
            task.sendPing { error in
                let isFirst = resumed.withLock { done in
                    defer { done = true }
                    return !done
                }
                guard isFirst else { return }
                if let error {
                    continuation.resume(throwing: error)
                } else {
                    continuation.resume()
                }
            }
        }
    }
}
