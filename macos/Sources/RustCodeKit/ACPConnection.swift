import Foundation

/// Messages the agent sends that the client must act on.
public enum ACPInbound: Equatable, Sendable {
    case notification(method: String, params: JSONValue)
    case request(id: JSONValue, method: String, params: JSONValue)
    /// The stream ended (engine exited or the transport failed).
    case closed(reason: String)
}

public enum ACPConnectionError: Error, Equatable, CustomStringConvertible {
    case closed(String)
    case timedOut(String)
    case writeFailed(String)

    public var description: String {
        switch self {
        case .closed(let reason): return "The RustCode engine connection closed: \(reason)"
        case .timedOut(let method): return "The RustCode engine did not answer \(method) in time."
        case .writeFailed(let reason): return "Could not write to the RustCode engine: \(reason)"
        }
    }
}

/// JSON-RPC 2.0 peer for ACP. Outgoing requests await their response by id;
/// agent-initiated requests and notifications are delivered in order on
/// `inbound`. Transport-agnostic so tests drive it without a process.
public actor ACPConnection {
    public typealias Writer = @Sendable (Data) throws -> Void

    private let writer: Writer
    private var nextId = 1
    private var pending: [Int: CheckedContinuation<JSONValue, Error>] = [:]
    private var closedReason: String?
    /// Requests and notifications delivered on `inbound` so far. A response
    /// resolves at once, ahead of notifications the agent sent before it that
    /// are still queued, so a client counts what it handled against this.
    public private(set) var inboundCount = 0
    private let continuation: AsyncStream<ACPInbound>.Continuation
    public nonisolated let inbound: AsyncStream<ACPInbound>

    public init(writer: @escaping Writer) {
        self.writer = writer
        var continuation: AsyncStream<ACPInbound>.Continuation!
        self.inbound = AsyncStream(bufferingPolicy: .unbounded) { continuation = $0 }
        self.continuation = continuation
    }

    public var isClosed: Bool { closedReason != nil }

    /// A request is waiting for its response.
    var hasPending: Bool { !pending.isEmpty }

    /// Send a request and wait for its result. `timeout` bounds handshake-style
    /// calls; prompts pass `nil` because a turn may legitimately run for hours.
    public func request(
        _ method: String, params: JSONValue, timeout: Duration? = nil
    ) async throws -> JSONValue {
        if let closedReason { throw ACPConnectionError.closed(closedReason) }
        let id = nextId
        nextId += 1
        let line = try RPCMessage.request(id: .number(Double(id)), method: method, params: params)
            .encodedLine()
        let timeoutTask = timeout.map { limit in
            Task { [weak self] in
                try? await Task.sleep(for: limit)
                await self?.expire(id: id, method: method)
            }
        }
        defer { timeoutTask?.cancel() }
        return try await withCheckedThrowingContinuation { continuation in
            pending[id] = continuation
            do {
                try writer(line)
            } catch {
                pending.removeValue(forKey: id)
                continuation.resume(throwing: ACPConnectionError.writeFailed("\(error)"))
            }
        }
    }

    public func notify(_ method: String, params: JSONValue) throws {
        if let closedReason { throw ACPConnectionError.closed(closedReason) }
        try send(.notification(method: method, params: params))
    }

    public func respond(id: JSONValue, result: JSONValue) throws {
        try send(.response(id: id, result: result))
    }

    public func respond(id: JSONValue, error: RPCError) throws {
        try send(.error(id: id, error: error))
    }

    /// Feed one framed line from the agent.
    public func receive(line: Data) {
        guard closedReason == nil else { return }
        let message: RPCMessage
        do {
            message = try RPCMessage.decode(line)
        } catch {
            // ACP reserves stdout for protocol traffic; a stray line is a
            // protocol violation but not worth tearing the session down for.
            return
        }
        switch message {
        case .request(let id, let method, let params):
            inboundCount += 1
            continuation.yield(.request(id: id, method: method, params: params))
        case .notification(let method, let params):
            inboundCount += 1
            continuation.yield(.notification(method: method, params: params))
        case .response(let id, let result):
            if let key = id.intValue, let waiter = pending.removeValue(forKey: key) {
                waiter.resume(returning: result)
            }
        case .error(let id, let error):
            if let key = id.intValue, let waiter = pending.removeValue(forKey: key) {
                waiter.resume(throwing: error)
            }
        }
    }

    /// Fail every outstanding request and end `inbound`. Idempotent.
    public func close(reason: String) {
        guard closedReason == nil else { return }
        closedReason = reason
        let waiters = pending
        pending.removeAll()
        for waiter in waiters.values {
            waiter.resume(throwing: ACPConnectionError.closed(reason))
        }
        continuation.yield(.closed(reason: reason))
        continuation.finish()
    }

    private func expire(id: Int, method: String) {
        pending.removeValue(forKey: id)?.resume(throwing: ACPConnectionError.timedOut(method))
    }

    private func send(_ message: RPCMessage) throws {
        do {
            try writer(try message.encodedLine())
        } catch {
            throw ACPConnectionError.writeFailed("\(error)")
        }
    }
}
