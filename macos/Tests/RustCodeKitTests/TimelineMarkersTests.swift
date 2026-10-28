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

    func testTicksStackCloseTogetherCentredOnTheRail() {
        let layout = TimelineLayout(turns: turns(3), tops: [100, 400, 900], offset: 50, visible: 800, follows: false)
        let step = TimelineLayout.step
        XCTAssertEqual(layout.markers.map(\.y), [400 - step, 400, 400 + step])
        XCTAssertEqual(layout.markers.map(\.isInView), [true, true, false])
    }

    func testRailShowsTheTurnsAroundTheCurrentOneWhenTheyDoNotFit() {
        // 100 points fit 11 ticks: 84 between the margins over 8-point steps.
        let tops = (0..<40).map { Double($0) * 1000 }
        let layout = TimelineLayout(turns: turns(40), tops: tops, offset: 20_000, visible: 100, follows: false)
        XCTAssertEqual(layout.current, 200)
        XCTAssertEqual(layout.markers.map(\.number), Array(16...26))
        let end = TimelineLayout(turns: turns(40), tops: tops, offset: 0, visible: 100, follows: true)
        XCTAssertEqual(end.markers.map(\.number), Array(30...40))
        XCTAssertGreaterThanOrEqual(end.markers.first?.y ?? 0, TimelineLayout.margin)
        XCTAssertLessThanOrEqual(end.markers.last?.y ?? .infinity, 100 - TimelineLayout.margin)
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

    func testLayoutDiffersOnlyWhenTheRailWouldMove() {
        let tops: [Double] = [100, 400]
        let layout = TimelineLayout(turns: turns(2), tops: tops, offset: 0, visible: 800, follows: false)
        let nudged = TimelineLayout(turns: turns(2), tops: tops, offset: 0.1, visible: 800, follows: false)
        let scrolled = TimelineLayout(turns: turns(2), tops: tops, offset: 300, visible: 800, follows: false)
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
