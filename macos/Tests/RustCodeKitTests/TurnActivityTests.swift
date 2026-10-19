import XCTest
@testable import RustCodeKit

final class TurnActivityTests: XCTestCase {
    func testCompletedTurnNoLongerRuns() {
        var activity = TurnActivity()
        let turn = activity.begin("a")
        XCTAssertTrue(activity.contains("a"))
        activity.end(turn)
        XCTAssertFalse(activity.contains("a"))
        XCTAssertEqual(activity.sessions, [])
    }

    func testChatRunsUntilItsLastTurnEnds() {
        var activity = TurnActivity()
        let first = activity.begin("a")
        let second = activity.begin("a")
        activity.end(first)
        XCTAssertTrue(activity.contains("a"))
        activity.end(second)
        XCTAssertFalse(activity.contains("a"))
    }

    func testTurnsInOtherChatsAreIndependent() {
        var activity = TurnActivity()
        let a = activity.begin("a")
        _ = activity.begin("b")
        activity.end(a)
        XCTAssertEqual(activity.sessions, ["b"])
    }

    func testTurnFromBeforeResetCannotEndNewTurn() {
        var activity = TurnActivity()
        let stale = activity.begin("a")
        activity.reset()
        XCTAssertFalse(activity.contains("a"))
        let fresh = activity.begin("a")
        activity.end(stale)
        XCTAssertTrue(activity.contains("a"))
        activity.end(fresh)
        XCTAssertFalse(activity.contains("a"))
    }

    func testEndingTwiceIsHarmless() {
        var activity = TurnActivity()
        let turn = activity.begin("a")
        activity.end(turn)
        activity.end(turn)
        XCTAssertEqual(activity, TurnActivity())
    }
}
