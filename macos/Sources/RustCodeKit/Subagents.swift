import Foundation

/// A subagent of the shown chat, as `_rustcode/agents` lists it: what the
/// terminal's agent picker shows for each child.
public struct SubagentInfo: Identifiable, Equatable, Sendable {
    public enum Status: String, Equatable, Sendable {
        case queued, running, interrupted, completed, failed, cancelled

        /// Still working or waiting to: the chat keeps polling for it.
        public var isActive: Bool { self == .queued || self == .running }

        public var label: String {
            switch self {
            case .queued: "Queued"
            case .running: "Running"
            case .interrupted: "Interrupted"
            case .completed: "Done"
            case .failed: "Failed"
            case .cancelled: "Cancelled"
            }
        }
    }

    public var id: Int
    public var name: String
    public var task: String
    public var model: String?
    /// `default`, `explorer` or `worker`.
    public var agentType: String
    public var status: Status
    /// A turn is running for this child, as after a follow-up.
    public var activeTurn: Bool
    public var parentId: Int?
    public var depth: Int
    public var writeAccess: Bool
    public var messageCount: Int
    public var startedAt: Date?
    public var finishedAt: Date?
    /// The summary the child returned when it finished.
    public var completion: String?

    public var isActive: Bool { status.isActive || activeTurn }

    public init?(json: JSONValue) {
        guard let id = json["id"]?.intValue, let name = json["name"]?.stringValue else { return nil }
        self.id = id
        self.name = name
        task = json["task"]?.stringValue ?? ""
        model = json["model"]?.stringValue
        agentType = json["agentType"]?.stringValue ?? "default"
        status = json["status"]?.stringValue.flatMap(Status.init(rawValue:)) ?? .queued
        activeTurn = json["activeTurn"]?.boolValue ?? false
        parentId = json["parentId"]?.intValue
        depth = json["depth"]?.intValue ?? 1
        writeAccess = json["writeAccess"]?.boolValue ?? false
        messageCount = json["messageCount"]?.intValue ?? 0
        startedAt = json["startedAtMs"]?.doubleValue.map { Date(timeIntervalSince1970: $0 / 1000) }
        finishedAt = json["finishedAtMs"]?.doubleValue.map { Date(timeIntervalSince1970: $0 / 1000) }
        completion = json["completion"]?.stringValue
    }

    /// The children the sidebar nests under their chat (#2492): those still
    /// working, and the one open on screen, so its row stays while it is read.
    public static func sidebarChildren(_ agents: [SubagentInfo], viewing: Int?) -> [SubagentInfo] {
        agents.filter { $0.isActive || $0.id == viewing }
    }

    /// The children in an `_rustcode/agents` list answer.
    public static func list(from result: JSONValue) -> [SubagentInfo] {
        (result["agents"]?.arrayValue ?? []).compactMap(SubagentInfo.init(json:))
    }

    /// How long the child ran, or has run so far, as `1m 05s`.
    public func elapsed(now: Date = Date()) -> String? {
        guard let startedAt else { return nil }
        let seconds = max(0, Int((finishedAt ?? now).timeIntervalSince(startedAt)))
        return seconds < 60 ? "\(seconds)s" : String(format: "%dm %02ds", seconds / 60, seconds % 60)
    }

    /// A child's conversation from an `agents` history answer, as the chat
    /// would show it after a replay.
    public static func transcript(from result: JSONValue) -> Transcript {
        var transcript = Transcript()
        for update in result["updates"]?.arrayValue ?? [] {
            transcript.apply(SessionUpdate(notificationParams: ["update": update]))
        }
        return transcript
    }
}

extension SubagentInfo {
    /// The child a `spawn_agent` result names: its JSON receipt
    /// (`{"agent_id":3,…}`, notes may follow it), or the `subagent 3 started`
    /// line older sessions recorded. A replayed session sends the receipt as
    /// history stored it, behind `spawn_agent: `.
    public static func spawnedId(in output: String?) -> Int? {
        guard let output else { return nil }
        for pattern in [#"^\s*(?:spawn_agent:\s*)?\{[^}]*"agent_id"\s*:\s*(\d+)"#,
                        #"^\s*(?:spawn_agent:\s*)?subagent (\d+) started"#] {
            if let match = output.firstMatch(of: try! Regex(pattern)),
               let range = match.output[1].range {
                return Int(output[range])
            }
        }
        return nil
    }

    /// What the pill over the composer says while agents work, as
    /// `2 agents working`; nil once none is, so the pill goes (#2327).
    public static func workingLabel(_ agents: [SubagentInfo]) -> String? {
        let working = agents.filter(\.isActive).count
        switch working {
        case 0: return nil
        case 1: return "1 agent working"
        default: return "\(working) agents working"
        }
    }

    /// The first line of the task, for a one-line row.
    public var taskLine: String {
        task.split(whereSeparator: \.isNewline).first.map { String($0).trimmingCharacters(in: .whitespaces) } ?? ""
    }
}

extension ToolItem {
    /// A `spawn_agent` call: the chat draws it as the agent it started.
    public var isAgentSpawn: Bool { title == "spawn_agent" }
}

/// A subagent a turn started, as its row in the chat draws it: the
/// `spawn_agent` call and, once the agents list has it, the child itself.
public struct AgentSpawn: Identifiable, Equatable, Sendable {
    public var callId: String
    /// The spawn call's own state: running until the child exists.
    public var status: ToolStatus?
    /// What the call asked for, until the child says it itself.
    public var request: String?
    /// What the call answered when it failed.
    public var failure: String?
    public var agentId: Int?
    public var info: SubagentInfo?

    public var id: String { callId }

    public init(tool: ToolItem, agents: [SubagentInfo]) {
        callId = tool.callId
        status = tool.status
        request = tool.summary ?? tool.inputDetail
        agentId = tool.status == .failed ? nil : SubagentInfo.spawnedId(in: tool.output)
        failure = tool.status == .failed ? tool.output?.split(whereSeparator: \.isNewline).first.map(String.init) : nil
        info = agentId.flatMap { id in agents.first { $0.id == id } }
    }

    /// The rows of the spawns `ids` names, in order.
    public static func rows(_ ids: [Int], in items: [TranscriptItem], agents: [SubagentInfo]) -> [AgentSpawn] {
        let wanted = Set(ids)
        return items.compactMap { item in
            guard wanted.contains(item.id), case .tool(let tool) = item.content else { return nil }
            return AgentSpawn(tool: tool, agents: agents)
        }
    }

    /// The name the row shows.
    public var name: String {
        if let info { return info.name }
        if let agentId { return "agent-\(agentId)" }
        return status == .failed ? "Agent did not start" : "Starting agent…"
    }

    /// The task the row shows after the name.
    public var task: String? {
        let text = info?.taskLine ?? request
        return text?.isEmpty == false ? text : failure
    }

    /// Read aloud by VoiceOver: name, role, state and time.
    public func accessibilityLabel(now: Date = Date()) -> String {
        var parts = [name]
        if let info {
            if info.agentType != "default" { parts.append(info.agentType) }
            parts.append(info.isActive ? "working" : info.status.label)
            if let elapsed = info.elapsed(now: now) { parts.append(elapsed) }
        }
        if let task { parts.append(task) }
        return parts.joined(separator: ", ")
    }
}

extension ACPRequests {
    public static func agents(sessionId: String) -> JSONValue {
        ["sessionId": .string(sessionId), "action": "list"]
    }

    public static func agentHistory(sessionId: String, id: Int) -> JSONValue {
        ["sessionId": .string(sessionId), "action": "history", "id": .number(Double(id))]
    }

    public static func agentSend(sessionId: String, id: Int, text: String) -> JSONValue {
        ["sessionId": .string(sessionId), "action": "send", "id": .number(Double(id)), "text": .string(text)]
    }

    public static func agentCancel(sessionId: String, id: Int) -> JSONValue {
        ["sessionId": .string(sessionId), "action": "cancel", "id": .number(Double(id))]
    }
}
