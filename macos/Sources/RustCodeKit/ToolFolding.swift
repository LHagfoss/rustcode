import Foundation

/// How much of a turn's tool calls the chat shows, as the terminal's
/// `verbosity` setting does.
public enum ToolVerbosity: String, CaseIterable, Identifiable, Sendable {
    /// Calls fold into one `N tool calls` row, a lone call too; while tools
    /// run, only the current call shows. A turn shows only its first thought,
    /// and what the model says between calls folds into the run.
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
    /// The first sentence of the last thing the model said inside the run,
    /// drawn under the row so a closed run still says what is going on.
    public var note: String? = nil

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

    /// The first sentence of `text`, on one line and without the Markdown
    /// marks a plain line would show.
    static func firstSentence(of text: String) -> String? {
        let line = text.replacingOccurrences(of: "**", with: "").replacingOccurrences(of: "`", with: "")
            .split(whereSeparator: \.isNewline)
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .first { !$0.isEmpty }
        guard let line else { return nil }
        var end = line.endIndex
        var index = line.startIndex
        while index < line.endIndex {
            let next = line.index(after: index)
            if ".!?".contains(line[index]), next == line.endIndex || line[next] == " " {
                end = next
                break
            }
            index = next
        }
        return String(line[..<end])
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

    /// `open` holds the runs the reader opened. While `running`, the run at
    /// the end of the chat folds everything but its current call.
    public init(items: [TranscriptItem], verbosity: ToolVerbosity, open: Set<Int>, running: Bool) {
        let thoughts = verbosity.folds ? Self.laterThoughts(in: items, running: running) : []
        hidden = thoughts
        var spawns: [Int] = []
        func closeAgents() {
            defer { spawns = [] }
            guard let first = spawns.first else { return }
            agentGroups[first] = spawns
            hidden.formUnion(spawns.dropFirst())
        }
        var run: [ToolItem] = []
        var ids: [Int] = []
        // What the model said inside the run, and what it said since the
        // run's last call, which joins the run only if another call follows.
        var said: [(id: Int, text: String)] = []
        var pending: [(id: Int, text: String)] = []
        func close(atEnd: Bool) {
            defer { run = []; ids = []; said = []; pending = [] }
            let live = atEnd && running
            let count = live ? run.count - 1 : run.count
            // A live call running alone folds nothing, so what was said
            // before it stays in the chat.
            guard count >= 1, let first = ids.first else { return }
            let isOpen = open.contains(first)
            let calls = run.prefix(count)
            // The sentence right before the live call belongs to it, so it
            // stays the run's note until that call is done.
            folds[first] = ToolRunFold(run: first, count: count,
                                       failed: calls.filter { $0.status == .failed }.count,
                                       isOpen: isOpen, label: ToolRunFold.label(for: calls),
                                       note: said.last.flatMap { ToolRunFold.firstSentence(of: $0.text) })
            if !isOpen {
                hidden.formUnion(ids.prefix(count).dropFirst())
                hidden.formUnion(said.map(\.id))
            }
        }
        for item in items {
            // A hidden thought does not split the calls around it into runs.
            if thoughts.contains(item.id) { continue }
            // Nor does what the model says between them: it folds into the
            // run, and only the reply after a turn's last call stays out.
            if verbosity.folds, case .assistant(let text) = item.content {
                pending.append((item.id, text))
                continue
            }
            guard case .tool(let tool) = item.content else {
                close(atEnd: false)
                closeAgents()
                continue
            }
            // A spawn ends the run before it; spawns in a row are one group.
            if tool.isAgentSpawn {
                close(atEnd: false)
                spawns.append(item.id)
                continue
            }
            closeAgents()
            guard verbosity.folds else { continue }
            if verbosity.editsOpen && !tool.changes.isEmpty {
                close(atEnd: false)
                continue
            }
            said += pending
            pending = []
            run.append(tool)
            ids.append(item.id)
        }
        close(atEnd: true)
        closeAgents()
    }

    /// The summary on `item`'s row, if it starts a folded run.
    public func fold(for item: Int) -> ToolRunFold? { folds[item] }

    /// The spawns `item`'s row draws, if it starts a group of agents.
    public func agents(for item: Int) -> [Int]? { agentGroups[item] }
}
