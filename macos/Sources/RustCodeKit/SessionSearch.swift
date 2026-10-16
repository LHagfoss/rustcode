import Foundation

/// A chat whose messages contain the sidebar search text, as
/// `rustcode sessions search --json` prints it.
public struct SessionContentMatch: Decodable, Equatable, Sendable {
    public var id: String
    public var title: String?
    /// Text around the first match, already shortened by the engine.
    public var snippet: String
    public var matches: Int
    public var cwd: String?
}

/// Searching chats by what was said in them: the engine command and how its
/// results join the sidebar's title matches.
public enum SessionContentSearch {
    /// Shorter queries match almost everything; only titles are filtered then.
    public static let minimumQueryLength = 2
    public static let limit = 50

    /// The query to search contents for, or nil when it is too short.
    public static func query(for filter: String) -> String? {
        let query = filter.trimmingCharacters(in: .whitespaces)
        return query.count >= minimumQueryLength ? query : nil
    }

    /// Engine arguments for a search; without `--all` the engine scopes it to
    /// the project it runs in.
    public static func arguments(query: String, allProjects: Bool, limit: Int = limit) -> [String] {
        ["sessions", "search", query, "--limit", String(limit), "--json"] + (allProjects ? ["--all"] : [])
    }

    public static func parse(_ json: Data) throws -> [SessionContentMatch] {
        try JSONDecoder().decode([SessionContentMatch].self, from: json)
    }

    /// Whether the sidebar's title filter matches a chat.
    public static func titleMatches(_ session: SessionSummary, filter: String) -> Bool {
        "\(session.title ?? "New chat") \(session.projectName)"
            .range(of: filter, options: [.caseInsensitive, .diacriticInsensitive]) != nil
    }

    /// Title matches first, in list order, then chats found only by their
    /// contents, in the engine's newest-first order. Only chats in `shown`
    /// appear, so the project scope and archive view still apply.
    public static func merge(shown: [SessionSummary], filter: String, contentMatches: [SessionContentMatch]) -> [SessionSummary] {
        let byTitle = shown.filter { titleMatches($0, filter: filter) }
        guard !contentMatches.isEmpty else { return byTitle }
        let titled = Set(byTitle.map(\.id))
        let byID = Dictionary(shown.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
        let byContent = contentMatches.compactMap { match in
            titled.contains(match.id) ? nil : byID[match.id]
        }
        return byTitle + byContent
    }
}
