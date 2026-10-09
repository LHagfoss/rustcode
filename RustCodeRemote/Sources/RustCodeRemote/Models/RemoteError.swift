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
    /// Too many failed attempts on the host. Nothing is accepted until the time has passed.
    case rateLimited(retryAfter: Duration?)
    /// The owner could not say whether a mutation was applied. Never resend on that basis.
    case outcomeUnknown
    /// The session's registration ended; the associated value says why.
    case sessionClosed(String)
    /// The host at the stored address is not the gateway this device paired with.
    case hostIdentityChanged
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
        case .codecUnavailable: "Connecting to a real host is not available in this build."
        case .timeout: "The host did not answer in time."
        case .resyncRequired: "The session has to be reloaded."
        case .pairingFailed(let detail): "Pairing failed: \(detail)"
        case .rateLimited(let retryAfter):
            if let retryAfter {
                "Too many failed attempts. Try again in \(retryAfter.formatted(.units(allowed: [.minutes, .seconds], width: .wide)))."
            } else {
                "Too many failed attempts. Try again later."
            }
        case .outcomeUnknown: "The host could not confirm the request."
        case .sessionClosed(let reason): reason
        case .hostIdentityChanged: "The host at this address is not the one this device was paired with. Pair again."
        case .transport(let detail): detail
        case .rejected(let rejection): rejection.message
        }
    }
}

extension RemoteRejection {
    public var message: String {
        switch self {
        case .busy: "The session is busy. Queue the message or wait for the turn to finish."
        case .notRunning: "There is no running turn any more. Send the message again to start one."
        case .invalidAnswer: "That answer does not match the question."
        case .staleQuestion: "That question was already answered."
        case .staleApproval: "That approval was already resolved."
        case .staleTurn: "That turn has already ended."
        case .staleSession: "The session is no longer shared."
        case .unsupportedOperation: "The host does not support that from a remote device."
        case .other(let code, let message): message ?? "The host rejected the request (\(code))."
        }
    }
}
