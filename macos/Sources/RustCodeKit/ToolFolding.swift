import Foundation

/// How much of a turn's tool calls the chat shows, as the terminal's
/// `verbosity` setting does.
public enum ToolVerbosity: String, CaseIterable, Identifiable, Sendable {
    /// Each run of calls is one row of what it did, a lone call too. A
    /// finished turn folds its work into one `Worked for…` row.
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
/// folded into one `Worked for 1m 29s` row on the row its work starts with.
/// Opened, it shows the work as it happened.
public struct TurnFold: Equatable, Sendable {
    /// The id of the item the turn's work starts with, which the row is drawn on.
    public var anchor: Int
    public var calls: Int
    public var failed: Int
    public var isOpen: Bool
    /// How long the turn took, when its answer says.
    public var elapsed: TimeInterval?

    /// `Worked for 1m 29s`, or the calls when the time is not known.
    public var title: String {
        guard let elapsed else { return "Worked · \(calls) tool \(calls == 1 ? "call" : "calls")" }
        return "Worked for " + Self.duration(elapsed)
    }

    static func duration(_ seconds: TimeInterval) -> String {
        let total = max(Int(seconds.rounded()), 1)
        if total < 60 { return "\(total)s" }
        if total < 3600 { return total % 60 == 0 ? "\(total / 60)m" : "\(total / 60)m \(total % 60)s" }
        return "\(total / 3600)h \(total % 3600 / 60)m"
    }

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
    /// What the run did, in words (`ToolPhrase.summary`).
    public var label: String
    /// The files the run's edits changed, each once in the order first edited.
    public var edited: [String] = []
    /// The lines those edits added and removed, when every edit carries its diff.
    public var lines: LineCounts? = nil

    public struct LineCounts: Equatable, Sendable {
        public var added: Int
        public var removed: Int
    }

    static func edited(by calls: some Collection<ToolItem>) -> [String] {
        var paths: [String] = []
        for change in calls.flatMap(\.changes) where !paths.contains(change.path) {
            paths.append(change.path)
        }
        return paths
    }

    /// Nil when a finished call of an editing tool came back without a diff,
    /// so a total would leave its lines out.
    static func lines(of calls: some Collection<ToolItem>) -> LineCounts? {
        let editing = Set(calls.filter { !$0.changes.isEmpty }.map(\.title))
        guard !editing.isEmpty else { return nil }
        let missing = calls.contains { editing.contains($0.title) && $0.changes.isEmpty && $0.status != .failed }
        guard !missing else { return nil }
        let changes = calls.flatMap(\.changes)
        return LineCounts(added: changes.reduce(0) { $0 + $1.added }, removed: changes.reduce(0) { $0 + $1.removed })
    }
}

/// The message the engine adds after a tool batch to show the model the
/// screenshots it took (`preview::screenshot_attachment_message`). It is the
/// tool's, not the user's: it does not start a turn.
public enum ToolAttachment {
    public static func isAttachment(_ text: String) -> Bool {
        text.hasPrefix("[Attached by the ") && text.contains("tool, not typed by the user")
    }
}

extension TranscriptItem {
    /// A message the user sent, which starts a turn.
    var startsTurn: Bool {
        switch content {
        case .user(let text): !ToolAttachment.isAttachment(text)
        case .command: true
        default: false
        }
    }
}

/// Which tool cards a chat shows at a verbosity: rows stay one per item, so
/// a folded run hides its calls and its first row carries the summary.
///
/// At the folding verbosities a turn only grows while it runs: what the
/// model says stays where it said it, each run of calls between is one row
/// of what they did, and its thoughts wait, read by the line under the turn
/// instead (`activity`). Once done, the work between its first words and its
/// answer folds into one `Worked for…` row, which opens to all of it.
public struct ToolFolding: Equatable, Sendable {
    /// Rows not drawn.
    public private(set) var hidden: Set<Int> = []
    /// The summary each run's first row draws.
    public private(set) var folds: [Int: ToolRunFold] = [:]
    /// The summary each finished turn's work draws above it.
    public private(set) var turns: [Int: TurnFold] = [:]
    /// Rows inside an opened turn, drawn indented under its row.
    public private(set) var nested: Set<Int> = []
    /// Agents started together, by the id of their first spawn: that row
    /// draws them all, at every verbosity, and the others are hidden (#2327).
    public private(set) var agentGroups: [Int: [Int]] = [:]
    /// What the running turn is doing now (`CurrentWork`).
    public private(set) var activity: String?

    public init() {}

    /// `open` holds the runs and turns the reader opened.
    public init(items: [TranscriptItem], verbosity: ToolVerbosity, open: Set<Int>, running: Bool) {
        groupAgents(items)
        let lastStart = items.lastIndex(where: \.startsTurn).map { $0 + 1 } ?? 0
        if running { activity = CurrentWork.now(in: items[lastStart...]) }
        guard verbosity.folds else { return }
        var start = 0
        for (index, item) in items.enumerated() where item.startsTurn {
            foldTurn(items[start..<index], verbosity: verbosity, open: open, live: false)
            start = index + 1
        }
        foldTurn(items[start...], verbosity: verbosity, open: open, live: running)
    }

    /// Spawns in a row are one group, at every verbosity (#2327).
    private mutating func groupAgents(_ items: [TranscriptItem]) {
        var spawns: [Int] = []
        func close() {
            defer { spawns = [] }
            guard let first = spawns.first else { return }
            agentGroups[first] = spawns
            hidden.formUnion(spawns.dropFirst())
        }
        for item in items {
            if case .tool(let tool) = item.content, tool.isAgentSpawn {
                spawns.append(item.id)
            } else if case .thinking = item.content {
                continue
            } else {
                close()
            }
        }
        close()
    }

    private static func isCall(_ item: TranscriptItem) -> Bool {
        if case .tool(let tool) = item.content { !tool.isAgentSpawn } else { false }
    }

    private static func isThought(_ item: TranscriptItem) -> Bool {
        if case .thinking = item.content { true } else { false }
    }

    private static func isSaid(_ item: TranscriptItem) -> Bool {
        if case .assistant = item.content { true } else { false }
    }

    /// A call, or the screenshots calls took: what a turn's row folds.
    private static func isWork(_ item: TranscriptItem) -> Bool {
        if case .user(let text) = item.content { ToolAttachment.isAttachment(text) } else { isCall(item) }
    }

    private mutating func foldTurn(_ turn: ArraySlice<TranscriptItem>, verbosity: ToolVerbosity,
                                   open: Set<Int>, live: Bool) {
        // A running turn's thoughts are read by the line under it.
        if live { hidden.formUnion(turn.filter(Self.isThought).map(\.id)) }
        guard let first = turn.firstIndex(where: Self.isCall),
              let last = turn.lastIndex(where: Self.isWork) else { return }
        // A turn that worked says so with what it did, not its reasoning; a
        // reply without calls keeps its thought.
        hidden.formUnion(turn.filter(Self.isThought).map(\.id))
        // The work starts at the call, or at what the model said after its
        // first words when that comes before the first call.
        let start = turn[..<first].filter(Self.isSaid).dropFirst().first
            .flatMap { item in turn.firstIndex { $0.id == item.id } } ?? first
        if live {
            foldRuns(turn[start...], verbosity: verbosity, open: open, live: true)
            return
        }
        let work = turn[start...last]
        let calls = work.filter(Self.isCall).compactMap { item -> ToolItem? in
            if case .tool(let tool) = item.content { tool } else { nil }
        }
        let anchor = work[start].id
        let isOpen = open.contains(TurnFold.key(for: anchor))
        turns[anchor] = TurnFold(anchor: anchor, calls: calls.count,
                                 failed: calls.filter { $0.status == .failed }.count, isOpen: isOpen,
                                 elapsed: turn.last { $0.turn != nil }?.turn?.elapsed)
        foldRuns(work, verbosity: verbosity, open: open, live: false)
        let inside = work.map(\.id).filter { $0 != anchor }
        if isOpen {
            nested.formUnion(inside)
            return
        }
        hidden.formUnion(inside)
        // A turn that stopped before its answer, as when the connection
        // dropped, ends on the last thing the model said under its row.
        let answered = turn[(last + 1)...].contains(where: Self.isSaid)
        if !answered, let lastSaid = work.last(where: { Self.isSaid($0) && $0.id != anchor }) {
            hidden.remove(lastSaid.id)
        }
    }

    /// Each run of calls between two things the model said is one row of
    /// what they did. While `live`, a call still running waits for the line
    /// under the turn, and joins its run once done.
    private mutating func foldRuns(_ work: ArraySlice<TranscriptItem>, verbosity: ToolVerbosity,
                                   open: Set<Int>, live: Bool) {
        var run: [ToolItem] = []
        var ids: [Int] = []
        func close() {
            defer { run = []; ids = [] }
            guard let first = ids.first else { return }
            let isOpen = open.contains(first)
            folds[first] = ToolRunFold(run: first, count: run.count,
                                       failed: run.filter { $0.status == .failed }.count,
                                       isOpen: isOpen, label: ToolPhrase.summary(run),
                                       edited: ToolRunFold.edited(by: run), lines: ToolRunFold.lines(of: run))
            if !isOpen { hidden.formUnion(ids.dropFirst()) }
        }
        for item in work {
            if Self.isThought(item) { continue }
            guard Self.isCall(item), case .tool(let tool) = item.content else {
                close()
                continue
            }
            if live, !(tool.status?.isTerminal ?? true) {
                hidden.insert(item.id)
                continue
            }
            // An edit stays out of the runs with its diff open.
            if verbosity.editsOpen && !tool.changes.isEmpty {
                close()
                continue
            }
            run.append(tool)
            ids.append(item.id)
        }
        close()
    }

    /// The `Worked for…` row on `item`, if a finished turn's work starts there.
    public func turn(for item: Int) -> TurnFold? { turns[item] }

    /// Whether `item` is drawn inside an opened turn.
    public func isNested(_ item: Int) -> Bool { nested.contains(item) }

    /// The summary on `item`'s row, if it starts a folded run.
    public func fold(for item: Int) -> ToolRunFold? { folds[item] }

    /// The spawns `item`'s row draws, if it starts a group of agents.
    public func agents(for item: Int) -> [Int]? { agentGroups[item] }
}
