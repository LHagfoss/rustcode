import Foundation

/// How long a finished turn took and when it ended, shown under its last
/// reply. A turn that failed has none: it never finished.
public struct TurnTiming: Equatable, Sendable {
    public enum Outcome: String, Equatable, Sendable {
        case completed
        /// The user stopped it.
        case cancelled
    }

    /// The engine's id for the turn, on replayed history only.
    public var id: String?
    public var finished: Date
    public var elapsed: TimeInterval
    public var outcome: Outcome

    public init(id: String? = nil, finished: Date, elapsed: TimeInterval, outcome: Outcome) {
        self.id = id
        self.finished = finished
        self.elapsed = elapsed
        self.outcome = outcome
    }

    /// The timing a replayed reply carries in `_meta.rustcode.turn`. Nil for
    /// a turn that failed or never ended.
    public init?(json: JSONValue?) {
        guard let json, let outcome = json["outcome"]?.stringValue.flatMap(Outcome.init(rawValue:)),
              let finished = json["endedAt"]?.stringValue.flatMap(Self.date) else { return nil }
        let started = json["startedAt"]?.stringValue.flatMap(Self.date)
        // The engine measures work on its own clock, without time spent
        // waiting on the user; the timestamps are the fallback.
        guard let elapsed = json["elapsedMs"]?.intValue.map({ Double($0) / 1000 })
                ?? started.map({ finished.timeIntervalSince($0) }) else { return nil }
        self.init(id: json["id"]?.stringValue, finished: finished, elapsed: max(elapsed, 0), outcome: outcome)
    }

    private static func date(_ text: String) -> Date? {
        let fractional = ISO8601DateFormatter()
        fractional.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return fractional.date(from: text) ?? ISO8601DateFormatter().date(from: text)
    }

    /// "1m 12s · 14:32", the finish time with its date when it was not
    /// today; a stopped turn says "Stopped after 8s".
    public func label(now: Date = Date(), locale: Locale = .current, calendar: Calendar = .current,
                      timeZone: TimeZone = .current) -> String {
        let took = Self.duration(elapsed)
        guard outcome == .completed else { return "Stopped after \(took)" }
        var calendar = calendar
        calendar.timeZone = timeZone
        var style = Date.FormatStyle(date: .omitted, time: .shortened, locale: locale, calendar: calendar, timeZone: timeZone)
        if !calendar.isDate(finished, inSameDayAs: now) {
            style = style.day().month(.abbreviated)
            if !calendar.isDate(finished, equalTo: now, toGranularity: .year) { style = style.year() }
        }
        return "\(took) · \(finished.formatted(style))"
    }

    /// "12s", "1m 12s", "1h 3m": how long a turn took.
    public static func duration(_ elapsed: TimeInterval) -> String {
        let seconds = max(Int(elapsed.rounded()), 1)
        if seconds < 60 { return "\(seconds)s" }
        if seconds < 3600 { return seconds % 60 == 0 ? "\(seconds / 60)m" : "\(seconds / 60)m \(seconds % 60)s" }
        let minutes = seconds / 60 % 60
        return minutes == 0 ? "\(seconds / 3600)h" : "\(seconds / 3600)h \(minutes)m"
    }

    /// "00:12", "12:05", "1:02:03": a running turn's clock.
    public static func clock(_ elapsed: TimeInterval) -> String {
        let seconds = max(Int(elapsed), 0)
        let (hours, minutes, rest) = (seconds / 3600, seconds / 60 % 60, seconds % 60)
        return hours > 0
            ? String(format: "%d:%02d:%02d", hours, minutes, rest)
            : String(format: "%02d:%02d", minutes, rest)
    }
}
