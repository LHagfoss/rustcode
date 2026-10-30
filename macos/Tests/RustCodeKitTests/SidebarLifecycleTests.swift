import XCTest
@testable import RustCodeKit

final class SidebarLifecycleTests: XCTestCase {
    private let now = Date(timeIntervalSince1970: 1_800_000_000)

    func testAttentionPrecedence() {
        XCTAssertEqual(SidebarAttention.of(needsAnswer: true, isRunning: true, hasUnseenReply: true), .needsYou)
        XCTAssertEqual(SidebarAttention.of(needsAnswer: false, isRunning: true, hasUnseenReply: true), .working)
        XCTAssertEqual(SidebarAttention.of(needsAnswer: false, isRunning: false, hasUnseenReply: true), .readyForReview)
        XCTAssertEqual(SidebarAttention.of(needsAnswer: false, isRunning: false, hasUnseenReply: false), .idle)
    }

    func testAChatWaitingOrWorkingCannotBePutAway() {
        XCTAssertNotNil(SidebarLifecycle.refusal(.needsYou))
        XCTAssertNotNil(SidebarLifecycle.refusal(.working))
        XCTAssertNil(SidebarLifecycle.refusal(.readyForReview))
        XCTAssertNil(SidebarLifecycle.refusal(.idle))
    }

    func testDoneAndSnoozeReplaceEachOtherAndReopenUndoes() {
        var lifecycle = SidebarLifecycle()
        lifecycle.markDone("a", at: now)
        XCTAssertTrue(lifecycle.isDone("a"))
        lifecycle.snooze("a", until: now + 60)
        XCTAssertFalse(lifecycle.isDone("a"))
        XCTAssertTrue(lifecycle.isSnoozed("a", now: now))
        lifecycle.markDone("a", at: now)
        XCTAssertFalse(lifecycle.isSnoozed("a", now: now))
        lifecycle.reopen("a")
        XCTAssertEqual(lifecycle, SidebarLifecycle())
    }

    func testSnoozeEndsOnTimeEarlyForAQuestionAndOnceOnly() {
        var lifecycle = SidebarLifecycle()
        lifecycle.snooze("due", until: now - 1)       // overdue after a restart
        lifecycle.snooze("later", until: now + 3600)
        lifecycle.snooze("asked", until: now + 3600)
        lifecycle.snooze("read", until: now + 3600)
        let attention: (String) -> SidebarAttention = { ["asked": .needsYou, "read": .readyForReview][$0] ?? .idle }

        XCTAssertEqual(lifecycle.settle(now: now, attention: attention), ["due", "asked"])
        XCTAssertEqual(lifecycle.settle(now: now, attention: attention), [], "a wake is reported once")
        XCTAssertTrue(lifecycle.isSnoozed("later", now: now))
        XCTAssertTrue(lifecycle.isSnoozed("read", now: now), "a reply to read does not wake a snooze")
        XCTAssertEqual(lifecycle.nextWake(after: now), now + 3600)
        lifecycle.wakeNow("later")
        XCTAssertFalse(lifecycle.isSnoozed("later", now: now))
    }

    func testNewActivityReopensADoneChat() {
        var lifecycle = SidebarLifecycle()
        lifecycle.markDone("quiet", at: now)
        lifecycle.markDone("busy", at: now)
        let back = lifecycle.settle(now: now) { $0 == "busy" ? .readyForReview : .idle }
        XCTAssertEqual(back, ["busy"])
        XCTAssertTrue(lifecycle.isDone("quiet"))
        XCTAssertFalse(lifecycle.isDone("busy"))
    }

    func testItSurvivesARelaunch() throws {
        var lifecycle = SidebarLifecycle()
        lifecycle.markDone("a", at: now)
        lifecycle.snooze("b", until: now + 60)
        let decoded = try JSONDecoder().decode(SidebarLifecycle.self, from: JSONEncoder().encode(lifecycle))
        XCTAssertEqual(decoded, lifecycle)
    }

    func testGroupsByAttentionPinsAndShelves() {
        var lifecycle = SidebarLifecycle()
        lifecycle.markDone("done", at: now)
        lifecycle.snooze("snoozed", until: now + 60)
        lifecycle.markDone("doneButAsked", at: now)
        let attention: (String) -> SidebarAttention = {
            ["asked": .needsYou, "doneButAsked": .needsYou, "running": .working, "replied": .readyForReview][$0] ?? .idle
        }
        let ids = ["idle", "asked", "pinned", "running", "done", "replied", "snoozed", "doneButAsked", "pinnedRunning"]
        let grouped = SidebarGroups(ids: ids, pinned: ["pinned", "pinnedRunning"], lifecycle: lifecycle, now: now,
                                    byAttention: true) { $0 == "pinnedRunning" ? .working : attention($0) }
        XCTAssertEqual(grouped.groups.map(\.kind), [.needsYou, .readyForReview, .working, .pinned, .chats, .snoozed, .done])
        XCTAssertEqual(grouped.groups.first { $0.kind == .needsYou }?.ids, ["asked", "doneButAsked"])
        XCTAssertEqual(grouped.groups.first { $0.kind == .working }?.ids, ["running", "pinnedRunning"])
        XCTAssertEqual(Set(grouped.ids), Set(ids), "every chat once")
        XCTAssertEqual(grouped.ids.count, ids.count)

        // Without grouping the list keeps its order; pins and shelves stay.
        let plain = SidebarGroups(ids: ids, pinned: ["pinned"], lifecycle: lifecycle, now: now,
                                  byAttention: false, attention: attention)
        XCTAssertEqual(plain.groups.map(\.kind), [.pinned, .chats, .snoozed, .done])
        XCTAssertEqual(plain.groups.first { $0.kind == .chats }?.ids,
                       ["idle", "asked", "running", "replied", "doneButAsked", "pinnedRunning"])
    }
}
