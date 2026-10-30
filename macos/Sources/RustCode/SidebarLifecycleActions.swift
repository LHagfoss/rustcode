import Foundation
import RustCodeKit

extension SidebarLifecycle {
    static let defaultsKey = "sidebarLifecycle"
}

/// Done and Snooze from the sidebar (#2634, #2635), and the attention groups
/// (#2633). These move rows only: nothing here stops a turn, answers a
/// request or touches a chat's folder.
@MainActor
extension DesktopModel {
    static let groupsByAttentionKey = "sidebarGroupsByAttention"

    /// What the sidebar has observed of a chat right now.
    func attention(of id: String) -> SidebarAttention {
        .of(needsAnswer: permissionQueue.contains { $0.sessionId == id },
            isRunning: runningSessions.contains(id),
            hasUnseenReply: unseenReplies.contains(id))
    }

    /// The visible chats in their sidebar groups.
    func sidebarGroups(byAttention: Bool, now: Date = Date()) -> SidebarGroups {
        SidebarGroups(ids: visibleSessions.map(\.id), pinned: pinnedSessions, lifecycle: lifecycle,
                             now: now, byAttention: byAttention && !showsArchived, attention: attention(of:))
    }

    func markDone(_ session: SessionSummary) {
        if let refusal = SidebarLifecycle.refusal(attention(of: session.id)) {
            showToast(Toast(style: .warning, systemImage: "checkmark.circle", title: "Not marked done", message: refusal))
            return
        }
        lifecycle.markDone(session.id, at: Date())
        confirm("Marked done. It stays on the Done shelf until you reopen it.", systemImage: "checkmark.circle.fill")
    }

    func reopen(_ session: SessionSummary) {
        lifecycle.reopen(session.id)
        lifecycle.wakeNow(session.id)
        confirm("Reopened", systemImage: "arrow.uturn.backward.circle.fill")
    }

    func snooze(_ session: SessionSummary, until wake: Date) {
        if let refusal = SidebarLifecycle.refusal(attention(of: session.id)) {
            showToast(Toast(style: .warning, systemImage: "moon.zzz", title: "Not snoozed", message: refusal))
            return
        }
        lifecycle.snooze(session.id, until: wake)
        confirm("Snoozed until \(SnoozeChoice.describe(wake))", systemImage: "moon.zzz.fill")
    }

    func wakeNow(_ session: SessionSummary) {
        lifecycle.wakeNow(session.id)
        confirm("Woken", systemImage: "sun.max.fill")
    }

    /// Bring back snoozes that are due and done or snoozed chats with new
    /// activity. Returns the next wake, for the sidebar's timer.
    @discardableResult
    func settleLifecycle(now: Date = Date()) -> Date? {
        var next = lifecycle
        next.settle(now: now, attention: attention(of:))
        if next != lifecycle { lifecycle = next }
        return lifecycle.nextWake(after: now)
    }
}

/// The snooze lengths the sidebar offers.
enum SnoozeChoice: CaseIterable, Identifiable {
    case oneHour, threeHours, tomorrow, nextWeek

    var id: Self { self }

    var title: String {
        switch self {
        case .oneHour: "For 1 Hour"
        case .threeHours: "For 3 Hours"
        case .tomorrow: "Until Tomorrow Morning"
        case .nextWeek: "Until Next Week"
        }
    }

    /// The wake instant, in the reader's calendar and time zone: tomorrow
    /// and next week wake at 9:00 local time.
    func wake(from now: Date, calendar: Calendar = .current) -> Date {
        switch self {
        case .oneHour: return now.addingTimeInterval(3600)
        case .threeHours: return now.addingTimeInterval(3 * 3600)
        case .tomorrow, .nextWeek:
            let days = self == .tomorrow ? 1 : 7
            let day = calendar.date(byAdding: .day, value: days, to: calendar.startOfDay(for: now)) ?? now
            return calendar.date(bySettingHour: 9, minute: 0, second: 0, of: day) ?? day
        }
    }

    static func describe(_ date: Date, now: Date = Date(), calendar: Calendar = .current) -> String {
        let time = date.formatted(date: .omitted, time: .shortened)
        if calendar.isDate(date, inSameDayAs: now) { return time }
        if let tomorrow = calendar.date(byAdding: .day, value: 1, to: now), calendar.isDate(date, inSameDayAs: tomorrow) {
            return "tomorrow \(time)"
        }
        return date.formatted(.dateTime.weekday(.abbreviated).day().month(.abbreviated).hour().minute())
    }
}
