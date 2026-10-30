import Foundation

/// The steps the agent planned for a larger task, as `todo_write` keeps them
/// (#2641). A step's status is the agent's own claim, not proof the work
/// was done: the inspector shows it as the plan, apart from the tool calls
/// and agents that did the work, and links each step to where the agent last
/// updated it.
public struct TaskPlan: Equatable, Sendable {
    public struct Step: Identifiable, Equatable, Sendable {
        public enum Status: String, Equatable, Sendable {
            case pending, inProgress = "in_progress", completed, blocked
            /// A status this app does not know, from a newer engine.
            case unknown
        }

        public var id: String
        public var content: String
        public var status: Status
        public var priority: String?
        public var blockedReason: String?

        public init(id: String, content: String, status: Status, priority: String? = nil, blockedReason: String? = nil) {
            self.id = id
            self.content = content
            self.status = status
            self.priority = priority
            self.blockedReason = blockedReason
        }
    }

    public var steps: [Step]

    public init(steps: [Step]) {
        self.steps = steps
    }

    /// The plan in a `plan` answer.
    public init(result: JSONValue) {
        steps = (result["steps"]?.arrayValue ?? []).compactMap { step in
            guard let id = step["id"]?.stringValue, let content = step["content"]?.stringValue else { return nil }
            return Step(id: id, content: content,
                        status: step["status"]?.stringValue.flatMap(Step.Status.init(rawValue:)) ?? .unknown,
                        priority: step["priority"]?.stringValue,
                        blockedReason: step["blockedReason"]?.stringValue)
        }
    }

    public var isEmpty: Bool { steps.isEmpty }
    public var done: Int { steps.filter { $0.status == .completed }.count }

    /// The step being worked on: the first marked in progress.
    public var current: Step? { steps.first { $0.status == .inProgress } }

    /// `3/7`, for the header button.
    public var progressLabel: String { "\(done)/\(steps.count)" }

    /// Whether the plan is worth a button: one step is a to-do, not a plan.
    public var isShown: Bool { steps.count >= 2 }

    /// Where the agent last wrote each step: the `todo_write` call in
    /// `items` that last named the step's id (or, from a call without ids,
    /// its text). A step the shown conversation never wrote, as one planned
    /// before the oldest message loaded, has none.
    public func lastUpdates(in items: [TranscriptItem]) -> [String: Int] {
        var found: [String: Int] = [:]
        for item in items.reversed() {
            guard case .tool(let tool) = item.content, tool.title == "todo_write", let input = tool.input else { continue }
            for step in steps where found[step.id] == nil {
                if input.contains("\"\(step.id)\"") || input.contains(step.content) { found[step.id] = item.id }
            }
            if found.count == steps.count { break }
        }
        return found
    }

    /// The latest `todo_write` call in `items`: where a step the calls do
    /// not name (a live call carries no input) was last written.
    public static func latestUpdate(in items: [TranscriptItem]) -> Int? {
        items.last { item in
            if case .tool(let tool) = item.content { return tool.title == "todo_write" }
            return false
        }?.id
    }

    /// How many plan updates `items` holds that finished, so a client reads
    /// the plan again when one more does.
    public static func finishedUpdates(in items: [TranscriptItem]) -> Int {
        items.filter { item in
            if case .tool(let tool) = item.content { return tool.title == "todo_write" && tool.status == .completed }
            return false
        }.count
    }

    /// How the step reads while no turn runs: a step still marked in
    /// progress after the turn ended was interrupted, not working.
    public static func isInterrupted(_ step: Step, turnRunning: Bool) -> Bool {
        step.status == .inProgress && !turnRunning
    }
}

extension ACPRequests {
    public static func plan(sessionId: String) -> JSONValue {
        ["sessionId": .string(sessionId), "action": "plan"]
    }
}
