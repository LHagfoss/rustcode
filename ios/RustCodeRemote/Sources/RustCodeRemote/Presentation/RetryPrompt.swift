import Foundation

/// Which prompt "Retry" sends again.
public enum RetryPrompt {
    /// The text to resend for a user message: the prompt as stored, including the host's
    /// image references, so the same attachments go with it. Nil when there is nothing to send.
    public static func text(of item: TranscriptItem) -> String? {
        guard case .user(let text) = item.content else { return nil }
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }

    /// The most recent user prompt in a transcript, oldest-first. Nil when it has none.
    public static func latest(in items: [TranscriptItem]) -> String? {
        items.reversed().lazy.compactMap(text(of:)).first
    }
}
