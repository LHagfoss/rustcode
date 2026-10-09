import Foundation

/// Chosen by the client and kept stable for as long as the request is unresolved, so the
/// host can deduplicate and the outcome can be queried after a disconnect.
public struct RequestID: Hashable, Sendable, CustomStringConvertible {
    public var rawValue: String

    public init(_ rawValue: String) {
        self.rawValue = rawValue
    }

    public init() {
        self.rawValue = UUID().uuidString.lowercased()
    }

    public var description: String { rawValue }
}

/// Why the session owner refused an operation.
public enum RemoteRejection: Sendable, Equatable {
    /// Prompt submission is idle-only and the session is running.
    case busy
    /// Steering or queueing needs a running turn and there is none.
    case notRunning
    case invalidAnswer
    case staleQuestion
    case staleApproval
    case staleTurn
    /// The session or its registration epoch is gone.
    case staleSession
    /// For example a slash command sent from the phone, or steering a session that cannot be steered.
    case unsupportedOperation
    case other(code: String, message: String?)
}

/// The owner's word on a mutation. `received` means the gateway has it but the owner has
/// not applied it; `unknown` means nobody can say, and the client must not resend on its own.
public enum ReceiptState: Sendable, Equatable {
    case received
    case applied
    case rejected(RemoteRejection)
    case unknown

    public var isFinal: Bool {
        switch self {
        case .applied, .rejected: true
        case .received, .unknown: false
        }
    }
}

public struct RequestReceipt: Sendable, Equatable {
    public var requestID: RequestID
    public var state: ReceiptState

    public init(requestID: RequestID, state: ReceiptState) {
        self.requestID = requestID
        self.state = state
    }
}

/// A mutation this device issued, with what is known about its outcome.
public struct TrackedRequest: Identifiable, Sendable, Equatable {
    public enum Kind: Sendable, Equatable {
        case submit(String)
        case steer(String)
        case queue(String)
        case cancel
        case answer
        case approval
    }

    public enum Phase: Sendable, Equatable {
        /// Sent, no receipt yet.
        case sending
        /// Never left the device, so it is safe to try again.
        case notSent(String)
        case settled(ReceiptState)
    }

    public var id: RequestID
    public var session: SessionRef
    public var kind: Kind
    public var phase: Phase

    public init(id: RequestID, session: SessionRef, kind: Kind, phase: Phase = .sending) {
        self.id = id
        self.session = session
        self.kind = kind
        self.phase = phase
    }

    /// True while the outcome is not known to be applied or rejected.
    public var isUncertain: Bool {
        switch phase {
        case .settled(.unknown), .settled(.received): true
        default: false
        }
    }

    public var text: String? {
        switch kind {
        case .submit(let text), .steer(let text), .queue(let text): text
        case .cancel, .answer, .approval: nil
        }
    }
}

/// The receipt reducer. Applied requests leave the ledger, because the transcript now shows
/// their effect; everything else stays until the user has seen it.
public struct ReceiptLedger: Sendable, Equatable {
    public private(set) var requests: [TrackedRequest] = []

    public init() {}

    public mutating func begin(_ request: TrackedRequest) {
        requests.append(request)
    }

    public mutating func record(_ receipt: RequestReceipt) {
        guard let index = requests.firstIndex(where: { $0.id == receipt.requestID }) else { return }
        if receipt.state == .applied {
            requests.remove(at: index)
        } else if !Self.isRegression(from: requests[index].phase, to: receipt.state) {
            requests[index].phase = .settled(receipt.state)
        }
    }

    /// The request may or may not have been applied: the reply never arrived.
    public mutating func markUncertain(_ id: RequestID) {
        guard let index = requests.firstIndex(where: { $0.id == id }) else { return }
        if case .settled(let state) = requests[index].phase, state.isFinal { return }
        requests[index].phase = .settled(.unknown)
    }

    public mutating func markNotSent(_ id: RequestID, reason: String) {
        guard let index = requests.firstIndex(where: { $0.id == id }) else { return }
        requests[index].phase = .notSent(reason)
    }

    /// Called when the connection drops: every request still in flight becomes uncertain.
    public mutating func connectionLost() {
        for index in requests.indices where requests[index].phase == .sending {
            requests[index].phase = .settled(.unknown)
        }
    }

    public mutating func dismiss(_ id: RequestID) {
        requests.removeAll { $0.id == id }
    }

    public mutating func removeAll() {
        requests.removeAll()
    }

    public var uncertain: [TrackedRequest] { requests.filter(\.isUncertain) }

    public func requests(for session: SessionRef) -> [TrackedRequest] {
        requests.filter { $0.session == session }
    }

    /// A late `received` or `unknown` must not overwrite a rejection the owner already reported.
    private static func isRegression(from phase: TrackedRequest.Phase, to state: ReceiptState) -> Bool {
        guard case .settled(let current) = phase else { return false }
        return current.isFinal && !state.isFinal
    }
}
