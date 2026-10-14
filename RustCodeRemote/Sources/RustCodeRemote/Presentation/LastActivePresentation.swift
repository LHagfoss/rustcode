import Foundation

/// A session row's last-activity label, in the style of a message list: the time
/// for today, "Yesterday", the weekday within the past week, then a short date.
public enum LastActivePresentation {
    public static func label(
        for date: Date,
        now: Date = .now,
        calendar: Calendar = .current,
        locale: Locale = .current
    ) -> String {
        if now.timeIntervalSince(date) < 60 { return "Now" }
        var style = Date.FormatStyle(locale: locale, calendar: calendar, timeZone: calendar.timeZone)
        if calendar.isDate(date, inSameDayAs: now) {
            return date.formatted(style.hour().minute())
        }
        if calendar.isDate(date, inSameDayAs: calendar.date(byAdding: .day, value: -1, to: now) ?? now) {
            return "Yesterday"
        }
        let startOfToday = calendar.startOfDay(for: now)
        if let weekAgo = calendar.date(byAdding: .day, value: -6, to: startOfToday), date >= weekAgo {
            return date.formatted(style.weekday(.wide))
        }
        style = calendar.isDate(date, equalTo: now, toGranularity: .year)
            ? style.day().month(.abbreviated)
            : style.day().month(.abbreviated).year()
        return date.formatted(style)
    }
}
