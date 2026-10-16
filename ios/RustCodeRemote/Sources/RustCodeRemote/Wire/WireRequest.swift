import Foundation

// DTOs for `docs/remote-protocol/v1/request.schema.json` at the repository root.
// The Rust types in `rustcode/engine/src/remote/protocol.rs` are the source of truth.

public struct WireRequest: Codable, Equatable, Sendable {
    public var protocolVersion: UInt32
    public var requestID: String
    public var sessionID: String?
    public var registrationEpoch: UInt64?
    public var operation: WireOperation

    public init(
        protocolVersion: UInt32 = remoteProtocolVersion,
        requestID: String,
        sessionID: String? = nil,
        registrationEpoch: UInt64? = nil,
        operation: WireOperation
    ) {
        self.protocolVersion = protocolVersion
        self.requestID = requestID
        self.sessionID = sessionID
        self.registrationEpoch = registrationEpoch
        self.operation = operation
    }

    enum CodingKeys: String, CodingKey {
        case protocolVersion = "protocol_version"
        case requestID = "request_id"
        case sessionID = "session_id"
        case registrationEpoch = "registration_epoch"
        case operation
    }
}

public enum WireOperation: Codable, Equatable, Sendable {
    case uploadImage(ImageUploadChunk)
    case listSessions
    case subscribeSessions
    case listDirectories(DirectoryPath)
    case createSession(DirectoryPath)
    case attachSession(AttachSession)
    case detachSession
    case getHistory(GetHistory)
    case getContent(GetContent)
    case executeCommand(ExecuteCommand)
    case cancelQuestion(CancelQuestion)
    case setSessionSettings(SetSessionSettings)
    case submitPrompt(Prompt)
    case steer(Prompt)
    case queue(Prompt)
    case cancelTurn(CancelTurn)
    case answerQuestion(AnswerQuestion)
    case resolveApproval(ResolveApproval)
    case getRequestStatus(GetRequestStatus)
    case requestShare(RequestShare)
    case listSavedSessions(ListSavedSessions)
    case resumeSession(ResumeSession)
    case unknown(type: String, JSONValue)

    /// Every `operation.type` this build knows.
    public static let knownTypes: Set<String> = [
        "upload_image", "list_directories", "create_session", "list_sessions", "subscribe_sessions", "attach_session", "detach_session", "get_history",
        "get_content", "submit_prompt", "steer", "queue", "cancel_turn", "answer_question",
        "resolve_approval", "get_request_status", "set_session_settings", "execute_command", "cancel_question",
        "request_share", "list_saved_sessions", "resume_session",
    ]

    public struct DirectoryPath: Codable, Equatable, Sendable { public var path: String? }

    public struct AttachSession: Codable, Equatable, Sendable {
        public var resume: WireResumeCursor?
    }

    public struct GetHistory: Codable, Equatable, Sendable {
        public var cursor: String?
        public var limit: UInt32
    }

    public struct GetContent: Codable, Equatable, Sendable {
        public var contentID: String
        public var offset: UInt64
        public var maxBytes: UInt32

        enum CodingKeys: String, CodingKey {
            case contentID = "content_id"
            case offset
            case maxBytes = "max_bytes"
        }
    }

    public struct ExecuteCommand: Codable, Equatable, Sendable { public var command: String }
    public struct CancelQuestion: Codable, Equatable, Sendable {
        public var questionID: String
        enum CodingKeys: String, CodingKey { case questionID = "question_id" }
    }
    public struct SetSessionSettings: Codable, Equatable, Sendable {
        public var model: String
        public var reasoningEffort: String
        enum CodingKeys: String, CodingKey { case model; case reasoningEffort = "reasoning_effort" }
    }

    public struct Prompt: Codable, Equatable, Sendable {
        public var prompt: String
    }

    public struct CancelTurn: Codable, Equatable, Sendable {
        public var turnID: String

        enum CodingKeys: String, CodingKey {
            case turnID = "turn_id"
        }
    }

    public struct AnswerQuestion: Codable, Equatable, Sendable {
        public var questionID: String
        public var answer: WireAnswer

        enum CodingKeys: String, CodingKey {
            case questionID = "question_id"
            case answer
        }
    }

    public struct ResolveApproval: Codable, Equatable, Sendable {
        public var batchID: String
        public var choice: WireOpen<WireApprovalChoice>

        enum CodingKeys: String, CodingKey {
            case batchID = "batch_id"
            case choice
        }
    }

    public struct GetRequestStatus: Codable, Equatable, Sendable {
        public var targetRequestID: String

        enum CodingKeys: String, CodingKey {
            case targetRequestID = "target_request_id"
        }
    }

    /// Asks the live terminal of an unshared session to share it.
    public struct RequestShare: Codable, Equatable, Sendable {
        public var sessionID: String

        enum CodingKeys: String, CodingKey {
            case sessionID = "session_id"
        }
    }

    /// One page of the host's saved sessions.
    public struct ListSavedSessions: Codable, Equatable, Sendable {
        public var cursor: String?
        public var limit: UInt32?
        public var query: String?
    }

    /// Reopens a saved session that nothing holds.
    public struct ResumeSession: Codable, Equatable, Sendable {
        public var sessionID: String

        enum CodingKeys: String, CodingKey {
            case sessionID = "session_id"
        }
    }

    public var type: String {
        switch self {
        case .uploadImage: "upload_image"
        case .listSessions: "list_sessions"
        case .subscribeSessions: "subscribe_sessions"
        case .listDirectories: "list_directories"
        case .createSession: "create_session"
        case .attachSession: "attach_session"
        case .detachSession: "detach_session"
        case .getHistory: "get_history"
        case .getContent: "get_content"
        case .executeCommand: "execute_command"
        case .cancelQuestion: "cancel_question"
        case .setSessionSettings: "set_session_settings"
        case .submitPrompt: "submit_prompt"
        case .steer: "steer"
        case .queue: "queue"
        case .cancelTurn: "cancel_turn"
        case .answerQuestion: "answer_question"
        case .resolveApproval: "resolve_approval"
        case .getRequestStatus: "get_request_status"
        case .requestShare: "request_share"
        case .listSavedSessions: "list_saved_sessions"
        case .resumeSession: "resume_session"
        case .unknown(let type, _): type
        }
    }

    public init(from decoder: any Decoder) throws {
        let type = try WireTag.read("type", from: decoder)
        switch type {
        case "upload_image": self = .uploadImage(try ImageUploadChunk(from: decoder))
        case "list_sessions": self = .listSessions
        case "subscribe_sessions": self = .subscribeSessions
        case "list_directories": self = .listDirectories(try DirectoryPath(from: decoder))
        case "create_session": self = .createSession(try DirectoryPath(from: decoder))
        case "attach_session": self = .attachSession(try AttachSession(from: decoder))
        case "detach_session": self = .detachSession
        case "get_history": self = .getHistory(try GetHistory(from: decoder))
        case "get_content": self = .getContent(try GetContent(from: decoder))
        case "execute_command": self = .executeCommand(try ExecuteCommand(from: decoder))
        case "cancel_question": self = .cancelQuestion(try CancelQuestion(from: decoder))
        case "set_session_settings": self = .setSessionSettings(try SetSessionSettings(from: decoder))
        case "submit_prompt": self = .submitPrompt(try Prompt(from: decoder))
        case "steer": self = .steer(try Prompt(from: decoder))
        case "queue": self = .queue(try Prompt(from: decoder))
        case "cancel_turn": self = .cancelTurn(try CancelTurn(from: decoder))
        case "answer_question": self = .answerQuestion(try AnswerQuestion(from: decoder))
        case "resolve_approval": self = .resolveApproval(try ResolveApproval(from: decoder))
        case "get_request_status": self = .getRequestStatus(try GetRequestStatus(from: decoder))
        case "request_share": self = .requestShare(try RequestShare(from: decoder))
        case "list_saved_sessions": self = .listSavedSessions(try ListSavedSessions(from: decoder))
        case "resume_session": self = .resumeSession(try ResumeSession(from: decoder))
        default: self = .unknown(type: type, try JSONValue(from: decoder))
        }
    }

    public func encode(to encoder: any Encoder) throws {
        let payload: (any Encodable)? = switch self {
        case .uploadImage(let payload): payload
        case .listSessions, .subscribeSessions, .detachSession: nil
        case .listDirectories(let payload), .createSession(let payload): payload
        case .attachSession(let payload): payload
        case .getHistory(let payload): payload
        case .getContent(let payload): payload
        case .executeCommand(let payload): payload
        case .cancelQuestion(let payload): payload
        case .setSessionSettings(let payload): payload
        case .submitPrompt(let payload), .steer(let payload), .queue(let payload): payload
        case .cancelTurn(let payload): payload
        case .answerQuestion(let payload): payload
        case .resolveApproval(let payload): payload
        case .getRequestStatus(let payload): payload
        case .requestShare(let payload): payload
        case .listSavedSessions(let payload): payload
        case .resumeSession(let payload): payload
        case .unknown(_, let raw): raw
        }
        if case .unknown(_, let raw) = self {
            try raw.encode(to: encoder)
        } else {
            try WireTag.write(type, as: "type", payload: payload, to: encoder)
        }
    }
}

public struct WireResumeCursor: Codable, Equatable, Sendable {
    public var snapshotID: String? = nil
    public var gatewayID: String
    /// The gateway run the sequence numbers came from. The gateway uses it to decide
    /// between a replay and a fresh snapshot.
    public var instanceID: String?
    public var lastSequence: UInt64

    enum CodingKeys: String, CodingKey {
        case gatewayID = "gateway_id"
        case instanceID = "instance_id"
        case snapshotID = "snapshot_id"
        case lastSequence = "last_sequence"
    }
}

public enum WireApprovalChoice: String, Sendable, Equatable, CaseIterable {
    case approve
    case deny
}

public enum WireAnswer: Codable, Equatable, Sendable {
    case selected(Selected)
    case custom(Custom)
    case unknown(type: String, JSONValue)

    public static let knownTypes: Set<String> = ["selected", "custom"]

    public struct Selected: Codable, Equatable, Sendable {
        /// Option labels, as the question listed them.
        public var options: [String]
    }

    public struct Custom: Codable, Equatable, Sendable {
        public var text: String
    }

    public init(from decoder: any Decoder) throws {
        let type = try WireTag.read("type", from: decoder)
        switch type {
        case "selected": self = .selected(try Selected(from: decoder))
        case "custom": self = .custom(try Custom(from: decoder))
        default: self = .unknown(type: type, try JSONValue(from: decoder))
        }
    }

    public func encode(to encoder: any Encoder) throws {
        switch self {
        case .selected(let payload): try WireTag.write("selected", as: "type", payload: payload, to: encoder)
        case .custom(let payload): try WireTag.write("custom", as: "type", payload: payload, to: encoder)
        case .unknown(_, let raw): try raw.encode(to: encoder)
        }
    }
}
