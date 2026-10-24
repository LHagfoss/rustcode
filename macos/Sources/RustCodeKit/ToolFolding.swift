import Foundation

/// How much of a turn's tool calls the chat shows, as the terminal's
/// `verbosity` setting does.
public enum ToolVerbosity: String, CaseIterable, Identifiable, Sendable {
    /// Calls fold into one `N tool calls` row, a lone call too; while tools
    /// run, only the current call shows. A finished turn shows what the model
    /// said first, one row for all its calls, and its answer.
    case low
    /// Every call is its own closed card.
    case high
    /// Like `low`, but file edits stay out of the runs with their diffs open.
    case edits

    public var id: String { rawValue }
    public static let key = "toolVerbosity"

    public var title: String {
        switch self {
        case .low: "Low"
        case .high: "High"
        case .edits: "Edits"
        }
    }

    /// Runs of calls fold (low and edits).
    public var folds: Bool { self != .high }
    /// File edits start open on their diff.
    public var editsOpen: Bool { self == .edits }

    /// The `verbosity = "…"` line of the terminal's `config.toml`, so the
    /// app starts where the terminal is set.
    public static func configured(in config: String) -> ToolVerbosity? {
        for line in config.split(separator: "\n") {
            let parts = line.split(separator: "=", maxSplits: 1)
            guard parts.count == 2, parts[0].trimmingCharacters(in: .whitespaces) == "verbosity" else { continue }
            let value = parts[1].trimmingCharacters(in: .whitespaces).trimmingCharacters(in: CharacterSet(charactersIn: "\""))
            return ToolVerbosity(rawValue: value)
        }
        return nil
    }
}

/// A folded run of tool calls, drawn on the row of its first call.
public struct ToolRunFold: Equatable, Sendable {
    /// The id of the run's first call, which names the run.
    public var run: Int
    public var count: Int
    public var failed: Int
    public var isOpen: Bool
    /// What the run did, after its count: the tools it used, or what a lone
    /// call said it does.
    public var label: String? = nil

    /// `Web search ×3, Fetch`: each tool once, in the order first used.
    static func label(for calls: some Collection<ToolItem>) -> String? {
        if calls.count == 1, let call = calls.first {
            return [call.displayTitle, call.summary ?? call.detail].compactMap { $0 }.joined(separator: " · ")
        }
        var order: [String] = []
        var counts: [String: Int] = [:]
        for call in calls {
            if counts[call.displayTitle] == nil { order.append(call.displayTitle) }
            counts[call.displayTitle, default: 0] += 1
        }
        let shown = order.prefix(3).map { title in
            counts[title, default: 1] > 1 ? "\(title) ×\(counts[title, default: 1])" : title
        }
        let rest = order.count - shown.count
        return (shown + (rest > 0 ? ["\(rest) more"] : [])).joined(separator: ", ")
    }
}

/// Which tool cards a chat shows at a verbosity: rows stay one per item, so
/// a folded run hides its calls and its first row carries the summary.
public struct ToolFolding: Equatable, Sendable {
    /// Calls inside a closed run.
    public private(set) var hidden: Set<Int> = []
    /// The summary each run's first row draws.
    public private(set) var folds: [Int: ToolRunFold] = [:]
    /// Agents started together, by the id of their first spawn: that row
    /// draws them all, at every verbosity, and the others are hidden (#2327).
    public private(set) var agentGroups: [Int: [Int]] = [:]

    public init() {}

    /// Thoughts after the first of their turn. At the folding verbosities a
    /// turn reads as its first thought, then its calls, then its reply,
    /// instead of a reasoning box before every step. The thought still
    /// streaming at the end stays, so the chat shows the work.
    static func laterThoughts(in items: [TranscriptItem], running: Bool) -> Set<Int> {
        var hidden: Set<Int> = []
        var seenThought = false
        for (index, item) in items.enumerated() {
            switch item.content {
            case .user, .command:
                seenThought = false
            case .thinking:
                let isLive = running && index == items.count - 1
                if seenThought && !isLive { hidden.insert(item.id) }
                seenThought = true
            case .assistant, .tool:
                break
            }
        }
        return hidden
    }

    /// `open` holds the runs the reader opened. At the folding verbosities a
    /// finished turn reads as what the model said first, one row for every
    /// call it made, and its answer; what it said and thought in between
    /// is inside the row. While `running`, the last turn's current round
    /// stays out of the row: the call running and what the model says
    /// about it, and at `edits` the turn's edits with their diffs.
    public init(items: [TranscriptItem], verbosity: ToolVerbosity, open: Set<Int>, running: Bool) {
        let thoughts = verbosity.folds ? Self.laterThoughts(in: items, running: running) : []
        hidden = thoughts
        groupAgents(items, skipping: thoughts)
        guard verbosity.folds else { return }
        var start = 0
        for (index, item) in items.enumerated() {
            switch item.content {
            case .user, .command:
                foldTurn(items[start..<index], verbosity: verbosity, open: open, live: false)
                start = index + 1
            default:
                break
            }
        }
        foldTurn(items[start...], verbosity: verbosity, open: open, live: running)
    }

    /// Spawns in a row are one group, at every verbosity (#2327).
    private mutating func groupAgents(_ items: [TranscriptItem], skipping thoughts: Set<Int>) {
        var spawns: [Int] = []
        func close() {
            defer { spawns = [] }
            guard let first = spawns.first else { return }
            agentGroups[first] = spawns
            hidden.formUnion(spawns.dropFirst())
        }
        for item in items where !thoughts.contains(item.id) {
            if case .tool(let tool) = item.content, tool.isAgentSpawn {
                spawns.append(item.id)
            } else {
                close()
            }
        }
        close()
    }

    /// Fold one turn's calls into the row of its first folded call.
    private mutating func foldTurn(_ turn: ArraySlice<TranscriptItem>, verbosity: ToolVerbosity,
                                   open: Set<Int>, live: Bool) {
        // Spawned agents keep their own row; every other call is the turn's.
        let calls = turn.filter {
            if case .tool(let tool) = $0.content { !tool.isAgentSpawn } else { false }
        }
        guard let lastCall = calls.last else { return }
        let current = live ? lastCall.id : nil
        // The call before the current one: what was said after it is about
        // the current call, so it stays in the chat with it.
        let previous = live ? calls.dropLast().last?.id : nil
        let folded = calls.filter { item in
            guard item.id != current else { return false }
            if live, verbosity.editsOpen, case .tool(let tool) = item.content, !tool.changes.isEmpty {
                return false
            }
            return true
        }
        guard let anchor = folded.first else { return }
        let tools = folded.compactMap { item -> ToolItem? in
            if case .tool(let tool) = item.content { tool } else { nil }
        }
        let isOpen = open.contains(anchor.id)
        folds[anchor.id] = ToolRunFold(run: anchor.id, count: tools.count,
                                       failed: tools.filter { $0.status == .failed }.count,
                                       isOpen: isOpen, label: ToolRunFold.label(for: tools))
        guard !isOpen else { return }
        hidden.formUnion(folded.dropFirst().map(\.id))
        // What the model said first and its answer after the last call stay;
        // so does, while live, what it says about the current call.
        let firstCall = calls[0].id
        let keepAfter = previous ?? lastCall.id
        for item in turn {
            guard case .assistant = item.content else { continue }
            let beforeCalls = item.id < firstCall
            let afterKept = item.id > keepAfter
            if !beforeCalls && !afterKept { hidden.insert(item.id) }
        }
        // Only the first sentence block before the calls stays, not every one.
        let opening = turn.filter { item in
            if case .assistant = item.content { item.id < firstCall } else { false }
        }
        hidden.formUnion(opening.dropFirst().map(\.id))
    }

    /// The summary on `item`'s row, if it starts a folded run.
    public func fold(for item: Int) -> ToolRunFold? { folds[item] }

    /// The spawns `item`'s row draws, if it starts a group of agents.
    public func agents(for item: Int) -> [Int]? { agentGroups[item] }
}
