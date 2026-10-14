import Foundation

/// Ranks slash commands for the composer's suggestion list and inline completion.
public enum CommandSuggestions {
    /// How a command relates to what was typed; lower ranks are listed first.
    enum Rank: Int, Comparable {
        case exact, prefix, nameContains, descriptionContains
        static func < (lhs: Rank, rhs: Rank) -> Bool { lhs.rawValue < rhs.rawValue }
    }

    /// The command as typed, always with its leading slash.
    public static func commandText(_ choice: SessionCommandChoice) -> String {
        choice.name.hasPrefix("/") ? choice.name : "/" + choice.name
    }

    /// Commands matching `query` (the composer text, starting with `/`): the exact
    /// command, then prefix matches, then names containing the query, then
    /// descriptions containing it. Catalog order is kept within each rank.
    public static func matches(_ query: String, in commands: [SessionCommandChoice], limit: Int = .max) -> [SessionCommandChoice] {
        let typed = query.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        guard typed.hasPrefix("/"), !typed.contains(where: \.isWhitespace) else { return [] }
        let needle = String(typed.dropFirst())
        let ranked = commands.enumerated().compactMap { index, choice -> (Rank, Int, SessionCommandChoice)? in
            rank(of: choice, for: needle).map { ($0, index, choice) }
        }
        return ranked.sorted { ($0.0, $0.1) < ($1.0, $1.1) }.prefix(limit).map(\.2)
    }

    /// The part of the best prefix match not yet typed, for drawing after the caret.
    /// Nil when the top match is not a prefix match or nothing remains to complete.
    public static func completionSuffix(for text: String, in commands: [SessionCommandChoice]) -> String? {
        guard text.hasPrefix("/"), text.count > 1, !text.contains(where: \.isWhitespace),
              let best = matches(text, in: commands, limit: 1).first else { return nil }
        let value = commandText(best)
        guard value.count > text.count, value.lowercased().hasPrefix(text.lowercased()) else { return nil }
        return String(value.dropFirst(text.count))
    }

    static func rank(of choice: SessionCommandChoice, for needle: String) -> Rank? {
        let name = String(commandText(choice).dropFirst()).lowercased()
        if needle.isEmpty { return .prefix }
        if name == needle { return .exact }
        if name.hasPrefix(needle) { return .prefix }
        if name.contains(needle) { return .nameContains }
        if needle.count >= 2, choice.description.lowercased().contains(needle) { return .descriptionContains }
        return nil
    }
}
