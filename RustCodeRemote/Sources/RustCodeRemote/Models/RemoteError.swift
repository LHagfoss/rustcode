import Foundation

public enum RemoteError: Error, Sendable, Equatable {
    case notConnected
    case connectionClosed
    /// The device token was rejected or revoked. Reconnecting cannot fix this; pair again.
    case unauthorized
    case incompatibleProtocol
    /// A frame did not match the protocol. The connection is dropped rather than guessed at.
    case invalidFrame(String)
    /// This build has no wire codec yet, so it cannot talk to a real gateway.
    case codecUnavailable
    case timeout
    /// Updates were lost or the cursor is no longer valid; attach again for a fresh snapshot.
    case resyncRequired
    case pairingFailed(String)
    case transport(String)
    case rejected(RemoteRejection)

    public var isUnauthorized: Bool { self == .unauthorized }
}

extension RemoteError: LocalizedError {
    public var errorDescription: String? {
        switch self {
        case .notConnected: "Not connected to the host."
        case .connectionClosed: "The connection to the host was closed."
        case .unauthorized: "This device is no longer authorized. Pair it again from the terminal."
        case .incompatibleProtocol: "The host speaks a protocol version this app does not support."
        case .invalidFrame(let detail): "The host sent data this app could not understand (\(detail))."
        case .codecUnavailable: "Connecting to a real host is not available in this build yet. Try the demo."
        case .timeout: "The host did not answer in time."
        case .resyncRequired: "The session has to be reloaded."
        case .pairingFailed(let detail): "Pairing failed: \(detail)"
        case .transport(let detail): detail
        case .rejected(let rejection): rejection.message
        }
    }
}

extension RemoteRejection {
    public var message: String {
        switch self {
        case .busy: "The session is busy. Queue the message or wait for the turn to finish."
        case .staleQuestion: "That question was already answered."
        case .staleApproval: "That approval was already resolved."
        case .staleTurn: "That turn has already ended."
        case .staleSession: "The session is no longer shared."
        case .unsupportedOperation: "The host does not support that from a remote device."
        case .other(let code, let message): message ?? "The host rejected the request (\(code))."
        }
    }
}
