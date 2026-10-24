import Foundation

/// How much of a turn's tool calls the chat shows, as the terminal's
/// `verbosity` setting does.
public enum ToolVerbosity: String, CaseIterable, Identifiable, Sendable {
    /// Calls fold into one `N tool calls` row, a lone call too; while tools
    /// run, only the current call shows. A finished turn folds its work into
    /// one row, which opens to its steps.
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

/// A finished turn's work between what the model said first and its answer,
/// folded into one row on the row of its first call. Opened, it lists the
/// turn's steps: each thing the model said and its own folded run.
public struct TurnFold: Equatable, Sendable {
    /// The id of the turn's first call, which the row is drawn on.
    public var anchor: Int
    public var steps: Int
    public var calls: Int
    public var failed: Int
    public var isOpen: Bool
    public var label: String? = nil

    /// The key the reader's opened set holds for this turn: negative, so it
    /// never collides with the run its first call starts.
    public var key: Int { Self.key(for: anchor) }
    public static func key(for anchor: Int) -> Int { -anchor - 1 }
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
    /// The summary each finished turn's first call draws above its run.
    public private(set) var turns: [Int: TurnFold] = [:]
    /// Rows inside an opened turn, drawn indented under its row.
    public private(set) var nested: Set<Int> = []
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

    /// `open` holds the runs and turns the reader opened. At the folding
    /// verbosities a finished turn reads as what the model said first, one
    /// row for its work, and its answer; opened, that row lists the turn's
    /// steps, each what the model said and its folded calls. A running turn
    /// shows its steps as they happen, with everything but the current call
    /// of the current step folded.
    public init(items: [TranscriptItem], verbosity: ToolVerbosity, open: Set<Int>, running: Bool) {
        let thoughts = verbosity.folds ? Self.laterThoughts(in: items, running: running) : []
        hidden = thoughts
        groupAgents(items, skipping: thoughts)
        guard verbosity.folds else { return }
        var start = 0
        for (index, item) in items.enumerated() {
            switch item.content {
            case .user, .command:
                foldTurn(items[start..<index], verbosity: verbosity, open: open, live: false, thoughts: thoughts)
                start = index + 1
            default:
                break
            }
        }
        foldTurn(items[start...], verbosity: verbosity, open: open, live: running, thoughts: thoughts)
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

    private static func isCall(_ item: TranscriptItem) -> Bool {
        if case .tool(let tool) = item.content { !tool.isAgentSpawn } else { false }
    }

    private mutating func foldTurn(_ turn: ArraySlice<TranscriptItem>, verbosity: ToolVerbosity,
                                   open: Set<Int>, live: Bool, thoughts: Set<Int>) {
        guard let first = turn.firstIndex(where: Self.isCall),
              let last = turn.lastIndex(where: Self.isCall) else { return }
        // The work starts at the call, or at what the model said after its
        // first words when that comes before the first call.
        let said = turn[..<first].filter {
            if case .assistant = $0.content { !thoughts.contains($0.id) } else { false }
        }
        let start = said.dropFirst().first.flatMap { item in turn.firstIndex { $0.id == item.id } } ?? first
        let work = turn[start...last]
        // A running turn, or one whose work is one call, shows its steps.
        let calls = work.filter(Self.isCall).compactMap { item -> ToolItem? in
            if case .tool(let tool) = item.content { tool } else { nil }
        }
        if live || calls.count == 1 {
            _ = foldSteps(work, verbosity: verbosity, open: open, live: live, thoughts: thoughts)
            return
        }
        let anchor = work[start].id
        let isOpen = open.contains(TurnFold.key(for: anchor))
        let steps = foldSteps(work, verbosity: verbosity, open: open, live: false, thoughts: thoughts)
        turns[anchor] = TurnFold(anchor: anchor, steps: steps, calls: calls.count,
                                 failed: calls.filter { $0.status == .failed }.count,
                                 isOpen: isOpen, label: ToolRunFold.label(for: calls))
        let inside = work.map(\.id).filter { $0 != anchor && !thoughts.contains($0) }
        if isOpen {
            nested.formUnion(inside)
        } else {
            hidden.formUnion(inside)
            // A turn that stopped before its answer, as when the connection
            // dropped, ends on the last thing the model said under its row.
            let answered = turn[(last + 1)...].contains { if case .assistant = $0.content { true } else { false } }
            if !answered, let lastSaid = work.last(where: { item in
                if case .assistant = item.content { item.id != anchor } else { false }
            }) {
                hidden.remove(lastSaid.id)
            }
        }
    }

    /// Fold `work` into steps: each run of calls between two things the model
    /// said is one row. While `live`, the last run's current call shows.
    /// Returns how many steps there are.
    private mutating func foldSteps(_ work: ArraySlice<TranscriptItem>, verbosity: ToolVerbosity,
                                    open: Set<Int>, live: Bool, thoughts: Set<Int>) -> Int {
        var steps = 0
        var run: [ToolItem] = []
        var ids: [Int] = []
        func close(atEnd: Bool) {
            defer { run = []; ids = [] }
            let count = atEnd && live ? run.count - 1 : run.count
            guard count >= 1, let first = ids.first else { return }
            steps += 1
            let isOpen = open.contains(first)
            let calls = run.prefix(count)
            folds[first] = ToolRunFold(run: first, count: count,
                                       failed: calls.filter { $0.status == .failed }.count,
                                       isOpen: isOpen, label: ToolRunFold.label(for: calls))
            if !isOpen { hidden.formUnion(ids.prefix(count).dropFirst()) }
        }
        for item in work where !thoughts.contains(item.id) {
            guard Self.isCall(item), case .tool(let tool) = item.content else {
                close(atEnd: false)
                continue
            }
            // An edit stays out of the runs with its diff open.
            if verbosity.editsOpen && !tool.changes.isEmpty {
                close(atEnd: false)
                steps += 1
                continue
            }
            run.append(tool)
            ids.append(item.id)
        }
        close(atEnd: true)
        return steps
    }

    /// The turn summary on `item`'s row, if it is a finished turn's first call.
    public func turn(for item: Int) -> TurnFold? { turns[item] }

    /// Whether `item` is drawn inside an opened turn.
    public func isNested(_ item: Int) -> Bool { nested.contains(item) }

    /// The summary on `item`'s row, if it starts a folded run.
    public func fold(for item: Int) -> ToolRunFold? { folds[item] }

    /// The spawns `item`'s row draws, if it starts a group of agents.
    public func agents(for item: Int) -> [Int]? { agentGroups[item] }
}
