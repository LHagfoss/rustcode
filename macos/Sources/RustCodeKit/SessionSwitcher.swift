import Foundation

/// ⌘K: jump to a chat by typing part of its title (#2192).
///
/// With nothing typed it lists the chats the sidebar lists, pinned first, so
/// ⌘K ⏎ goes back to the chat opened before. Typed text matches the title,
/// then the project and model, then what was said, as the engine's content
/// search finds it. The sidebar's All or Project scope applies: the chats
/// passed in are already scoped.
public enum SessionSwitcher {
    /// Rows the switcher lists at most.
    public static let limit = 50

    /// One row: the chat and, for a chat found by its contents, the text
    /// around the match.
    public struct Result: Identifiable, Equatable, Sendable {
        public var session: SessionSummary
        public var snippet: String?
        public var id: String { session.id }

        public init(session: SessionSummary, snippet: String? = nil) {
            self.session = session
            self.snippet = snippet
        }
    }

    /// How well a chat's title, project or model matches `query`; nil when
    /// none does. Lower is better.
    static func rank(_ session: SessionSummary, query: String) -> Int? {
        let options: String.CompareOptions = [.caseInsensitive, .diacriticInsensitive]
        let title = session.title ?? "New thread"
        if title.range(of: query, options: options.union(.anchored)) != nil { return 0 }
        // A word of the title starting with the query: "parser" in "Fix parser".
        let words = title.split { !$0.isLetter && !$0.isNumber }
        if words.contains(where: { $0.range(of: query, options: options.union(.anchored)) != nil }) { return 1 }
        if title.range(of: query, options: options) != nil { return 2 }
        if session.projectName.range(of: query, options: options) != nil { return 3 }
        if let model = session.modelName, model.range(of: query, options: options) != nil { return 4 }
        return nil
    }

    /// The rows for `query`. `sessions` is the sidebar's list in its order,
    /// `contentMatches` what the engine found in chats' messages for it.
    public static func results(sessions: [SessionSummary], query: String, pinned: Set<String>,
                               contentMatches: [SessionContentMatch] = [], limit: Int = limit) -> [Result] {
        let query = query.trimmingCharacters(in: .whitespaces)
        guard !query.isEmpty else {
            let ordered = sessions.filter { pinned.contains($0.id) } + sessions.filter { !pinned.contains($0.id) }
            return ordered.prefix(limit).map { Result(session: $0) }
        }
        // Ties keep the sidebar's order, newest first.
        let ranked = sessions.enumerated()
            .compactMap { index, session in rank(session, query: query).map { (rank: $0, index: index, session: session) } }
            .sorted { ($0.rank, $0.index) < ($1.rank, $1.index) }
        var results = ranked.map { Result(session: $0.session) }
        var seen = Set(results.map(\.id))
        let byID = Dictionary(sessions.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
        for match in contentMatches {
            guard let session = byID[match.id], seen.insert(match.id).inserted else { continue }
            results.append(Result(session: session, snippet: match.snippet))
        }
        return Array(results.prefix(limit))
    }

    /// The row highlighted when the switcher opens: the first one that is
    /// not the open chat, so ⏎ switches somewhere.
    public static func initialSelection(_ results: [Result], current: String?) -> Int {
        results.firstIndex { $0.id != current } ?? 0
    }

    /// ↑ and ↓ move the highlight, wrapping at either end.
    public static func move(_ selection: Int, by step: Int, count: Int) -> Int {
        guard count > 0 else { return 0 }
        return ((selection + step) % count + count) % count
    }

    /// What a key press adds to the search while the switcher's field is
    /// still taking focus: printable text typed without ⌘ or ⌃
    /// (⌥ types characters such as | and @ on many layouts). Keys such
    /// as Return, Tab, Esc and the arrows are left to the switcher.
    public static func typeahead(_ characters: String?, commandKey: Bool) -> String? {
        guard !commandKey, let characters, !characters.isEmpty else { return nil }
        let printable = characters.unicodeScalars.allSatisfy { scalar in
            !CharacterSet.controlCharacters.contains(scalar)
                && !(0xF700...0xF8FF).contains(scalar.value) // AppKit's function keys
        }
        return printable ? characters : nil
    }

    /// The highlight once the rows change under it: on the same chat when it
    /// is still listed, else the first row.
    public static func keep(_ selected: String?, in results: [Result]) -> Int {
        selected.flatMap { id in results.firstIndex { $0.id == id } } ?? 0
    }
}
