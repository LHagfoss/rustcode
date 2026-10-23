import Foundation

// The wire boundary.
//
// Everything on this side of `RemoteCodec` is expressed in domain types. The JSON shapes of
// remote protocol v1 and of the gateway handshake live in `Sources/RustCodeRemote/Wire/`,
// behind `WireCodec`. Nothing outside this file's types knows about them.

/// The operations of remote protocol v1 and the two handshake requests, in domain terms.
public enum RemoteCommand: Sendable, Equatable {
    /// First frame of a pairing connection. Answered by `.handshake(.paired)`. `installID`
    /// lets the host replace the record this install paired before.
    case pair(credential: PairingCredential, deviceName: String, installID: String? = nil)
    /// First frame of every other connection. Answered by `.handshake(.authenticated)`.
    case authenticate(device: DeviceIdentity, deviceToken: String)
    case subscribeSessions
    case listDirectories(path: String?)
    case createSession(path: String?)
    case attachSession(SessionRef, resume: ResumeCursor?)
    case detachSession(SessionRef)
    case getHistory(SessionRef, before: HistoryCursor, limit: Int)
    case getContent(SessionRef, contentID: String, offset: UInt64, maxBytes: Int)
    case uploadImage(SessionRef, ImageUploadChunk)
    case executeCommand(SessionRef, command: String)
    case cancelQuestion(SessionRef, questionID: String)
    case setSessionSettings(SessionRef, model: String, reasoningEffort: String)
    case submitPrompt(SessionRef, text: String)
    case steer(SessionRef, text: String)
    case queue(SessionRef, text: String)
    case cancelTurn(SessionRef, turnID: String)
    case answerQuestion(SessionRef, questionID: String, answer: QuestionAnswer)
    case resolveApproval(SessionRef, batchID: String, decision: ApprovalDecision)
    case getRequestStatus(SessionRef, target: RequestID)
    /// Asks the live terminal of an unshared session to share it. Answered by
    /// `.shareRequestSent`; the outcome comes from `getRequestStatus` on the same request.
    case requestShare(sessionID: String)
    /// One page of the host's saved sessions. Needs `HostCapability.savedSessions`.
    case listSavedSessions(cursor: String?, limit: Int?, query: String?)
    /// Reopens a saved session. Answered by `.sessionResumed` once it is shared.
    case resumeSession(sessionID: String)

    public var isHandshake: Bool {
        switch self {
        case .pair, .authenticate: true
        default: false
        }
    }
}

/// Where a replay should continue. The gateway ID is the one the host reported when the
/// session was attached; a cursor from another gateway is worthless.
public struct ResumeCursor: Sendable, Equatable {
    public var origin: ReplayOrigin
    public var lastSequence: UInt64
    public var snapshotID: String?

    public init(origin: ReplayOrigin, lastSequence: UInt64, snapshotID: String? = nil) {
        self.origin = origin
        self.lastSequence = lastSequence
        self.snapshotID = snapshotID
    }
}

/// The gateway, and the run of it, that numbered a session's events.
public struct ReplayOrigin: Sendable, Equatable {
    public var gatewayID: String
    public var instanceID: String?

    public init(gatewayID: String, instanceID: String? = nil) {
        self.gatewayID = gatewayID
        self.instanceID = instanceID
    }
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
    /// The gateway the QR code named. The host that answers must be that one.
    public var expectedGatewayID: String?
    /// The host's name as the QR code gave it.
    public var hostName: String?

    public init(
        address: HostAddress,
        credential: PairingCredential,
        expectedGatewayID: String? = nil,
        hostName: String? = nil
    ) {
        self.address = address
        self.credential = credential
        self.expectedGatewayID = expectedGatewayID
        self.hostName = hostName
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

public enum HandshakeReply: Sendable, Equatable {
    case paired(PairingGrant)
    case authenticated(ConnectionIdentity)
}

/// What the host answers to one command.
public enum RemoteReply: Sendable, Equatable {
    case handshake(HandshakeReply)
    /// The live list, and the host-wide operations the host offers.
    case sessions([SessionSummary], capabilities: Set<String> = [])
    case savedSessions(SavedSessionPage)
    /// A saved session is shared again and can be attached.
    case sessionResumed(SessionSummary)
    case imageUploaded(ImageUploadProgress)
    case directories(DirectoryListing)
    case sessionCreated(SessionSummary)
    /// `origin` is what a later resume cursor for this session has to name.
    case attached(SessionAttachment.Start, origin: ReplayOrigin)
    case detached
    case history(HistoryPage)
    case content(ContentChunk)
    /// A mutation's receipt, or the answer to `getRequestStatus`. A `received` receipt may be
    /// followed by a final one for the same request.
    case commandExecuted(SessionCommandResult)
    /// A share request reached the session's terminal and awaits its answer.
    case shareRequestSent(pendingID: String)
    /// The terminal shared the session; it can be attached.
    case shareAccepted(SessionSummary)
    /// The terminal declined, the request expired, or the terminal went away.
    case shareDenied(reason: String)
    case receipt(ReceiptState)
    case failure(RemoteError)
    /// A result this version of the app does not know.
    case unknown(String)
}

/// An error frame that names no request. During the handshake it answers the first frame.
/// Afterwards every reply is a correlated response, so it can only mean that the gateway
/// is closing the connection, and why.
public struct GatewayFailure: Sendable, Equatable {
    public var error: RemoteError

    public init(error: RemoteError) {
        self.error = error
    }
}

/// One decoded incoming frame.
public enum RemoteMessage: Sendable, Equatable {
    case reply(requestID: RequestID, RemoteReply)
    case handshake(HandshakeReply)
    case gatewayFailure(GatewayFailure)
    case sessionsChanged([SessionSummary])
    case sessionUpdate(SessionUpdate)
    case resyncRequired(SessionRef)
    /// The session's registration ended; `reason` is ready to show.
    case sessionClosed(SessionRef, reason: String)
    /// A well-formed frame of a kind this version does not know. Ignored.
    case unknown(String)
}

public protocol RemoteCodec: Sendable {
    /// Interprets a scanned QR payload or a manual address and code.
    func pairingTarget(for request: PairingRequest) throws -> PairingTarget

    func endpoint(for address: HostAddress) throws -> ConnectionEndpoint

    func encode(_ command: RemoteCommand, requestID: RequestID) throws -> String

    /// Throws for any frame that does not match the schema.
    func decode(_ frame: String) throws -> RemoteMessage
}

/// A codec that refuses everything with `RemoteError.codecUnavailable`, for a build or a
/// test that must not talk to a host. The app uses `WireCodec`.
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
