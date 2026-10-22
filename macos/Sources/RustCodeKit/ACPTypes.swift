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
    public static let remote = "_rustcode/remote"
    public static let remoteStart = "_rustcode/remote/start"
    public static let remotePair = "_rustcode/remote/pair"
    public static let remoteStop = "_rustcode/remote/stop"
    /// Hands a running turn a message to read after its current step.
    public static let steer = "_rustcode/steer"
    /// Lists the chat's subagents, reads one's conversation, sends it more
    /// instructions or cancels it.
    public static let agents = "_rustcode/agents"
    /// Takes a chat back to just before one of its messages.
    public static let rewind = "_rustcode/rewind"
}

public struct AgentInfo: Equatable, Sendable {
    public var protocolVersion: Int
    public var loadSession: Bool
    public var listSessions: Bool
    public var closeSession: Bool
    /// The engine takes image blocks in `session/prompt`.
    public var promptImages: Bool
    /// The engine implements the `_rustcode/remote*` sharing bridge.
    public var remote: Bool
    /// The engine takes `_rustcode/steer` for a running turn.
    public var steer: Bool
    /// The engine answers `_rustcode/agents`.
    public var agents: Bool
    /// The engine answers `_rustcode/rewind`.
    public var rewind: Bool

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
        remote = capabilities?["_meta"]?["rustcode"]?["remote"]?.intValue != nil
        steer = capabilities?["_meta"]?["rustcode"]?["steer"]?.intValue != nil
        agents = capabilities?["_meta"]?["rustcode"]?["agents"]?.intValue != nil
        rewind = capabilities?["_meta"]?["rustcode"]?["rewind"]?.intValue != nil
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
                // The transcript draws `rustcode-viz` blocks natively, so the
                // engine may teach the agent the format (docs/visualizations.md).
                // The localhost preview answers `_rustcode/preview` requests,
                // so the engine offers `localhost_preview` (docs/preview.md).
                // Presence asks the engine for Discord Rich Presence, which
                // it gives only clients that ask (docs/discord-rich-presence.md).
                "_meta": ["rustcode": [
                    "visualizations": .number(Double(Visualization.capabilityLevel)),
                    "preview": .number(Double(rustcodePreviewCapabilityVersion)),
                    "presence": .number(Double(rustcodePresenceCapabilityVersion)),
                ]],
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

    public static func rewind(sessionId: String, userIndex: Int, text: String) -> JSONValue {
        ["sessionId": .string(sessionId), "userIndex": .number(Double(userIndex)), "text": .string(text)]
    }

    public static func steer(sessionId: String, text: String) -> JSONValue {
        ["sessionId": .string(sessionId), "text": .string(text)]
    }

    /// Steers a finished turn never read, from its `session/prompt` answer.
    public static func unusedSteers(in result: JSONValue) -> [String] {
        (result["_meta"]?["rustcode"]?["unusedSteers"]?.arrayValue ?? []).compactMap(\.stringValue)
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

    /// The current share and pairing offer for a chat.
    public static func remoteStatus(sessionId: String) -> JSONValue {
        ["sessionId": .string(sessionId)]
    }

    /// Share a chat and, when asked, issue a pairing offer as soon as the
    /// gateway is connected.
    public static func remoteStart(sessionId: String, pair: Bool = true) -> JSONValue {
        ["sessionId": .string(sessionId), "pair": .bool(pair)]
    }

    /// Replace the current pairing offer with a fresh one.
    public static func remotePair(sessionId: String) -> JSONValue {
        ["sessionId": .string(sessionId)]
    }

    /// Stop sharing and switch remote access off for this Mac.
    public static func remoteStop(sessionId: String) -> JSONValue {
        ["sessionId": .string(sessionId)]
    }
}

public struct SessionSummary: Identifiable, Equatable, Codable, Sendable {
    public var id: String
    public var cwd: String
    public var title: String?
    public var updatedAt: Date?
    /// The model profile the chat last used, when the engine recorded one.
    public var model: String?

    public init(id: String, cwd: String, title: String? = nil, updatedAt: Date? = nil, model: String? = nil) {
        self.id = id
        self.cwd = cwd
        self.title = title
        self.updatedAt = updatedAt
        self.model = model
    }

    /// The row a chat shows from its first message on. The engine lists a
    /// session only once it has saved history, which can be minutes into the
    /// first turn.
    public static func started(id: String, cwd: String, prompt: String, model: String?) -> SessionSummary {
        let line = prompt.split(whereSeparator: \.isNewline).first.map(String.init) ?? ""
        let title = line.count > 80 ? String(line.prefix(79)) + "…" : line
        return SessionSummary(id: id, cwd: cwd, title: title.isEmpty ? nil : title, updatedAt: Date(), model: model)
    }

    /// The engine's list, with every started chat it does not list yet,
    /// latest activity first. Each chat appears once: the sidebar keys its
    /// rows by id, and a repeated id drew two rows that shared one row's
    /// selection and spinner.
    public static func merging(_ listed: [SessionSummary], started: [SessionSummary]) -> [SessionSummary] {
        let known = Set(listed.map(\.id))
        var seen = Set<String>()
        return byActivity((started.filter { !known.contains($0.id) } + listed).filter { seen.insert($0.id).inserted })
    }

    /// Latest activity first. The engine lists chats by when they started, so
    /// its pages stay stable; a chat used since would stay where it began.
    /// Ties and undated chats keep their order.
    public static func byActivity(_ sessions: [SessionSummary]) -> [SessionSummary] {
        sessions.enumerated().sorted { left, right in
            let (a, b) = (left.element.updatedAt ?? .distantPast, right.element.updatedAt ?? .distantPast)
            return a != b ? a > b : left.offset < right.offset
        }.map(\.element)
    }

    /// `provider/model` reads as just the model.
    public var modelName: String? { model.map { $0.split(separator: "/").last.map(String.init) ?? $0 } }

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
                updatedAt: stamp.flatMap { formatter.date(from: $0) ?? plain.date(from: $0) },
                model: entry["_meta"]?["model"]?.stringValue)
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
    /// The `/context` reply: its Markdown text and the breakdown it carries.
    case contextReport(text: String, context: ContextBreakdown)
    case thoughtText(String)
    /// A harness notice, such as why a turn stopped. It is shown as a
    /// toast, never as part of the conversation.
    case notice(String)
    /// `detail` is the engine's one line on what the call acts on.
    case toolCall(id: String, title: String, kind: String?, status: ToolStatus?, input: JSONValue?, detail: String? = nil)
    /// `outputDelta` is output a running call printed since the last update;
    /// `output` is the whole result once it is in.
    case toolCallUpdate(id: String, title: String?, status: ToolStatus?, output: String?, diffs: [FileDiff] = [],
                        outputDelta: String? = nil)
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
            let meta = update["_meta"]?["rustcode"]
            if meta?["kind"]?.stringValue == "context",
               let context = ContextBreakdown(json: meta?["context"]) {
                self = .contextReport(text: Self.text(update["content"]), context: context)
            } else if meta?["kind"]?.stringValue == "notice" {
                self = .notice(Self.text(update["content"]))
            } else {
                self = .agentText(Self.text(update["content"]))
            }
        case "agent_thought_chunk":
            self = .thoughtText(Self.text(update["content"]))
        case "tool_call":
            self = .toolCall(
                id: update["toolCallId"]?.stringValue ?? UUID().uuidString,
                title: update["title"]?.stringValue ?? "Tool",
                kind: update["kind"]?.stringValue,
                status: update["status"]?.stringValue.flatMap(ToolStatus.init(rawValue:)),
                input: update["rawInput"],
                detail: update["_meta"]?["rustcode"]?["detail"]?.stringValue)
        case "tool_call_update":
            self = .toolCallUpdate(
                id: update["toolCallId"]?.stringValue ?? "",
                title: update["title"]?.stringValue,
                status: update["status"]?.stringValue.flatMap(ToolStatus.init(rawValue:)),
                output: Self.toolOutput(update),
                diffs: FileDiff.parse(content: update["content"]),
                outputDelta: update["_meta"]?["rustcode"]?["outputDelta"]?.stringValue)
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

    /// The update is a report command's answer, which can arrive while a
    /// turn streams into the same session.
    public static func isReport(notificationParams params: JSONValue) -> Bool {
        params["update"]?["_meta"]?["rustcode"]?["report"]?.boolValue == true
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

    /// The option auto-approve answers with: allow once, else allow always.
    /// Nil for a question, which always waits for the user.
    public var autoApproveOption: PermissionOption? {
        guard question == nil else { return nil }
        return options.first { $0.kind == .allowOnce } ?? options.first { $0.kind == .allowAlways }
    }

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
        /// Opens the engine's remote pairing panel.
        case remote
        /// Opens the workspace terminal. The app's own; the engine never
        /// advertises or sees it.
        case terminal
        /// Opens the guide to in-chat charts, tables and trees. The app's
        /// own, like `terminal`.
        case visualizations
    }

    /// With the leading slash, as typed.
    public var name: String
    public var description: String
    public var action: Action
    public var takesArguments: Bool
    /// A host command whose bare form only reads the session. The engine
    /// answers it beside a running turn, so it is sent at once, not queued.
    public var report: Bool

    public var id: String { name }

    public init(name: String, description: String, action: Action = .execute, takesArguments: Bool = false,
                report: Bool = false) {
        self.name = name
        self.description = description
        self.action = action
        self.takesArguments = takesArguments
        self.report = report
    }

    init?(json: JSONValue) {
        guard let name = json["name"]?.stringValue, !name.isEmpty else { return nil }
        self.name = name.hasPrefix("/") ? name : "/" + name
        description = json["description"]?.stringValue ?? ""
        action = json["_meta"]?["rustcode"]?["action"]?.stringValue.flatMap(Action.init(rawValue:)) ?? .execute
        takesArguments = json["input"]?["hint"] != nil
        report = action == .execute && json["_meta"]?["rustcode"]?["report"]?.boolValue == true
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

    /// Commands the app adds to the engine's, answered without the engine.
    public static let appCommands = [
        SlashCommand(name: "/terminal", description: "Open a terminal in this project (⇧⌘T)", action: .terminal),
        // Also advertised by the engine, but available before the first
        // session exists so `/remote` opens the panel instead of being sent
        // as a prompt.
        SlashCommand(name: "/remote", description: "Share this chat with a phone or another device",
                     action: .remote),
        SlashCommand(name: "/visualize", description: "Charts, tables and trees you can ask for, with examples",
                     action: .visualizations),
    ]

    /// The engine's commands plus the app's; an engine command of the same
    /// name wins.
    public static func withAppCommands(_ engine: [SlashCommand]) -> [SlashCommand] {
        let names = Set(engine.map { $0.name.lowercased() })
        return engine + appCommands.filter { !names.contains($0.name.lowercased()) }
    }

    /// The app command a draft (or its command chip) invokes, if any. These
    /// work with no engine running.
    public static func appCommand(chip: SlashCommand?, draft: String) -> SlashCommand? {
        if let chip { return appCommands.contains(chip) ? chip : nil }
        return invoked(by: draft, in: appCommands)
    }

    /// How a sent draft runs while its chat may be busy with a turn.
    public enum Dispatch: Equatable, Sendable {
        /// Answered by the app itself, at once.
        case app(SlashCommand)
        /// `/cancel`: stops the chat's running turn, at once.
        case cancel
        /// A bare report: sent to the engine at once, beside a running turn.
        case report(SlashCommand)
        /// A prompt, or a command that changes or needs the session's turn:
        /// it queues behind a running turn.
        case prompt
    }

    /// Classify a sent draft. `plain` is false when files or images ride
    /// along, which makes it a prompt.
    public static func dispatch(_ typed: String, in commands: [SlashCommand], plain: Bool = true) -> Dispatch {
        guard let command = invoked(by: typed, in: commands) else { return .prompt }
        if command.action != .execute { return .app(command) }
        let bare = plain && typed.trimmingCharacters(in: .whitespacesAndNewlines).lowercased() == command.name.lowercased()
        if bare, command.name.lowercased() == "/cancel" { return .cancel }
        if bare, command.report { return .report(command) }
        return .prompt
    }

    /// The advertised command a draft invokes, if it is one.
    public static func invoked(by draft: String, in commands: [SlashCommand]) -> SlashCommand? {
        let name = draft.trimmingCharacters(in: .whitespaces).split(separator: " ").first.map(String.init)?.lowercased()
        return commands.first { $0.name.lowercased() == name }
    }
}
