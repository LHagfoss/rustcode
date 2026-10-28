import Foundation

/// The `_meta.rustcode` version of the compaction card this app reads.
public let rustcodeCompactionMetaVersion = 1

/// A compaction of the chat's history, as the chat shows it: one card where
/// it happened, with the messages before it still above. The engine sends it
/// live (for `/compact` and for an automatic compaction) and again on replay,
/// where the summary is all that is left of the earlier messages.
public struct CompactionCard: Equatable, Sendable {
    /// The engine's one-line notice, shown when the numbers are unknown.
    public var text: String
    /// `manual` or `auto`; nil for a replayed compaction with no record.
    public var trigger: String?
    /// `ai_summary` or `deterministic_record`.
    public var mode: String?
    public var beforeTokens: Int?
    public var afterTokens: Int?
    /// How many compactions the session has run, this one included.
    public var count: Int?
    /// What the model works from now: the handoff the summarizer wrote, or
    /// the deterministic record.
    public var summary: String?

    public init(text: String, trigger: String? = nil, mode: String? = nil, beforeTokens: Int? = nil,
                afterTokens: Int? = nil, count: Int? = nil, summary: String? = nil) {
        self.text = text
        self.trigger = trigger
        self.mode = mode
        self.beforeTokens = beforeTokens
        self.afterTokens = afterTokens
        self.count = count
        self.summary = summary
    }

    /// Reads a reply chunk's `_meta.rustcode`; nil unless it is a compaction
    /// card of a version this app knows.
    public init?(text: String, meta: JSONValue?) {
        guard let meta, meta["kind"]?.stringValue == "compaction",
              meta["version"]?.intValue == rustcodeCompactionMetaVersion else { return nil }
        let summary = meta["summary"]?.stringValue?.trimmingCharacters(in: .whitespacesAndNewlines)
        self.init(text: text,
                  trigger: meta["trigger"]?.stringValue,
                  mode: meta["mode"]?.stringValue,
                  beforeTokens: meta["beforeTokens"]?.intValue,
                  afterTokens: meta["afterTokens"]?.intValue,
                  count: meta["count"]?.intValue,
                  summary: summary?.isEmpty == false ? summary : nil)
    }

    /// "Compacted 161k → 49k · #2", or "History compacted" without numbers.
    public var headline: String {
        guard let beforeTokens, let afterTokens else { return "History compacted" }
        var line = "Compacted \(SessionUsage.compact(beforeTokens)) → \(SessionUsage.compact(afterTokens))"
        if let count { line += " · #\(count)" }
        return line
    }

    /// What ran it and how, for the card's second line: "Automatic · reduced
    /// record" or "Manual · AI summary".
    public var detail: String? {
        let parts = [
            trigger.map { $0 == "auto" ? "Automatic" : "Manual" },
            mode.map { $0 == "deterministic_record" ? "reduced record" : "AI summary" },
        ].compactMap { $0 }
        return parts.isEmpty ? nil : parts.joined(separator: " · ")
    }
}
