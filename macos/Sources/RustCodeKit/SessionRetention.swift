import Foundation

/// When the sidebar tidies itself: an unpinned chat idle for a week moves to
/// the archive, and an archived chat left there a month moves to the Trash,
/// where it can still be put back (#2489). Pinned chats and chats in use are
/// never touched.
public struct SessionRetention: Equatable, Sendable {
    public static let archiveAfter: TimeInterval = 7 * 24 * 60 * 60
    public static let trashAfter: TimeInterval = 30 * 24 * 60 * 60
    /// How often the sweep runs at most.
    public static let sweepInterval: TimeInterval = 24 * 60 * 60

    public var toArchive: [String] = []
    public var toTrash: [String] = []

    public init(toArchive: [String] = [], toTrash: [String] = []) {
        self.toArchive = toArchive
        self.toTrash = toTrash
    }

    /// What to move, given every listed chat and when each archived chat was
    /// archived. Ages count from last activity (`updatedAt`), never creation;
    /// a chat with no known activity is left alone. An archived chat expires
    /// a month after the later of its archiving and its last activity, so a
    /// chat archived without a date, or used after archiving, starts over.
    public static func plan(
        sessions: [SessionSummary],
        archived: [String: Date],
        pinned: Set<String>,
        inUse: Set<String>,
        now: Date
    ) -> SessionRetention {
        var plan = SessionRetention()
        for session in sessions where !pinned.contains(session.id) && !inUse.contains(session.id) {
            guard let active = session.updatedAt else { continue }
            if let archivedAt = archived[session.id] {
                if now.timeIntervalSince(max(archivedAt, active)) >= trashAfter { plan.toTrash.append(session.id) }
            } else if now.timeIntervalSince(active) >= archiveAfter {
                plan.toArchive.append(session.id)
            }
        }
        return plan
    }

    /// Archive dates for the archived chats, with a chat archived before
    /// dates were kept dated `now`: none of them expires sooner than a month
    /// after this release, and repeating it changes nothing.
    public static func dated(_ archived: Set<String>, dates: [String: Date], now: Date) -> [String: Date] {
        Dictionary(uniqueKeysWithValues: archived.map { ($0, dates[$0] ?? now) })
    }
}
