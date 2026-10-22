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
