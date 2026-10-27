import XCTest
@testable import RustCodeKit

final class SessionRetentionTests: XCTestCase {
    private let now = Date(timeIntervalSince1970: 2_000_000_000)
    private let day: TimeInterval = 24 * 60 * 60

    private func chat(_ id: String, idle days: Double?) -> SessionSummary {
        SessionSummary(id: id, cwd: "/p", updatedAt: days.map { now.addingTimeInterval(-$0 * day) })
    }

    func testArchivesOnlyChatsIdleAWeek() {
        let plan = SessionRetention.plan(sessions: [chat("recent", idle: 6.9), chat("old", idle: 7), chat("unknown", idle: nil)],
                                         archived: [:], pinned: [], inUse: [], now: now)
        XCTAssertEqual(plan.toArchive, ["old"])
        XCTAssertEqual(plan.toTrash, [])
    }

    func testNeverTouchesPinnedOrInUseChats() {
        let sessions = [chat("pinned", idle: 90), chat("running", idle: 90), chat("pinnedArchived", idle: 90)]
        let plan = SessionRetention.plan(sessions: sessions, archived: ["pinnedArchived": now.addingTimeInterval(-60 * day)],
                                         pinned: ["pinned", "pinnedArchived"], inUse: ["running"], now: now)
        XCTAssertEqual(plan, SessionRetention())
    }

    func testTrashesArchivedChatsAMonthAfterTheLaterOfArchiveAndActivity() {
        let sessions = [chat("expired", idle: 45), chat("young", idle: 45), chat("usedSince", idle: 10)]
        let archived = ["expired": now.addingTimeInterval(-30 * day),
                        "young": now.addingTimeInterval(-29 * day),
                        "usedSince": now.addingTimeInterval(-40 * day)]
        let plan = SessionRetention.plan(sessions: sessions, archived: archived, pinned: [], inUse: [], now: now)
        XCTAssertEqual(plan.toTrash, ["expired"])
        XCTAssertEqual(plan.toArchive, [])
    }

    func testMigrationDatesUndatedArchivesNowSoNothingExpiresOnTheFirstSweep() {
        let earlier = now.addingTimeInterval(-90 * day)
        let dated = SessionRetention.dated(["old", "dated"], dates: ["dated": earlier, "gone": earlier], now: now)
        XCTAssertEqual(dated, ["old": now, "dated": earlier])
        XCTAssertEqual(SessionRetention.dated(["old", "dated"], dates: dated, now: now.addingTimeInterval(day)), dated)
        let plan = SessionRetention.plan(sessions: [chat("old", idle: 400)], archived: dated, pinned: [], inUse: [], now: now)
        XCTAssertEqual(plan, SessionRetention())
    }
}
