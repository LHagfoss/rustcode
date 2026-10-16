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
    /// The engine takes image blocks in `session/prompt`.
    public var promptImages: Bool

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
        promptImages = capabilities?["promptCapabilities"]?["image"]?.boolValue ?? false
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

    /// One page of `session/list`: a project's sessions, or every project's
    /// when `cwd` is nil.
    public static func listSessions(cwd: URL?, cursor: String? = nil) -> JSONValue {
        var params: [String: JSONValue] = [:]
        if let cwd { params["cwd"] = .string(cwd.path) }
        if let cursor { params["cursor"] = .string(cursor) }
        return .object(params)
    }

    /// The text block, then one image block per attachment in order.
    public static func prompt(sessionId: String, text: String, images: [ImageAttachment] = []) -> JSONValue {
        var blocks: [JSONValue] = text.isEmpty && !images.isEmpty ? [] : [["type": "text", "text": .string(text)]]
        blocks += images.map(\.block)
        return ["sessionId": .string(sessionId), "prompt": .array(blocks)]
    }

    public static func session(_ sessionId: String) -> JSONValue {
        ["sessionId": .string(sessionId)]
    }

    public static func setModel(sessionId: String, value: String) -> JSONValue {
        setConfig(sessionId: sessionId, configId: "model", value: value)
    }

    public static func setConfig(sessionId: String, configId: String, value: String) -> JSONValue {
        ["sessionId": .string(sessionId), "configId": .string(configId), "value": .string(value)]
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

    /// The cursor for the next page, when the list has more.
    public static func nextCursor(from result: JSONValue) -> String? {
        result["nextCursor"]?.stringValue
    }

    /// The project folder's name, shown beside sessions from every project.
    public var projectName: String {
        cwd.isEmpty ? "" : URL(fileURLWithPath: cwd).lastPathComponent
    }

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
                title: entry["title"]?.stringValue.map(ImageMarkers.title),
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

    /// The reasoning effort select, when the active model offers a choice.
    public var effort: String?
    public var efforts: [ModelChoice] = []

    /// Reads the `model` and `effort` selects from a `configOptions` array.
    public init(configOptions: JSONValue?) {
        func select(_ id: String) -> (String?, [ModelChoice]) {
            let option = configOptions?.arrayValue?.first { $0["id"]?.stringValue == id }
            let choices: [ModelChoice] = (option?["options"]?.arrayValue ?? []).compactMap { option in
                guard let value = option["value"]?.stringValue else { return nil }
                return ModelChoice(
                    id: value, name: option["name"]?.stringValue ?? value,
                    detail: option["description"]?.stringValue)
            }
            return (option?["currentValue"]?.stringValue, choices)
        }
        (current, choices) = select("model")
        (effort, efforts) = select("effort")
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
    case toolCallUpdate(id: String, title: String?, status: ToolStatus?, output: String?, diffs: [FileDiff] = [])
    case title(String)
    case configOptions(JSONValue)
    case availableCommands([SlashCommand])
    /// Context and token usage (`usage_update`).
    case usage(SessionUsage)
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
                output: Self.toolOutput(update),
                diffs: FileDiff.parse(content: update["content"]))
        case "session_info_update":
            if let title = update["title"]?.stringValue {
                self = .title(title)
            } else {
                self = .unknown(kind)
            }
        case "available_commands_update":
            self = .availableCommands((update["availableCommands"]?.arrayValue ?? []).compactMap(SlashCommand.init(json:)))
        case "config_option_update":
            self = .configOptions(update["configOptions"] ?? .null)
        case "usage_update":
            self = SessionUsage(json: update).map(Self.usage) ?? .unknown(kind)
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

/// A command the engine advertises with `available_commands_update`. Host
/// commands run when sent as a prompt; the others are app actions.
public struct SlashCommand: Identifiable, Equatable, Sendable {
    public enum Action: String, Sendable {
        case execute, newSession = "new_session", modelPicker = "model_picker"
        case sessionPicker = "session_picker", copyResponse = "copy_response"
    }

    /// With the leading slash, as typed.
    public var name: String
    public var description: String
    public var action: Action
    public var takesArguments: Bool

    public var id: String { name }

    public init(name: String, description: String, action: Action = .execute, takesArguments: Bool = false) {
        self.name = name
        self.description = description
        self.action = action
        self.takesArguments = takesArguments
    }

    init?(json: JSONValue) {
        guard let name = json["name"]?.stringValue, !name.isEmpty else { return nil }
        self.name = name.hasPrefix("/") ? name : "/" + name
        description = json["description"]?.stringValue ?? ""
        action = json["_meta"]?["rustcode"]?["action"]?.stringValue.flatMap(Action.init(rawValue:)) ?? .execute
        takesArguments = json["input"]?["hint"] != nil
    }

    /// What completing the command puts in the composer.
    public var insertion: String { takesArguments ? name + " " : name }

    /// Commands for a draft still typing its first word, prefix matches first.
    public static func suggestions(for draft: String, in commands: [SlashCommand]) -> [SlashCommand] {
        guard draft.hasPrefix("/"), !draft.contains(where: \.isWhitespace) else { return [] }
        let typed = draft.lowercased()
        let prefixed = commands.filter { $0.name.lowercased().hasPrefix(typed) }
        let others = commands.filter { !prefixed.contains($0) && $0.name.lowercased().contains(typed.dropFirst()) && typed.count > 1 }
        return prefixed + others
    }

    /// The advertised command a draft invokes, if it is one.
    public static func invoked(by draft: String, in commands: [SlashCommand]) -> SlashCommand? {
        let name = draft.trimmingCharacters(in: .whitespaces).split(separator: " ").first.map(String.init)?.lowercased()
        return commands.first { $0.name.lowercased() == name }
    }
}
