import Foundation

public enum TimingPresentation {
    public static func duration(_ milliseconds: UInt64) -> String {
        if milliseconds < 1000 { return "\(milliseconds)ms" }
        let seconds = milliseconds / 1000
        if seconds < 60 { return "\(seconds)s" }
        if seconds < 3600 { return "\(seconds / 60)m \(seconds % 60)s" }
        return "\(seconds / 3600)h \((seconds % 3600) / 60)m"
    }

    public static func turnLabel(_ timing: TurnTiming, isWaiting: Bool = false) -> String {
        let status: String = switch timing.outcome {
        case "completed": "Completed"
        case "cancelled": "Cancelled"
        case "failed": "Failed"
        case .some(let value): "Outcome: \(value)"
        case nil: isWaiting ? "Waiting for you" : "Working"
        }
        guard let elapsed = timing.elapsedWorkMilliseconds else { return status }
        return "\(status) · Worked for \(duration(elapsed))"
    }

    /// The host reports a single aggregate for the assistant segment. Display it on
    /// the first thinking disclosure, even when that segment has several phases.
    public static func thinkingLabel(at index: Int, in blocks: [AssistantBlock], milliseconds: UInt64?, tokens: UInt32? = nil, estimated: Bool? = nil) -> String? {
        guard blocks.indices.contains(index), case .thinking = blocks[index] else { return nil }
        let first = blocks.firstIndex { if case .thinking = $0 { true } else { false } }
        guard first == index else { return "Thinking" }
        var parts = ["Thinking"]
        if let milliseconds { parts.append(duration(milliseconds)) }
        if let tokens { parts.append("\(estimated == true ? "~" : "")\(tokens) tokens") }
        return parts.joined(separator: " · ")
    }

    /// Last correlated transcript entry owns the logical-turn footer. Metadata on a
    /// user or system entry also preserves an empty failed/cancelled turn in history.
    public static func footerOwners(items: [TranscriptItem], lastTurn: TurnTiming?) -> [String: TurnTiming] {
        var owners: [String: String] = [:]
        var terminal: [String: TurnTiming] = [:]
        for item in items {
            guard let timing = item.turnTiming else { continue }
            owners[timing.turnID] = item.id
            if timing.outcome != nil { terminal[timing.turnID] = timing }
        }
        if let lastTurn, lastTurn.outcome != nil { terminal[lastTurn.turnID] = lastTurn }
        var footers: [String: TurnTiming] = [:]
        for (turnID, owner) in owners {
            if let timing = terminal[turnID] { footers[owner] = timing }
        }
        return footers
    }

    public static func standaloneFooter(items: [TranscriptItem], lastTurn: TurnTiming?) -> TurnTiming? {
        guard let lastTurn, lastTurn.outcome != nil else { return nil }
        let represented = footerOwners(items: items, lastTurn: lastTurn).values.contains { $0.turnID == lastTurn.turnID }
        return represented ? nil : lastTurn
    }

    public static func endTimestamp(timing: TurnTiming?, legacy: String?) -> String? {
        if let timing { return timing.endedAt }
        return legacy
    }

    /// The whole turn footer on one line: outcome, work time, and when it ended.
    ///
    /// The end time drops the date when the turn ended today. A timestamp that
    /// does not parse is shown as the host sent it.
    public static func footerLine(
        timing: TurnTiming?,
        legacyCompletedAt: String?,
        now: Date = Date(),
        calendar: Calendar = .current,
        locale: Locale = .current
    ) -> String? {
        var parts: [String] = []
        if let timing, timing.outcome != nil {
            parts.append(turnLabel(timing))
        }
        if let timestamp = endTimestamp(timing: timing, legacy: legacyCompletedAt) {
            if parts.isEmpty { parts.append("Completed") }
            if let date = date(timestamp) {
                var style = Date.FormatStyle(date: .omitted, time: .shortened, locale: locale, calendar: calendar, timeZone: calendar.timeZone)
                if !calendar.isDate(date, inSameDayAs: now) {
                    style = style.month(.abbreviated).day()
                    if !calendar.isDate(date, equalTo: now, toGranularity: .year) { style = style.year() }
                }
                parts.append(date.formatted(style))
            } else {
                parts.append(timestamp)
            }
        }
        return parts.isEmpty ? nil : parts.joined(separator: " · ")
    }

    public static func endLabel(outcome: String?) -> String {
        switch outcome {
        case "failed": "Failed at"
        case "cancelled": "Cancelled at"
        case "completed", nil: "Completed at"
        default: "Ended at"
        }
    }

    public static func date(_ timestamp: String) -> Date? {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let date = formatter.date(from: timestamp) { return date }
        formatter.formatOptions = [.withInternetDateTime]
        return formatter.date(from: timestamp)
    }
}
