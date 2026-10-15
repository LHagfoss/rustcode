import Foundation

/// The ACP version this client implements. The engine echoes the requested
/// version, so the client still checks the answer.
public let acpProtocolVersion = 1

/// Version of the `_meta.rustcode` question payload this client understands
/// (`engine/src/acp/questions.rs`, `QUESTION_META_VERSION`).
public let rustcodeQuestionMetaVersion = 2

public enum ACPMethod {
    public static let initialize = "initialize"
    public static let sessionNew = "session/new"
    public static let sessionLoad = "session/load"
    public static let sessionList = "session/list"
    public static let sessionClose = "session/close"
    public static let sessionPrompt = "session/prompt"
    public static let sessionCancel = "session/cancel"
    public static let sessionSetConfigOption = "session/set_config_option"
    public static let sessionUpdate = "session/update"
    public static let requestPermission = "session/request_permission"
}

public struct AgentInfo: Equatable, Sendable {
    public var protocolVersion: Int
    public var loadSession: Bool
    public var listSessions: Bool
    public var closeSession: Bool

    public init(initializeResult result: JSONValue) throws {
        guard let version = result["protocolVersion"]?.intValue else {
            throw ACPProtocolError.malformed("initialize response has no protocolVersion")
        }
        guard version == acpProtocolVersion else {
            throw ACPProtocolError.unsupportedVersion(version)
        }
        protocolVersion = version
        let capabilities = result["agentCapabilities"]
        loadSession = capabilities?["loadSession"]?.boolValue ?? false
        listSessions = capabilities?["sessionCapabilities"]?["list"] != nil
        closeSession = capabilities?["sessionCapabilities"]?["close"] != nil
    }
}

public enum ACPProtocolError: Error, Equatable, CustomStringConvertible {
    case unsupportedVersion(Int)
    case malformed(String)

    public var description: String {
        switch self {
        case .unsupportedVersion(let version):
            return "The engine speaks ACP version \(version); this app requires version "
                + "\(acpProtocolVersion). Update RustCode so the app and engine match."
        case .malformed(let detail):
            return "The engine sent an unexpected response: \(detail)"
        }
    }
}

public enum ACPRequests {
    public static func initialize(appVersion: String) -> JSONValue {
        [
            "protocolVersion": .number(Double(acpProtocolVersion)),
            // The app offers no client-side filesystem or terminal: the engine
            // uses its own tools, under its own sandbox and approval policy.
            "clientCapabilities": [
                "fs": ["readTextFile": false, "writeTextFile": false],
                "terminal": false,
            ],
            "clientInfo": ["name": "rustcode-macos", "title": "RustCode", "version": .string(appVersion)],
        ]
    }

    public static func newSession(cwd: URL) -> JSONValue {
        ["cwd": .string(cwd.path), "mcpServers": []]
    }

    public static func loadSession(id: String, cwd: URL) -> JSONValue {
        ["sessionId": .string(id), "cwd": .string(cwd.path), "mcpServers": []]
    }

    public static func listSessions(cwd: URL) -> JSONValue {
        ["cwd": .string(cwd.path)]
    }

    public static func prompt(sessionId: String, text: String) -> JSONValue {
        ["sessionId": .string(sessionId), "prompt": [["type": "text", "text": .string(text)]]]
    }

    public static func session(_ sessionId: String) -> JSONValue {
        ["sessionId": .string(sessionId)]
    }

    public static func setModel(sessionId: String, value: String) -> JSONValue {
        ["sessionId": .string(sessionId), "configId": "model", "value": .string(value)]
    }

    public static func permissionSelected(optionId: String) -> JSONValue {
        ["outcome": ["outcome": "selected", "optionId": .string(optionId)]]
    }

    /// Answer a question with several of its options. ACP needs one selected
    /// option; the engine reads the full answer from `_meta.rustcode.answer`.
    public static func questionOptions(_ options: [PermissionOption]) -> JSONValue {
        [
            "outcome": ["outcome": "selected", "optionId": .string(options.first?.id ?? "")],
            "_meta": ["rustcode": ["answer": ["options": .array(options.map { .string($0.name) })]]],
        ]
    }

    /// Answer a question with free text instead of one of its options.
    public static func questionText(_ text: String, carrier: PermissionOption) -> JSONValue {
        [
            "outcome": ["outcome": "selected", "optionId": .string(carrier.id)],
            "_meta": ["rustcode": ["answer": ["text": .string(text)]]],
        ]
    }

    /// ACP requires every pending permission request to be answered with
    /// `cancelled` when the client cancels the turn.
    public static let permissionCancelled: JSONValue = ["outcome": ["outcome": "cancelled"]]
}

public struct SessionSummary: Identifiable, Equatable, Sendable {
    public var id: String
    public var cwd: String
    public var title: String?
    public var updatedAt: Date?

    public static func list(from result: JSONValue) -> [SessionSummary] {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        let plain = ISO8601DateFormatter()
        return (result["sessions"]?.arrayValue ?? []).compactMap { entry in
            guard let id = entry["sessionId"]?.stringValue else { return nil }
            let stamp = entry["updatedAt"]?.stringValue
            return SessionSummary(
                id: id,
                cwd: entry["cwd"]?.stringValue ?? "",
                title: entry["title"]?.stringValue,
                updatedAt: stamp.flatMap { formatter.date(from: $0) ?? plain.date(from: $0) })
        }
    }
}

public struct ModelChoice: Identifiable, Equatable, Sendable {
    public var id: String
    public var name: String
    public var detail: String?
}

public struct ModelOptions: Equatable, Sendable {
    public var current: String?
    public var choices: [ModelChoice]

    public static let empty = ModelOptions(current: nil, choices: [])

    /// Reads the `model` select from a `configOptions` array.
    public init(configOptions: JSONValue?) {
        let model = configOptions?.arrayValue?.first { $0["id"]?.stringValue == "model" }
        current = model?["currentValue"]?.stringValue
        choices = (model?["options"]?.arrayValue ?? []).compactMap { option in
            guard let value = option["value"]?.stringValue else { return nil }
            return ModelChoice(
                id: value, name: option["name"]?.stringValue ?? value,
                detail: option["description"]?.stringValue)
        }
    }

    public init(current: String?, choices: [ModelChoice]) {
        self.current = current
        self.choices = choices
    }
}

public enum ToolStatus: String, Equatable, Sendable {
    case pending, inProgress = "in_progress", completed, failed

    public var isTerminal: Bool { self == .completed || self == .failed }
}

/// One `session/update` payload, reduced to what the transcript renders.
public enum SessionUpdate: Equatable, Sendable {
    case userText(String)
    case agentText(String)
    case thoughtText(String)
    case toolCall(id: String, title: String, kind: String?, status: ToolStatus?, input: JSONValue?)
    case toolCallUpdate(id: String, title: String?, status: ToolStatus?, output: String?)
    case title(String)
    case configOptions(JSONValue)
    case unknown(String)

    public init(notificationParams params: JSONValue) {
        let update = params["update"] ?? .null
        let kind = update["sessionUpdate"]?.stringValue ?? ""
        switch kind {
        case "user_message_chunk":
            self = .userText(Self.text(update["content"]))
        case "agent_message_chunk":
            self = .agentText(Self.text(update["content"]))
        case "agent_thought_chunk":
            self = .thoughtText(Self.text(update["content"]))
        case "tool_call":
            self = .toolCall(
                id: update["toolCallId"]?.stringValue ?? UUID().uuidString,
                title: update["title"]?.stringValue ?? "Tool",
                kind: update["kind"]?.stringValue,
                status: update["status"]?.stringValue.flatMap(ToolStatus.init(rawValue:)),
                input: update["rawInput"])
        case "tool_call_update":
            self = .toolCallUpdate(
                id: update["toolCallId"]?.stringValue ?? "",
                title: update["title"]?.stringValue,
                status: update["status"]?.stringValue.flatMap(ToolStatus.init(rawValue:)),
                output: Self.toolOutput(update))
        case "session_info_update":
            if let title = update["title"]?.stringValue {
                self = .title(title)
            } else {
                self = .unknown(kind)
            }
        case "config_option_update":
            self = .configOptions(update["configOptions"] ?? .null)
        default:
            self = .unknown(kind)
        }
    }

    static func text(_ content: JSONValue?) -> String {
        guard let content else { return "" }
        if content["type"]?.stringValue == "text" { return content["text"]?.stringValue ?? "" }
        if let resource = content["resource"], let text = resource["text"]?.stringValue {
            return text
        }
        return ""
    }

    static func toolOutput(_ update: JSONValue) -> String? {
        if let content = update["rawOutput"]?["content"]?.stringValue { return content }
        let blocks = (update["content"]?.arrayValue ?? []).map { text($0["content"]) }
            .filter { !$0.isEmpty }
        return blocks.isEmpty ? nil : blocks.joined(separator: "\n")
    }
}

public struct PermissionOption: Identifiable, Equatable, Sendable {
    public enum Kind: String, Sendable {
        case allowOnce = "allow_once", allowAlways = "allow_always"
        case rejectOnce = "reject_once", rejectAlways = "reject_always"

        public var isAllow: Bool { self == .allowOnce || self == .allowAlways }
    }

    public var id: String
    public var name: String
    public var kind: Kind?
}

/// A `session/request_permission` from the engine: either a tool approval or
/// (tagged with `_meta.rustcode.kind == "question"`) an `ask_question`.
public struct PermissionRequest: Identifiable, Equatable, Sendable {
    public struct Question: Equatable, Sendable {
        public var header: String
        public var text: String
        public var descriptions: [String]
        public var multiple: Bool
    }

    /// JSON-RPC id of the request; the reply must echo it.
    public var rpcId: JSONValue
    public var sessionId: String
    public var toolCallId: String
    public var title: String
    public var input: JSONValue?
    public var options: [PermissionOption]
    public var question: Question?

    public var id: String { toolCallId + "#" + (rpcId.intValue.map(String.init) ?? "\(rpcId)") }

    public init?(rpcId: JSONValue, params: JSONValue) {
        guard let sessionId = params["sessionId"]?.stringValue,
              let toolCall = params["toolCall"],
              let options = params["options"]?.arrayValue
        else { return nil }
        self.rpcId = rpcId
        self.sessionId = sessionId
        toolCallId = toolCall["toolCallId"]?.stringValue ?? ""
        title = toolCall["title"]?.stringValue ?? "Tool call"
        input = toolCall["rawInput"]
        self.options = options.compactMap { option in
            guard let id = option["optionId"]?.stringValue else { return nil }
            return PermissionOption(
                id: id, name: option["name"]?.stringValue ?? id,
                kind: option["kind"]?.stringValue.flatMap(PermissionOption.Kind.init(rawValue:)))
        }
        let meta = params["_meta"]?["rustcode"]
        if meta?["kind"]?.stringValue == "question",
           meta?["version"]?.intValue == rustcodeQuestionMetaVersion
        {
            question = Question(
                header: input?["header"]?.stringValue ?? "",
                text: input?["question"]?.stringValue ?? title,
                descriptions: (input?["descriptions"]?.arrayValue ?? []).map {
                    $0.stringValue ?? ""
                },
                multiple: input?["multiple"]?.boolValue ?? false)
        } else {
            question = nil
        }
    }

    /// Answer options of a question (everything but the reject option).
    public var answerOptions: [PermissionOption] {
        options.filter { $0.kind?.isAllow ?? true }
    }

    public var rejectOption: PermissionOption? {
        options.first { $0.kind == .rejectOnce || $0.kind == .rejectAlways }
    }
}

public enum StopReason: String, Sendable {
    case endTurn = "end_turn", maxTokens = "max_tokens"
    case maxTurnRequests = "max_turn_requests", refusal, cancelled
}
