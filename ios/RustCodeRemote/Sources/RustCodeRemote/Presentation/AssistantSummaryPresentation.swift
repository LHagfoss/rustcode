import Foundation

/// A host-generated conversation summary carried as a small JSON assistant message.
/// Raw transcript text remains available for replay and copying.
public struct AssistantSummaryPresentation: Equatable, Sendable {
    public let summary: String
    public let nextAction: String?

    public init(summary: String, nextAction: String?) {
        self.summary = summary
        self.nextAction = nextAction
    }

    public static func parse(_ text: String) -> Self? {
        guard let data = text.trimmingCharacters(in: .whitespacesAndNewlines).data(using: .utf8),
              let object = try? JSONSerialization.jsonObject(with: data),
              let values = object as? [String: Any],
              Set(values.keys).isSubset(of: ["summary", "next_action"]),
              let summary = values["summary"] as? String else { return nil }

        let summaryText = summary.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !summaryText.isEmpty else { return nil }

        let nextAction: String?
        if let value = values["next_action"] {
            if value is NSNull {
                nextAction = nil
            } else if let action = value as? String {
                let actionText = action.trimmingCharacters(in: .whitespacesAndNewlines)
                nextAction = actionText.isEmpty ? nil : actionText
            } else {
                return nil
            }
        } else {
            nextAction = nil
        }

        return Self(summary: summaryText, nextAction: nextAction)
    }
}
