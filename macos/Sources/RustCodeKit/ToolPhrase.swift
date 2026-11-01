import Foundation

/// What tool calls did, in words: `Read 4 files, ran 2 commands` for a run,
/// `Ran npm test` for one call, and `Running npm test` for the call a turn
/// is on now.
public enum ToolPhrase {
    enum Kind: CaseIterable {
        case read, search, web, edit, run, skill, agent, other

        var verbs: (done: String, now: String) {
            switch self {
            case .read: ("Read", "Reading")
            case .search: ("Searched", "Searching")
            case .web: ("Browsed", "Browsing")
            case .edit: ("Edited", "Editing")
            case .run: ("Ran", "Running")
            case .skill: ("Loaded", "Loading")
            case .agent: ("Started", "Starting")
            case .other: ("Used", "Using")
            }
        }

        var nouns: (one: String, many: String) {
            switch self {
            case .read, .edit: ("file", "files")
            case .search: ("search", "searches")
            case .web: ("page", "pages")
            case .run: ("command", "commands")
            case .skill: ("skill", "skills")
            case .agent: ("agent", "agents")
            case .other: ("tool", "tools")
            }
        }
    }

    static func kind(of call: ToolItem) -> Kind {
        if !call.changes.isEmpty { return .edit }
        switch call.displayTitle {
        case "Read", "List", "Project map": return .read
        case "Search": return .search
        case "Web search", "Read URL", "Preview", "GitHub": return .web
        case "Edit", "Write", "Delete", "Move", "Copy": return .edit
        case "Run", "Git status", "Git diff", "Git add", "Git commit": return .run
        case "Skill", "Skills": return .skill
        case "Start agent", "Delegate": return .agent
        default: return .other
        }
    }

    /// A run of calls: one call by what it acted on, more by kind and count
    /// in the order first used.
    public static func summary(_ calls: some Collection<ToolItem>) -> String {
        if calls.count == 1, let call = calls.first { return phrase(call, now: false) }
        var order: [Kind] = []
        var counts: [Kind: Int] = [:]
        var edited: Set<String> = []
        for call in calls {
            let kind = kind(of: call)
            if counts[kind] == nil { order.append(kind) }
            if kind == .edit, !call.changes.isEmpty {
                let new = call.changes.map(\.path).filter { !edited.contains($0) }
                edited.formUnion(new)
                counts[kind, default: 0] += new.count
            } else {
                counts[kind, default: 0] += 1
            }
        }
        return order.enumerated().map { index, kind in
            let count = max(counts[kind, default: 1], 1)
            let verb = index == 0 ? kind.verbs.done : kind.verbs.done.lowercased()
            // Searches are not things to count by name: `searched code 3 times`.
            if kind == .search { return "\(verb) code" + (count > 1 ? " \(count) times" : "") }
            return "\(verb) \(count) \(count == 1 ? kind.nouns.one : kind.nouns.many)"
        }.joined(separator: ", ")
    }

    /// `Ran npm test`, or `Running npm test` while it runs. A search, a
    /// skill, an agent or another tool reads as what the model said it does,
    /// `Find Tab handlers`, which already starts with its verb.
    public static func phrase(_ call: ToolItem, now: Bool) -> String {
        if let git = gitPhrase(call, now: now) { return git }
        let kind = kind(of: call)
        if [.search, .skill, .agent, .other].contains(kind), let summary = call.summary { return summary }
        let verb = now ? kind.verbs.now : kind.verbs.done
        guard let object = object(of: call, kind: kind) else { return "\(verb) a \(kind.nouns.one)" }
        return "\(verb) \(object)"
    }

    /// The built-in git tools in words, `Staged calc.py` or `Committed
    /// “fix: add”`, instead of `Ran` and whatever detail the engine had: for
    /// a call without a path that was the tool's own name, `GitAdd`.
    static func gitPhrase(_ call: ToolItem, now: Bool) -> String? {
        let input = call.input.flatMap { try? JSONDecoder().decode(JSONValue.self, from: Data($0.utf8)) }
        switch call.title {
        case "git_add":
            let names = (input?["paths"]?.arrayValue ?? []).compactMap(\.stringValue)
                .map { URL(fileURLWithPath: $0).lastPathComponent }
            let what = switch names.count {
            case 0: "changes"
            case 1...3: names.joined(separator: ", ")
            default: names.prefix(2).joined(separator: ", ") + " and \(names.count - 2) more"
            }
            return (now ? "Staging " : "Staged ") + what
        case "git_commit":
            let message = input?["message"]?.stringValue
                .flatMap { $0.split(whereSeparator: \.isNewline).first.map(String.init) }
            return (now ? "Committing" : "Committed") + (message.map { " “\($0)”" } ?? "")
        case "git_status": return now ? "Checking git status" : "Checked git status"
        case "git_diff": return now ? "Reading the diff" : "Read the diff"
        default: return nil
        }
    }

    private static func object(of call: ToolItem, kind: Kind) -> String? {
        switch kind {
        case .read, .edit:
            let path = call.changes.first?.path ?? call.detail
            return path.map { URL(fileURLWithPath: $0).lastPathComponent }.flatMap { $0.isEmpty ? nil : $0 } ?? call.summary
        case .run, .web:
            return call.detail ?? call.summary
        case .search, .skill, .agent:
            return call.summary ?? call.detail
        case .other:
            return call.displayTitle
        }
    }
}

/// What a running turn is doing now, for the line under it: the call it is
/// on, else the heading of the thought it is writing. Nil while it writes
/// its reply or between steps.
public enum CurrentWork {
    public static func now(in turn: some BidirectionalCollection<TranscriptItem>) -> String? {
        if let running = turn.last(where: { item in
            if case .tool(let tool) = item.content { !(tool.status?.isTerminal ?? true) && !tool.isAgentSpawn } else { false }
        }), case .tool(let tool) = running.content {
            return ToolPhrase.phrase(tool, now: true)
        }
        if case .thinking(let text) = turn.last?.content { return heading(of: text) }
        return nil
    }

    /// The `**Heading**` a thought is on, the last one written.
    static func heading(of text: String) -> String {
        let headings = text.split(whereSeparator: \.isNewline)
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .filter { $0.count > 4 && $0.hasPrefix("**") && $0.hasSuffix("**") }
        return headings.last.map { $0.replacingOccurrences(of: "**", with: "") } ?? "Thinking"
    }
}
