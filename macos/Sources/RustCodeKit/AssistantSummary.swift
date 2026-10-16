import Foundation

/// The engine's closing summary, a small JSON object (`{"summary": …, "next_action": …}`)
/// that arrives as assistant text, alone or right after the reply's prose.
public struct AssistantSummary: Equatable, Sendable {
    public let summary: String
    public let nextAction: String?

    public init(summary: String, nextAction: String?) {
        self.summary = summary
        self.nextAction = nextAction
    }

    /// The prose before a trailing summary object, and the summary. Text with no
    /// such object comes back whole with a nil summary.
    public static func split(_ text: String) -> (prose: String, summary: AssistantSummary?) {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard trimmed.hasSuffix("}") else { return (text, nil) }
        var search = trimmed.endIndex
        while let start = trimmed.range(of: "{", options: .backwards, range: trimmed.startIndex..<search)?.lowerBound {
            if let summary = parse(String(trimmed[start...])) {
                return (String(trimmed[..<start]).trimmingCharacters(in: .whitespacesAndNewlines), summary)
            }
            search = start
        }
        return (text, nil)
    }

    public static func parse(_ text: String) -> AssistantSummary? {
        guard let data = text.data(using: .utf8),
              let values = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
              Set(values.keys).isSubset(of: ["summary", "next_action"]),
              let summary = (values["summary"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines),
              !summary.isEmpty else { return nil }
        var nextAction: String?
        switch values["next_action"] {
        case nil, is NSNull: nextAction = nil
        case let action as String:
            let action = action.trimmingCharacters(in: .whitespacesAndNewlines)
            nextAction = action.isEmpty ? nil : action
        default: return nil
        }
        return AssistantSummary(summary: summary, nextAction: nextAction)
    }
}
