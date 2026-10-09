import Foundation

// DTOs for `docs/remote-protocol/v1/request.schema.json` in the RustCode repository.
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
    case listSessions
    case subscribeSessions
    case attachSession(AttachSession)
    case detachSession
    case getHistory(GetHistory)
    case getContent(GetContent)
    case submitPrompt(Prompt)
    case steer(Prompt)
    case queue(Prompt)
    case cancelTurn(CancelTurn)
    case answerQuestion(AnswerQuestion)
    case resolveApproval(ResolveApproval)
    case getRequestStatus(GetRequestStatus)
    case unknown(type: String, JSONValue)

    /// Every `operation.type` this build knows.
    public static let knownTypes: Set<String> = [
        "list_sessions", "subscribe_sessions", "attach_session", "detach_session", "get_history",
        "get_content", "submit_prompt", "steer", "queue", "cancel_turn", "answer_question",
        "resolve_approval", "get_request_status",
    ]

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

    public var type: String {
        switch self {
        case .listSessions: "list_sessions"
        case .subscribeSessions: "subscribe_sessions"
        case .attachSession: "attach_session"
        case .detachSession: "detach_session"
        case .getHistory: "get_history"
        case .getContent: "get_content"
        case .submitPrompt: "submit_prompt"
        case .steer: "steer"
        case .queue: "queue"
        case .cancelTurn: "cancel_turn"
        case .answerQuestion: "answer_question"
        case .resolveApproval: "resolve_approval"
        case .getRequestStatus: "get_request_status"
        case .unknown(let type, _): type
        }
    }

    public init(from decoder: any Decoder) throws {
        let type = try WireTag.read("type", from: decoder)
        switch type {
        case "list_sessions": self = .listSessions
        case "subscribe_sessions": self = .subscribeSessions
        case "attach_session": self = .attachSession(try AttachSession(from: decoder))
        case "detach_session": self = .detachSession
        case "get_history": self = .getHistory(try GetHistory(from: decoder))
        case "get_content": self = .getContent(try GetContent(from: decoder))
        case "submit_prompt": self = .submitPrompt(try Prompt(from: decoder))
        case "steer": self = .steer(try Prompt(from: decoder))
        case "queue": self = .queue(try Prompt(from: decoder))
        case "cancel_turn": self = .cancelTurn(try CancelTurn(from: decoder))
        case "answer_question": self = .answerQuestion(try AnswerQuestion(from: decoder))
        case "resolve_approval": self = .resolveApproval(try ResolveApproval(from: decoder))
        case "get_request_status": self = .getRequestStatus(try GetRequestStatus(from: decoder))
        default: self = .unknown(type: type, try JSONValue(from: decoder))
        }
    }

    public func encode(to encoder: any Encoder) throws {
        let payload: (any Encodable)? = switch self {
        case .listSessions, .subscribeSessions, .detachSession: nil
        case .attachSession(let payload): payload
        case .getHistory(let payload): payload
        case .getContent(let payload): payload
        case .submitPrompt(let payload), .steer(let payload), .queue(let payload): payload
        case .cancelTurn(let payload): payload
        case .answerQuestion(let payload): payload
        case .resolveApproval(let payload): payload
        case .getRequestStatus(let payload): payload
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
