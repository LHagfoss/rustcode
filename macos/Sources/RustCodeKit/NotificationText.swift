import Foundation

/// The first line of a reply, short enough for a notification.
public func notificationSnippet(_ text: String, limit: Int = 140) -> String {
    let line = text.split(whereSeparator: \.isNewline)
        .map { $0.trimmingCharacters(in: .whitespaces) }
        .first { !$0.isEmpty && !$0.hasPrefix("```") } ?? ""
    let plain = line.replacingOccurrences(of: "**", with: "").replacingOccurrences(of: "`", with: "")
    return plain.count > limit ? String(plain.prefix(limit - 1)) + "…" : plain
}
