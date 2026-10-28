import Foundation

/// A subscription plan's usage windows, as `_rustcode/usage/limits` answers
/// (#2545): the short window (5 hours) and the long one (a week), each with
/// how much is used and when it resets.
public struct PlanUsage: Equatable, Sendable {
    public struct Window: Equatable, Sendable, Identifiable {
        public var usedPercent: Double
        public var minutes: Int?
        public var resetsAt: Date?

        public var id: String { title }
        public var leftPercent: Int { max(0, min(100, Int((100 - usedPercent).rounded()))) }
        public var leftFraction: Double { Double(leftPercent) / 100 }

        /// `5-hour`, `Weekly`, or the window's length.
        public var title: String {
            switch minutes {
            case 10080: "Weekly"
            case let minutes? where minutes % 1440 == 0: "\(minutes / 1440)-day"
            case let minutes? where minutes % 60 == 0: "\(minutes / 60)-hour"
            case let minutes?: "\(minutes)-minute"
            case nil: "Usage"
            }
        }

        /// `58% left · resets 14:30`, or a weekday for a reset days away.
        public func detail(now: Date = Date(), calendar: Calendar = .current, locale: Locale = .current) -> String {
            guard let resetsAt else { return "\(leftPercent)% left" }
            let style: Date.FormatStyle = calendar.isDate(resetsAt, inSameDayAs: now)
                || resetsAt.timeIntervalSince(now) < 12 * 3600
                ? .dateTime.hour().minute()
                : .dateTime.weekday(.abbreviated).hour().minute()
            var format = style
            format.calendar = calendar
            format.locale = locale
            format.timeZone = calendar.timeZone
            return "\(leftPercent)% left · resets \(resetsAt.formatted(format))"
        }
    }

    /// Where the numbers come from: `codex` (the Codex CLI's report for the
    /// ChatGPT plan) or `provider` (sent with the provider's responses).
    public var source: String
    public var windows: [Window]

    public init?(json: JSONValue?) {
        guard let json, case .object = json else { return nil }
        func window(_ key: String) -> Window? {
            guard let value = json[key], let used = value["usedPercent"]?.doubleValue else { return nil }
            return Window(usedPercent: used,
                          minutes: value["windowMinutes"]?.intValue,
                          resetsAt: value["resetsAt"]?.doubleValue.map { Date(timeIntervalSince1970: $0) })
        }
        source = json["source"]?.stringValue ?? "provider"
        windows = [window("primary"), window("secondary")].compactMap { $0 }
        if windows.isEmpty { return nil }
    }

    /// The meter's tooltip: one line per window.
    public func summary(now: Date = Date()) -> String {
        let lines = windows.map { "\($0.title): \($0.detail(now: now))" }
        return (lines + (source == "codex" ? ["As the Codex CLI reports for your ChatGPT plan"] : []))
            .joined(separator: "\n")
    }
}

extension ACPMethod {
    public static let usageLimits = "_rustcode/usage/limits"
}

extension ACPRequests {
    public static func usageLimits(sessionId: String) -> JSONValue {
        ["sessionId": .string(sessionId)]
    }
}
