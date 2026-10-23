import XCTest
@testable import RustCodeKit

final class TimelineMarkersTests: XCTestCase {
    private func turns(_ count: Int) -> [TimelineTurn] {
        (0..<count).map { TimelineTurn(id: $0 * 10, title: "Turn \($0)") }
    }

    func testTurnsAreTheUserMessages() {
        var transcript = Transcript()
        transcript.apply(.userText("Fix   the\nparser please"))
        transcript.apply(.agentText("Done."))
        transcript.apply(.userText("Now the tests"))
        let turns = TimelineTurn.turns(in: transcript.items)
        XCTAssertEqual(turns.map(\.title), ["Fix the parser please", "Now the tests"])
        XCTAssertEqual(turns.map(\.id), [transcript.items[0].id, transcript.items[2].id])
    }

    func testTitleIsCutAtAWord() {
        XCTAssertEqual(TimelineTurn.title("one two three four", limit: 10), "one two…")
        XCTAssertEqual(TimelineTurn.title("abcdefghijkl", limit: 5), "abcde…")
        XCTAssertEqual(TimelineTurn.title("  \n "), "Image")
    }

    func testMarkersInViewSitLevelWithTheirMessage() {
        let layout = TimelineLayout(turns: turns(3), tops: [100, 400, 700], offset: 50, visible: 800, follows: false)
        XCTAssertEqual(layout.markers.map(\.y), [66, 366, 666])
        XCTAssertTrue(layout.markers.allSatisfy(\.isInView))
    }

    func testTurnsOutOfViewStackAtTheEndsNearestInnermost() {
        // Eight turns above the view, one in it, two below.
        let tops: [Double?] = [nil, nil] + (0..<6).map { Double($0) * 100 } + [1000, 2000, 2100]
        let layout = TimelineLayout(turns: turns(11), tops: tops, offset: 1000, visible: 500, follows: false)
        let above = layout.markers.filter { !$0.isInView && $0.y < 250 }
        let below = layout.markers.filter { !$0.isInView && $0.y > 250 }
        XCTAssertEqual(above.count, TimelineLayout.stackLimit)
        XCTAssertEqual(above.map(\.number), [4, 5, 6, 7, 8])
        // Nearest above is the lowest of the top stack.
        XCTAssertEqual(above.map(\.y), above.map(\.y).sorted())
        XCTAssertEqual(above.first?.y, TimelineLayout.margin)
        XCTAssertEqual(below.map(\.number), [10, 11])
        XCTAssertEqual(below.last?.y, 500 - TimelineLayout.margin)
        XCTAssertLessThan(below[0].y, below[1].y)
        // The one in view keeps clear of the top stack.
        let inView = layout.markers.filter(\.isInView)
        XCTAssertEqual(inView.map(\.number), [9])
        XCTAssertEqual(inView[0].y, TimelineLayout.margin + 5 * TimelineLayout.stackStep)
    }

    func testCurrentTurnIsTheOneBeingRead() {
        let tops: [Double] = [0, 500, 1000, 1500]
        let reading = TimelineLayout(turns: turns(4), tops: tops, offset: 900, visible: 600, follows: false)
        // Turn 3 starts at 100 in the view, above the reading line at 180.
        XCTAssertEqual(reading.current, 20)
        XCTAssertEqual(reading.markers.filter(\.isCurrent).map(\.id), [20])
        let top = TimelineLayout(turns: turns(4), tops: tops, offset: 0, visible: 600, follows: false)
        XCTAssertEqual(top.current, 0)
        let following = TimelineLayout(turns: turns(4), tops: tops, offset: 0, visible: 600, follows: true)
        XCTAssertEqual(following.current, 30)
    }

    func testMarkersRecedeWithDistanceFromTheCurrentTurn() {
        let tops = (0..<6).map { Double($0) * 1000 }
        let layout = TimelineLayout(turns: turns(6), tops: tops, offset: 0, visible: 2000, follows: false)
        XCTAssertEqual(layout.markers.count, 6)
        XCTAssertEqual(layout.current, 0)
        let scales = layout.markers.map(\.scale)
        let opacities = layout.markers.map(\.opacity)
        XCTAssertEqual(scales.first, 1)
        XCTAssertEqual(scales, scales.sorted(by: >))
        XCTAssertEqual(opacities, opacities.sorted(by: >))
        XCTAssertGreaterThan(scales.last ?? 0, 0)
    }

    func testRailShowsOnlyWithRoomAndTurns() {
        // A 760-point column centred in 848 leaves 44 points.
        XCTAssertTrue(TimelineLayout.shows(width: 848, column: 760, padding: 28, turns: 2))
        XCTAssertFalse(TimelineLayout.shows(width: 840, column: 760, padding: 28, turns: 2))
        XCTAssertFalse(TimelineLayout.shows(width: 1400, column: 760, padding: 28, turns: 1))
        XCTAssertEqual(TimelineLayout.gutter(width: 700, column: 760, padding: 28), 28)
    }

    func testLayoutDiffersOnlyWhenTheRailWouldMove() {
        let tops: [Double] = [100, 400]
        let layout = TimelineLayout(turns: turns(2), tops: tops, offset: 0, visible: 800, follows: false)
        let nudged = TimelineLayout(turns: turns(2), tops: tops, offset: 0.1, visible: 800, follows: false)
        let scrolled = TimelineLayout(turns: turns(2), tops: tops, offset: 30, visible: 800, follows: false)
        XCTAssertFalse(nudged.differs(from: layout))
        XCTAssertTrue(scrolled.differs(from: layout))
        XCTAssertTrue(TimelineLayout.empty.differs(from: layout))
    }

    func testMapGivesTheTopsOfTurns() {
        var map = ChatScrollMap(spacing: 10, topPadding: 20, bottomPadding: 5)
        map.setRows([1, 2, 3])
        map.measured(1, height: 100)
        map.measured(2, height: 50)
        XCTAssertEqual(map.tops(of: [3, 9, 1]), [190, nil, 20])
        XCTAssertEqual(map.tops(of: []), [])
    }
}
