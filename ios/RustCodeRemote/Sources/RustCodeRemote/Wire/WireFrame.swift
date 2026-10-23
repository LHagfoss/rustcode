import Foundation

// DTOs for `docs/remote-protocol/v1/frame.schema.json` at the repository root.
// The host omits absent optionals; the schema also tolerates null. Both decode to nil.

public enum WireFrame: Codable, Equatable, Sendable {
    case response(WireResponse)
    case event(WireEventFrame)
    case sessions(WireSessionsFrame)
    case unknown(kind: String, JSONValue)

    public static let knownKinds: Set<String> = ["response", "event", "sessions"]

    public init(from decoder: any Decoder) throws {
        let kind = try WireTag.read("kind", from: decoder)
        switch kind {
        case "response": self = .response(try WireResponse(from: decoder))
        case "event": self = .event(try WireEventFrame(from: decoder))
        case "sessions": self = .sessions(try WireSessionsFrame(from: decoder))
        default: self = .unknown(kind: kind, try JSONValue(from: decoder))
        }
    }

    public func encode(to encoder: any Encoder) throws {
        switch self {
        case .response(let payload): try WireTag.write("response", as: "kind", payload: payload, to: encoder)
        case .event(let payload): try WireTag.write("event", as: "kind", payload: payload, to: encoder)
        case .sessions(let payload): try WireTag.write("sessions", as: "kind", payload: payload, to: encoder)
        case .unknown(_, let raw): try raw.encode(to: encoder)
        }
    }
}

// MARK: - Responses

public struct WireResponse: Codable, Equatable, Sendable {
    public var protocolVersion: UInt32
    public var requestID: String
    public var receipt: WireOpen<WireReceiptState>?
    public var result: WireResult

    enum CodingKeys: String, CodingKey {
        case protocolVersion = "protocol_version"
        case requestID = "request_id"
        case receipt
        case result
    }
}

public enum WireReceiptState: String, Sendable, Equatable, CaseIterable {
    case received
    case applied
    case rejected
    case unknown
}

public indirect enum WireResult: Codable, Equatable, Sendable {
    case error(WireError)
    case sessions(Sessions)
    case imageUploaded(ImageUploadProgress)
    case directories(DirectoryListing)
    case sessionCreated(SessionCreated)
    case sessionResumed(SessionCreated)
    case savedSessions(SavedSessions)
    case attached(Attached)
    case resumed(Resumed)
    case detached
    case history(WireHistoryPage)
    case content(WireContentChunk)
    case sessionSettingsUpdated(SessionSettingsUpdated)
    case promptAccepted(PromptAccepted)
    case turnCancelled(TurnCancelled)
    case commandExecuted(SessionCommandResult)
    case questionCancelled(QuestionAnswered)
    case questionAnswered(QuestionAnswered)
    case approvalResolved(ApprovalResolved)
    case shareRequestSent(ShareRequestSent)
    case shareAccepted(ShareAccepted)
    case shareDenied(ShareDenied)
    case requestStatus(RequestStatus)
    case unknown(type: String, JSONValue)

    public static let knownTypes: Set<String> = [
        "image_uploaded", "directories", "session_created", "error", "sessions", "attached", "resumed", "detached", "history", "content",
        "session_settings_updated", "prompt_accepted", "turn_cancelled", "question_answered", "question_cancelled", "command_executed", "approval_resolved", "request_status",
        "share_request_sent", "share_accepted", "share_denied", "session_resumed", "saved_sessions",
    ]

    public struct SessionCreated: Codable, Equatable, Sendable { public var session: WireSessionInfo }

    public struct Sessions: Codable, Equatable, Sendable {
        public var gatewayID: String
        public var instanceID: String?
        public var sessions: [WireSessionInfo]
        public var subscribed: Bool
        /// Optional host-wide operations, such as `saved_sessions`. Absent from older hosts.
        public var capabilities: [String]? = nil

        enum CodingKeys: String, CodingKey {
            case gatewayID = "gateway_id"
            case instanceID = "instance_id"
            case sessions
            case subscribed
            case capabilities
        }
    }

    /// One page of `list_saved_sessions`.
    public struct SavedSessions: Codable, Equatable, Sendable {
        public var sessions: [WireSavedSession]
        public var nextCursor: String?

        enum CodingKeys: String, CodingKey {
            case sessions
            case nextCursor = "next_cursor"
        }
    }

    public struct Attached: Codable, Equatable, Sendable {
        public var gatewayID: String
        public var instanceID: String?
        /// Set when the client asked to resume and could not: whatever it held for the
        /// session is void and this snapshot replaces it.
        public var resync: WireOpen<WireResyncReason>?
        public var snapshot: WireSnapshot

        enum CodingKeys: String, CodingKey {
            case gatewayID = "gateway_id"
            case instanceID = "instance_id"
            case resync
            case snapshot
        }
    }

    public struct Resumed: Codable, Equatable, Sendable {
        public var gatewayID: String
        public var instanceID: String?
        /// Exactly the missed events follow, starting here.
        public var nextSequence: UInt64

        enum CodingKeys: String, CodingKey {
            case gatewayID = "gateway_id"
            case instanceID = "instance_id"
            case nextSequence = "next_sequence"
        }
    }

    public struct SessionSettingsUpdated: Codable, Equatable, Sendable { public var settings: WireSessionSettings }

    public struct PromptAccepted: Codable, Equatable, Sendable {
        public var disposition: WireOpen<WirePromptDisposition>
    }

    public struct TurnCancelled: Codable, Equatable, Sendable {
        public var turnID: String

        enum CodingKeys: String, CodingKey {
            case turnID = "turn_id"
        }
    }

    public struct QuestionAnswered: Codable, Equatable, Sendable {
        public var questionID: String

        enum CodingKeys: String, CodingKey {
            case questionID = "question_id"
        }
    }

    public struct ApprovalResolved: Codable, Equatable, Sendable {
        public var batchID: String
        public var choice: WireOpen<WireApprovalChoice>

        enum CodingKeys: String, CodingKey {
            case batchID = "batch_id"
            case choice
        }
    }

    /// A `request_share` reached the session's terminal; the outcome follows from
    /// `get_request_status` on the same request ID.
    public struct ShareRequestSent: Codable, Equatable, Sendable {
        public var pendingID: String

        enum CodingKeys: String, CodingKey {
            case pendingID = "pending_id"
        }
    }

    /// The terminal shared the session; it can be attached.
    public struct ShareAccepted: Codable, Equatable, Sendable { public var session: WireSessionInfo }

    /// The terminal declined, the request expired, or the terminal went away.
    public struct ShareDenied: Codable, Equatable, Sendable { public var reason: String }

    public struct RequestStatus: Codable, Equatable, Sendable {
        public var targetRequestID: String
        public var receipt: WireOpen<WireReceiptState>
        /// The original outcome, when the owner still has it.
        public var result: WireResult?

        enum CodingKeys: String, CodingKey {
            case targetRequestID = "target_request_id"
            case receipt
            case result
        }
    }

    public var type: String {
        switch self {
        case .error: "error"
        case .sessions: "sessions"
        case .imageUploaded: "image_uploaded"
        case .directories: "directories"
        case .sessionCreated: "session_created"
        case .sessionResumed: "session_resumed"
        case .savedSessions: "saved_sessions"
        case .attached: "attached"
        case .resumed: "resumed"
        case .detached: "detached"
        case .history: "history"
        case .content: "content"
        case .sessionSettingsUpdated: "session_settings_updated"
        case .promptAccepted: "prompt_accepted"
        case .turnCancelled: "turn_cancelled"
        case .commandExecuted: "command_executed"
        case .questionCancelled: "question_cancelled"
        case .questionAnswered: "question_answered"
        case .approvalResolved: "approval_resolved"
        case .shareRequestSent: "share_request_sent"
        case .shareAccepted: "share_accepted"
        case .shareDenied: "share_denied"
        case .requestStatus: "request_status"
        case .unknown(let type, _): type
        }
    }

    public init(from decoder: any Decoder) throws {
        let type = try WireTag.read("type", from: decoder)
        switch type {
        case "error": self = .error(try WireError(from: decoder))
        case "sessions": self = .sessions(try Sessions(from: decoder))
        case "image_uploaded": self = .imageUploaded(try ImageUploadProgress(from: decoder))
        case "directories": self = .directories(try DirectoryListing(from: decoder))
        case "session_created": self = .sessionCreated(try SessionCreated(from: decoder))
        case "session_resumed": self = .sessionResumed(try SessionCreated(from: decoder))
        case "saved_sessions": self = .savedSessions(try SavedSessions(from: decoder))
        case "attached": self = .attached(try Attached(from: decoder))
        case "resumed": self = .resumed(try Resumed(from: decoder))
        case "detached": self = .detached
        case "history": self = .history(try WireHistoryPage(from: decoder))
        case "content": self = .content(try WireContentChunk(from: decoder))
        case "session_settings_updated": self = .sessionSettingsUpdated(try SessionSettingsUpdated(from: decoder))
        case "prompt_accepted": self = .promptAccepted(try PromptAccepted(from: decoder))
        case "turn_cancelled": self = .turnCancelled(try TurnCancelled(from: decoder))
        case "command_executed": self = .commandExecuted(try SessionCommandResult(from: decoder))
        case "question_cancelled": self = .questionCancelled(try QuestionAnswered(from: decoder))
        case "question_answered": self = .questionAnswered(try QuestionAnswered(from: decoder))
        case "approval_resolved": self = .approvalResolved(try ApprovalResolved(from: decoder))
        case "share_request_sent": self = .shareRequestSent(try ShareRequestSent(from: decoder))
        case "share_accepted": self = .shareAccepted(try ShareAccepted(from: decoder))
        case "share_denied": self = .shareDenied(try ShareDenied(from: decoder))
        case "request_status": self = .requestStatus(try RequestStatus(from: decoder))
        default: self = .unknown(type: type, try JSONValue(from: decoder))
        }
    }

    public func encode(to encoder: any Encoder) throws {
        let payload: (any Encodable)? = switch self {
        case .error(let payload): payload
        case .sessions(let payload): payload
        case .imageUploaded(let payload): payload
        case .directories(let payload): payload
        case .sessionCreated(let payload): payload
        case .sessionResumed(let payload): payload
        case .savedSessions(let payload): payload
        case .attached(let payload): payload
        case .resumed(let payload): payload
        case .detached: nil
        case .history(let payload): payload
        case .content(let payload): payload
        case .sessionSettingsUpdated(let payload): payload
        case .promptAccepted(let payload): payload
        case .turnCancelled(let payload): payload
        case .commandExecuted(let payload): payload
        case .questionCancelled(let payload): payload
        case .questionAnswered(let payload): payload
        case .approvalResolved(let payload): payload
        case .shareRequestSent(let payload): payload
        case .shareAccepted(let payload): payload
        case .shareDenied(let payload): payload
        case .requestStatus(let payload): payload
        case .unknown(_, let raw): raw
        }
        if case .unknown(_, let raw) = self {
            try raw.encode(to: encoder)
        } else {
            try WireTag.write(type, as: "type", payload: payload, to: encoder)
        }
    }
}

public enum WirePromptDisposition: String, Sendable, Equatable, CaseIterable {
    case started
    case queued
    case steered
}

public struct WireError: Codable, Equatable, Sendable {
    public var code: WireOpen<WireErrorCode>
    public var message: String
    /// Only with `incompatible_version`; omitted when empty.
    public var supportedVersions: [UInt32]?

    enum CodingKeys: String, CodingKey {
        case code
        case message
        case supportedVersions = "supported_versions"
    }
}

public enum WireErrorCode: String, Sendable, Equatable, CaseIterable {
    case incompatibleVersion = "incompatible_version"
    case invalidRequest = "invalid_request"
    case unsupportedOperation = "unsupported_operation"
    case unauthorized
    case notFound = "not_found"
    case staleSession = "stale_session"
    case busy
    case notRunning = "not_running"
    case staleTurn = "stale_turn"
    case staleQuestion = "stale_question"
    case invalidAnswer = "invalid_answer"
    case staleApproval = "stale_approval"
    case staleCursor = "stale_cursor"
    case requestConflict = "request_conflict"
    case ownerUnavailable = "owner_unavailable"
    case receiptCapacity = "receipt_capacity"
    case openElsewhere = "open_elsewhere"
    case frameTooLarge = "frame_too_large"
    case rateLimited = "rate_limited"
    case `internal`
}

// MARK: - Events

public struct WireEventFrame: Codable, Equatable, Sendable {
    public var protocolVersion: UInt32
    public var sessionID: String
    public var registrationEpoch: UInt64
    public var sequence: UInt64
    public var generation: UInt64
    public var event: WireEvent

    enum CodingKeys: String, CodingKey {
        case protocolVersion = "protocol_version"
        case sessionID = "session_id"
        case registrationEpoch = "registration_epoch"
        case sequence
        case generation
        case event
    }
}

public enum WireEvent: Codable, Equatable, Sendable {
    case turnStarted(TurnStarted)
    case textDelta(TextDelta)
    case toolStarted(ToolStarted)
    case toolFinished(ToolFinished)
    case subagentUpdated(SubagentUpdated)
    case approvalRequested(ApprovalRequested)
    case approvalResolved(ApprovalResolved)
    case questionRequested(QuestionRequested)
    case questionResolved(QuestionResolved)
    case turnFinished(TurnEnded)
    case turnCancelled(TurnEnded)
    case snapshot(Snapshot)
    case resyncRequired(ResyncRequired)
    case sessionClosed(SessionClosed)
    case unknown(type: String, JSONValue)

    public static let knownTypes: Set<String> = [
        "turn_started", "text_delta", "tool_started", "tool_finished", "subagent_updated",
        "approval_requested", "approval_resolved", "question_requested", "question_resolved",
        "turn_finished", "turn_cancelled", "snapshot", "resync_required", "session_closed",
    ]

    public struct TurnStarted: Codable, Equatable, Sendable {
        /// Absent only when the turn had already ended when the event was projected.
        public var turnID: String?
        public var prompt: WireBoundedText
        public var timing: WireTurnTiming? = nil

        enum CodingKeys: String, CodingKey {
            case turnID = "turn_id"
            case prompt
            case timing
        }
    }

    public struct TextDelta: Codable, Equatable, Sendable {
        public var text: String
        public var timing: WireTurnTiming? = nil
        public var thoughtTokens: UInt32? = nil
        public var thoughtTokensEstimated: Bool? = nil
        public var thoughtTimeMilliseconds: UInt64? = nil

        enum CodingKeys: String, CodingKey {
            case text, timing
            case thoughtTokens = "thought_tokens"
            case thoughtTokensEstimated = "thought_tokens_estimated"
            case thoughtTimeMilliseconds = "thought_time_ms"
        }
    }

    public struct ToolStarted: Codable, Equatable, Sendable {
        public var tool: WireTool
    }

    public struct ToolFinished: Codable, Equatable, Sendable {
        public var id: String
        public var success: Bool
        /// The tool handed off to a background task and has no result yet.
        public var pending: Bool
        public var content: WireBoundedText
    }

    public struct SubagentUpdated: Codable, Equatable, Sendable {
        public var subagent: WireSubagent
    }

    public struct ApprovalRequested: Codable, Equatable, Sendable {
        public var approval: WireApprovalBatch
    }

    public struct ApprovalResolved: Codable, Equatable, Sendable {
        public var batchID: String

        enum CodingKeys: String, CodingKey {
            case batchID = "batch_id"
        }
    }

    public struct QuestionRequested: Codable, Equatable, Sendable {
        public var question: WireQuestion
    }

    public struct QuestionResolved: Codable, Equatable, Sendable {
        public var questionID: String

        enum CodingKeys: String, CodingKey {
            case questionID = "question_id"
        }
    }

    public struct TurnEnded: Codable, Equatable, Sendable {
        public var turnID: String?
        public var timing: WireTurnTiming? = nil

        enum CodingKeys: String, CodingKey {
            case timing
            case turnID = "turn_id"
        }
    }

    public struct Snapshot: Codable, Equatable, Sendable {
        public var snapshot: WireSnapshot
    }

    public struct ResyncRequired: Codable, Equatable, Sendable {
        public var reason: WireOpen<WireResyncReason>
    }

    public struct SessionClosed: Codable, Equatable, Sendable {
        public var reason: WireOpen<WireSessionCloseReason>
    }

    public var type: String {
        switch self {
        case .turnStarted: "turn_started"
        case .textDelta: "text_delta"
        case .toolStarted: "tool_started"
        case .toolFinished: "tool_finished"
        case .subagentUpdated: "subagent_updated"
        case .approvalRequested: "approval_requested"
        case .approvalResolved: "approval_resolved"
        case .questionRequested: "question_requested"
        case .questionResolved: "question_resolved"
        case .turnFinished: "turn_finished"
        case .turnCancelled: "turn_cancelled"
        case .snapshot: "snapshot"
        case .resyncRequired: "resync_required"
        case .sessionClosed: "session_closed"
        case .unknown(let type, _): type
        }
    }

    public init(from decoder: any Decoder) throws {
        let type = try WireTag.read("type", from: decoder)
        switch type {
        case "turn_started": self = .turnStarted(try TurnStarted(from: decoder))
        case "text_delta": self = .textDelta(try TextDelta(from: decoder))
        case "tool_started": self = .toolStarted(try ToolStarted(from: decoder))
        case "tool_finished": self = .toolFinished(try ToolFinished(from: decoder))
        case "subagent_updated": self = .subagentUpdated(try SubagentUpdated(from: decoder))
        case "approval_requested": self = .approvalRequested(try ApprovalRequested(from: decoder))
        case "approval_resolved": self = .approvalResolved(try ApprovalResolved(from: decoder))
        case "question_requested": self = .questionRequested(try QuestionRequested(from: decoder))
        case "question_resolved": self = .questionResolved(try QuestionResolved(from: decoder))
        case "turn_finished": self = .turnFinished(try TurnEnded(from: decoder))
        case "turn_cancelled": self = .turnCancelled(try TurnEnded(from: decoder))
        case "snapshot": self = .snapshot(try Snapshot(from: decoder))
        case "resync_required": self = .resyncRequired(try ResyncRequired(from: decoder))
        case "session_closed": self = .sessionClosed(try SessionClosed(from: decoder))
        default: self = .unknown(type: type, try JSONValue(from: decoder))
        }
    }

    public func encode(to encoder: any Encoder) throws {
        let payload: any Encodable = switch self {
        case .turnStarted(let payload): payload
        case .textDelta(let payload): payload
        case .toolStarted(let payload): payload
        case .toolFinished(let payload): payload
        case .subagentUpdated(let payload): payload
        case .approvalRequested(let payload): payload
        case .approvalResolved(let payload): payload
        case .questionRequested(let payload): payload
        case .questionResolved(let payload): payload
        case .turnFinished(let payload), .turnCancelled(let payload): payload
        case .snapshot(let payload): payload
        case .resyncRequired(let payload): payload
        case .sessionClosed(let payload): payload
        case .unknown(_, let raw): raw
        }
        if case .unknown(_, let raw) = self {
            try raw.encode(to: encoder)
        } else {
            try WireTag.write(type, as: "type", payload: payload, to: encoder)
        }
    }
}

public enum WireResyncReason: String, Sendable, Equatable, CaseIterable {
    case lagged
    case sequenceGap = "sequence_gap"
    case gatewayRestarted = "gateway_restarted"
    case historyChanged = "history_changed"
}

public enum WireSessionCloseReason: String, Sendable, Equatable, CaseIterable {
    case sharingDisabled = "sharing_disabled"
    case sessionChanged = "session_changed"
    case ownerExited = "owner_exited"
    case deviceRevoked = "device_revoked"
}

// MARK: - Sessions

public struct WireSessionsFrame: Codable, Equatable, Sendable {
    public var protocolVersion: UInt32
    public var gatewayID: String
    public var instanceID: String?
    public var sessions: [WireSessionInfo]

    enum CodingKeys: String, CodingKey {
        case protocolVersion = "protocol_version"
        case gatewayID = "gateway_id"
        case instanceID = "instance_id"
        case sessions
    }
}

public struct WireSessionInfo: Codable, Equatable, Sendable {
    public var sessionID: String
    public var registrationEpoch: UInt64
    public var title: String
    public var workspace: String?
    public var model: String
    public var activity: WireOpen<WireSessionActivity>
    public var attention: WireAttention
    public var turnCount: UInt64? = nil
    public var lastActiveAt: String? = nil
    public var health: WireOpen<WireOwnerHealth>

    enum CodingKeys: String, CodingKey {
        case sessionID = "session_id"
        case registrationEpoch = "registration_epoch"
        case title
        case workspace
        case model
        case activity
        case attention
        case health
        case turnCount = "turn_count"
        case lastActiveAt = "last_active_at"
    }
}

/// One session saved on the host, as `list_saved_sessions` reports it.
public struct WireSavedSession: Codable, Equatable, Sendable {
    public var sessionID: String
    public var title: String
    public var workspace: String?
    public var updatedAt: String?
    public var messageCount: UInt64
    public var state: WireOpen<WireSavedSessionState>
    /// What has an `open` session open; absent from older hosts.
    public var openIn: WireOpen<WireSavedSessionHolder>?

    enum CodingKeys: String, CodingKey {
        case sessionID = "session_id"
        case title
        case workspace
        case updatedAt = "updated_at"
        case messageCount = "message_count"
        case state
        case openIn = "open_in"
    }
}

public enum WireSavedSessionState: String, Sendable, Equatable, CaseIterable {
    case saved
    case shared
    case open
}

public enum WireSavedSessionHolder: String, Sendable, Equatable, CaseIterable {
    case app
    case terminal
    case remote
}

public enum WireSessionActivity: String, Sendable, Equatable, CaseIterable {
    case idle
    case running
    case awaitingApproval = "awaiting_approval"
    case awaitingQuestion = "awaiting_question"
}

public struct WireAttention: Codable, Equatable, Sendable {
    public var approval: Bool
    public var question: Bool
}

public enum WireOwnerHealth: String, Sendable, Equatable, CaseIterable {
    case live
    case unresponsive
}

// MARK: - Content

public struct WireBoundedText: Codable, Equatable, Sendable {
    public var text: String
    public var truncated: Bool
    public var offset: UInt64
    public var totalBytes: UInt64
    /// Pass to `get_content` for the rest. Absent when the content cannot be fetched later.
    public var contentID: String?

    enum CodingKeys: String, CodingKey {
        case text
        case truncated
        case offset
        case totalBytes = "total_bytes"
        case contentID = "content_id"
    }
}

public struct WireContentChunk: Codable, Equatable, Sendable {
    public var contentID: String
    public var offset: UInt64
    public var text: String
    public var totalBytes: UInt64
    public var nextOffset: UInt64?

    enum CodingKeys: String, CodingKey {
        case contentID = "content_id"
        case offset
        case text
        case totalBytes = "total_bytes"
        case nextOffset = "next_offset"
    }
}

// MARK: - Snapshot

public struct WireSnapshot: Codable, Equatable, Sendable {
    public var commands: [SessionCommandChoice]? = nil
    public var capabilities: [String]? = nil
    public var settings: WireSessionSettings? = nil
    public var snapshotID: String? = nil
    public var lastTurn: WireTurnTiming? = nil
    public var session: WireSessionInfo
    public var sequence: UInt64
    public var generation: UInt64
    public var turn: WireTurn?
    public var transcript: [WireMessage]
    public var historyRevision: String
    public var historyCursor: String?
    public var pendingQuestion: WireQuestion?
    public var pendingApproval: WireApprovalBatch?
    public var pendingPrompts: [WirePendingPrompt]
    public var subagents: [WireSubagent]
    public var backgroundTasks: [WireBackgroundTask]
    public var omitted: WireOmitted

    enum CodingKeys: String, CodingKey {
        case session
        case sequence
        case generation
        case turn
        case transcript
        case capabilities
        case commands
        case settings
        case snapshotID = "snapshot_id"
        case lastTurn = "last_turn"
        case historyRevision = "history_revision"
        case historyCursor = "history_cursor"
        case pendingQuestion = "pending_question"
        case pendingApproval = "pending_approval"
        case pendingPrompts = "pending_prompts"
        case subagents
        case backgroundTasks = "background_tasks"
        case omitted
    }
}

public struct WireOmitted: Codable, Equatable, Sendable {
    public var tools: UInt32
    public var subagents: UInt32
    public var pendingPrompts: UInt32
    public var backgroundTasks: UInt32

    enum CodingKeys: String, CodingKey {
        case tools
        case subagents
        case pendingPrompts = "pending_prompts"
        case backgroundTasks = "background_tasks"
    }
}

public struct WireTurn: Codable, Equatable, Sendable {
    public var timing: WireTurnTiming? = nil
    public var thoughtTokens: UInt32? = nil
    public var thoughtTokensEstimated: Bool? = nil
    public var thoughtTimeMilliseconds: UInt64? = nil
    public var turnID: String
    public var liveResponse: WireBoundedText
    public var tools: [WireTool]
    public var canSteer: Bool

    enum CodingKeys: String, CodingKey {
        case turnID = "turn_id"
        case timing
        case thoughtTokens = "thought_tokens"
        case thoughtTokensEstimated = "thought_tokens_estimated"
        case thoughtTimeMilliseconds = "thought_time_ms"
        case liveResponse = "live_response"
        case tools
        case canSteer = "can_steer"
    }
}

public struct WireTool: Codable, Equatable, Sendable {
    public var id: String
    public var name: String
    public var detail: String?
    public var state: WireOpen<WireToolState>
}

public enum WireToolState: String, Sendable, Equatable, CaseIterable {
    case preparing
    case running
    case succeeded
    case failed
}

public struct WireSubagent: Codable, Equatable, Sendable {
    public var id: UInt32
    public var name: String
    public var task: WireBoundedText
    public var model: String?
    public var status: WireOpen<WireSubagentStatus>
    public var activeTurn: Bool
    public var parentID: UInt32?
    public var depth: UInt32
    public var messageCount: UInt32

    enum CodingKeys: String, CodingKey {
        case id
        case name
        case task
        case model
        case status
        case activeTurn = "active_turn"
        case parentID = "parent_id"
        case depth
        case messageCount = "message_count"
    }
}

public enum WireSubagentStatus: String, Sendable, Equatable, CaseIterable {
    case queued
    case running
    case interrupted
    case completed
    case failed
    case cancelled
}

public struct WireBackgroundTask: Codable, Equatable, Sendable {
    public var id: String
    public var command: String
    public var elapsedMilliseconds: UInt64

    enum CodingKeys: String, CodingKey {
        case id
        case command
        case elapsedMilliseconds = "elapsed_ms"
    }
}

public struct WireMessage: Codable, Equatable, Sendable {
    public var messageID: String
    public var role: String
    public var content: WireBoundedText
    /// Set on a tool-result message.
    public var tool: WireMessageTool?
    public var timestamp: String?
    public var responseTimeMilliseconds: UInt64? = nil
    public var thoughtTokens: UInt32? = nil
    public var thoughtTokensEstimated: Bool? = nil
    public var thoughtTimeMilliseconds: UInt64? = nil
    public var completedAt: String? = nil
    public var turn: WireTurnTiming? = nil
    public var metadata: [String: JSONValue] = [:]

    enum CodingKeys: String, CodingKey {
        case messageID = "message_id"
        case role
        case content
        case tool
        case timestamp, turn
        case responseTimeMilliseconds = "response_time_ms"
        case thoughtTokens = "thought_tokens"
        case thoughtTokensEstimated = "thought_tokens_estimated"
        case thoughtTimeMilliseconds = "thought_time_ms"
        case completedAt = "completed_at"
    }

    public init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        messageID = try container.decode(String.self, forKey: .messageID)
        role = try container.decode(String.self, forKey: .role)
        content = try container.decode(WireBoundedText.self, forKey: .content)
        tool = try container.decodeIfPresent(WireMessageTool.self, forKey: .tool)
        timestamp = try container.decodeIfPresent(String.self, forKey: .timestamp)
        responseTimeMilliseconds = try container.decodeIfPresent(UInt64.self, forKey: .responseTimeMilliseconds)
        thoughtTimeMilliseconds = try container.decodeIfPresent(UInt64.self, forKey: .thoughtTimeMilliseconds)
        thoughtTokens = try container.decodeIfPresent(UInt32.self, forKey: .thoughtTokens)
        thoughtTokensEstimated = try container.decodeIfPresent(Bool.self, forKey: .thoughtTokensEstimated)
        completedAt = try container.decodeIfPresent(String.self, forKey: .completedAt)
        turn = try container.decodeIfPresent(WireTurnTiming.self, forKey: .turn)
        var fields = try decoder.singleValueContainer().decode([String: JSONValue].self)
        for key in ["message_id", "role", "content", "tool", "timestamp", "response_time_ms", "thought_time_ms", "thought_tokens", "thought_tokens_estimated", "completed_at", "turn"] {
            fields.removeValue(forKey: key)
        }
        metadata = fields
    }

    public func encode(to encoder: any Encoder) throws {
        var extras = encoder.container(keyedBy: WireTag.Key.self)
        for (key, value) in metadata { try extras.encode(value, forKey: .init(key)) }
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(messageID, forKey: .messageID)
        try container.encode(role, forKey: .role)
        try container.encode(content, forKey: .content)
        try container.encodeIfPresent(tool, forKey: .tool)
        try container.encodeIfPresent(timestamp, forKey: .timestamp)
        try container.encodeIfPresent(responseTimeMilliseconds, forKey: .responseTimeMilliseconds)
        try container.encodeIfPresent(thoughtTimeMilliseconds, forKey: .thoughtTimeMilliseconds)
        try container.encodeIfPresent(thoughtTokens, forKey: .thoughtTokens)
        try container.encodeIfPresent(thoughtTokensEstimated, forKey: .thoughtTokensEstimated)
        try container.encodeIfPresent(completedAt, forKey: .completedAt)
        try container.encodeIfPresent(turn, forKey: .turn)
    }
}

public struct WireMessageTool: Codable, Equatable, Sendable {
    public var callID: String? = nil
    public var name: String
    public var detail: String?
    public var success: Bool
    public var pending: Bool
    enum CodingKeys: String, CodingKey {
        case name, detail, success, pending
        case callID = "call_id"
    }
}

public struct WireHistoryPage: Codable, Equatable, Sendable {
    public var historyRevision: String
    public var messages: [WireMessage]
    public var nextCursor: String?

    enum CodingKeys: String, CodingKey {
        case historyRevision = "history_revision"
        case messages
        case nextCursor = "next_cursor"
    }
}

public struct WireQuestion: Codable, Equatable, Sendable {
    public var questionID: String
    public var header: String
    public var text: String
    public var options: [WireQuestionOption]
    public var multiple: Bool
    public var position: UInt32
    public var chainLength: UInt32

    enum CodingKeys: String, CodingKey {
        case questionID = "question_id"
        case header
        case text
        case options
        case multiple
        case position
        case chainLength = "chain_length"
    }
}

public struct WireQuestionOption: Codable, Equatable, Sendable {
    public var label: String
    public var description: String?
}

public struct WireApprovalBatch: Codable, Equatable, Sendable {
    public var batchID: String
    public var actions: [WireApprovalAction]

    enum CodingKeys: String, CodingKey {
        case batchID = "batch_id"
        case actions
    }
}

public struct WireApprovalAction: Codable, Equatable, Sendable {
    public var toolName: String
    public var summary: String
    public var risk: String
    public var details: WireBoundedText

    enum CodingKeys: String, CodingKey {
        case toolName = "tool_name"
        case summary
        case risk
        case details
    }
}

public struct WirePendingPrompt: Codable, Equatable, Sendable {
    public var kind: WireOpen<WirePendingPromptKind>
    public var text: WireBoundedText
}

public enum WirePendingPromptKind: String, Sendable, Equatable, CaseIterable {
    case steer
    case queue
}

public struct WireSessionSettings: Codable, Equatable, Sendable {
    public struct Model: Codable, Equatable, Sendable {
        public var id: String
        public var model: String
        public var reasoningEfforts: [String]
        enum CodingKeys: String, CodingKey { case id, model; case reasoningEfforts = "reasoning_efforts" }
    }
    public var models: [Model]
    public var selectedModel: String
    public var reasoningEffort: String
    public var canChange: Bool
    enum CodingKeys: String, CodingKey {
        case models
        case selectedModel = "selected_model"
        case reasoningEffort = "reasoning_effort"
        case canChange = "can_change"
    }
    var domain: SessionSettings {
        SessionSettings(models: models.map { .init(id: $0.id, model: $0.model, reasoningEfforts: $0.reasoningEfforts) }, selectedModel: selectedModel, reasoningEffort: reasoningEffort, canChange: canChange)
    }
}
