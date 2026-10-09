import Foundation

// The wire boundary.
//
// Everything on this side of `RemoteCodec` is expressed in domain types. The JSON shape of
// remote protocol v1 (envelope, field names, error codes, the pairing QR layout, the
// WebSocket path and how the device token is presented) belongs to a `RemoteCodec`
// implementation built against the schema and golden frames in the RustCode repository.
// That implementation and its `Codable` DTOs go in `Sources/RustCodeRemote/Wire/`; nothing
// outside this file's types has to change when it lands.

/// The operations of remote protocol v1, in domain terms.
public enum RemoteCommand: Sendable, Equatable {
    case pair(credential: PairingCredential, deviceName: String)
    case authenticate(device: DeviceIdentity, deviceToken: String)
    case subscribeSessions
    case attachSession(SessionRef, resumeAfter: UInt64?)
    case detachSession(SessionRef)
    case getHistory(SessionRef, before: HistoryCursor, limit: Int)
    case submitPrompt(SessionRef, text: String)
    case steer(SessionRef, text: String)
    case queue(SessionRef, text: String)
    case cancelTurn(SessionRef, turnID: String)
    case answerQuestion(SessionRef, questionID: String, answer: QuestionAnswer)
    case resolveApproval(SessionRef, batchID: String, decision: ApprovalDecision)
    case getRequestStatus(SessionRef, target: RequestID)
}

public enum PairingCredential: Sendable, Equatable {
    /// The single-use credential carried by the QR code.
    case oneTime(String)
    /// The short code typed by hand.
    case shortCode(String)
}

/// Where to pair and with what, after the codec has interpreted the user's input.
public struct PairingTarget: Sendable, Equatable {
    public var address: HostAddress
    public var credential: PairingCredential

    public init(address: HostAddress, credential: PairingCredential) {
        self.address = address
        self.credential = credential
    }
}

public struct ConnectionEndpoint: Sendable, Equatable {
    public var url: URL
    /// Never put a device token in the URL.
    public var headers: [String: String]

    public init(url: URL, headers: [String: String] = [:]) {
        self.url = url
        self.headers = headers
    }
}

/// What the host answers to one command.
public enum RemoteReply: Sendable, Equatable {
    case paired(StoredCredential)
    case authenticated
    case sessions([SessionSummary])
    case attached(SessionAttachment.Start)
    case detached
    case history(HistoryPage)
    /// A mutation's receipt, or the answer to `getRequestStatus`. A `received` receipt may be
    /// followed by a final one for the same request.
    case receipt(ReceiptState)
    case failure(RemoteError)
}

/// One decoded incoming frame.
public enum RemoteMessage: Sendable, Equatable {
    case reply(requestID: RequestID, RemoteReply)
    case sessionsChanged([SessionSummary])
    case sessionUpdate(SessionUpdate)
    case resyncRequired(SessionRef)
    /// The host is ending the connection, for example after revoking this device.
    case closing(RemoteError)
}

public protocol RemoteCodec: Sendable {
    /// Interprets a scanned QR payload or a manual address and code.
    func pairingTarget(for request: PairingRequest) throws -> PairingTarget

    func endpoint(for address: HostAddress) throws -> ConnectionEndpoint

    func encode(_ command: RemoteCommand, requestID: RequestID) throws -> String

    /// Throws for any frame that does not match the schema.
    func decode(_ frame: String) throws -> RemoteMessage
}

/// Placeholder until the schema-backed codec exists. Every use fails with
/// `RemoteError.codecUnavailable`, which the app shows as "not available in this build".
public struct UnavailableRemoteCodec: RemoteCodec {
    public init() {}

    public func pairingTarget(for request: PairingRequest) throws -> PairingTarget {
        throw RemoteError.codecUnavailable
    }

    public func endpoint(for address: HostAddress) throws -> ConnectionEndpoint {
        throw RemoteError.codecUnavailable
    }

    public func encode(_ command: RemoteCommand, requestID: RequestID) throws -> String {
        throw RemoteError.codecUnavailable
    }

    public func decode(_ frame: String) throws -> RemoteMessage {
        throw RemoteError.codecUnavailable
    }
}
