import XCTest
@testable import RustCodeKit

final class ChatScrollMapTests: XCTestCase {
    private func map(_ heights: [Int: Double], rows: [Int]) -> ChatScrollMap {
        var map = ChatScrollMap(spacing: 10, topPadding: 20, bottomPadding: 5)
        map.setRows(rows)
        for (row, height) in heights { map.measured(row, height: height) }
        return map
    }

    func testLaysOutMeasuredRowsWithGaps() {
        var map = map([1: 100, 2: 50, 3: 30], rows: [1, 2, 3])
        XCTAssertEqual(map.top(of: 1), 20)
        XCTAssertEqual(map.top(of: 2), 130)
        XCTAssertEqual(map.top(of: 3), 190)
        XCTAssertEqual(map.contentHeight(), 225)
    }

    func testUnmeasuredRowsTakeTheAverage() {
        var map = map([1: 100, 3: 50], rows: [1, 2, 3])
        XCTAssertEqual(map.estimate, 75)
        XCTAssertEqual(map.top(of: 3), 20 + 100 + 10 + 75 + 10)
    }

    func testMeasurementsOutliveRowsLeavingTheScreen() {
        var map = map([1: 100, 2: 400], rows: [1, 2])
        let height = map.contentHeight()
        // A lazy stack drops rows it scrolled past; their heights stay known.
        map.setRows([1, 2, 3])
        map.setRows([1, 2])
        XCTAssertEqual(map.contentHeight(), height)
        XCTAssertEqual(map.height(of: 2), 400)
    }

    func testRevisionChangesOnlyWithTheLayout() {
        var map = map([1: 100], rows: [1])
        let revision = map.revision
        map.measured(1, height: 100.2)
        map.setRows([1])
        XCTAssertEqual(map.revision, revision)
        map.measured(1, height: 120)
        XCTAssertGreaterThan(map.revision, revision)
    }

    func testFindsTheRowAtAPosition() {
        var map = map([1: 100, 2: 50], rows: [1, 2])
        XCTAssertEqual(map.row(at: 0)?.row, 1)
        XCTAssertEqual(map.row(at: 140)?.row, 2)
        XCTAssertEqual(map.row(at: 140)?.offset, 10)
        XCTAssertEqual(map.row(at: 10_000)?.row, 2)
    }

    func testKnobSpansTheTrackFromTopToEnd() throws {
        let top = try XCTUnwrap(ChatScrollerKnob(offset: 0, contentHeight: 2000, visible: 500, track: 400))
        XCTAssertEqual(top.top, 0)
        XCTAssertEqual(top.height, 100)
        let end = try XCTUnwrap(ChatScrollerKnob(offset: 1500, contentHeight: 2000, visible: 500, track: 400))
        XCTAssertEqual(end.top, 300)
        XCTAssertNil(ChatScrollerKnob(offset: 0, contentHeight: 400, visible: 500, track: 400))
        XCTAssertEqual(ChatScrollerKnob(offset: 0, contentHeight: 100_000, visible: 500, track: 400)?.height,
                       ChatScrollerKnob.minimumHeight)
    }

    func testDraggingTheKnobMapsBackToAnOffset() {
        let offset = ChatScrollerKnob.offset(forKnobTop: 150, knobHeight: 100, contentHeight: 2000,
                                             visible: 500, track: 400)
        XCTAssertEqual(offset, 750)
    }

    /// With every height known, the knob is exactly where the map puts it.
    func testFollowingWithAStableMapIsExact() throws {
        let start = try XCTUnwrap(ChatScrollerKnob(offset: 800, contentHeight: 2000, visible: 500, track: 400))
        let target = try XCTUnwrap(ChatScrollerKnob(offset: 700, contentHeight: 2000, visible: 500, track: 400))
        let next = start.following(target, moved: -100, offset: 700, contentHeight: 2000, visible: 500, track: 400)
        XCTAssertEqual(next.top, target.top, accuracy: 0.001)
    }

    /// A row above measured taller than guessed moves the map, not the chat:
    /// the knob holds still, then closes the gap as the chat scrolls.
    func testMeasuredRowDoesNotMoveTheKnob() throws {
        let knob = ChatScrollerKnob(top: 200, height: 100)
        let shifted = ChatScrollerKnob(top: 260, height: 100)
        let still = knob.following(shifted, moved: 0, offset: 1200, contentHeight: 2600, visible: 500, track: 400)
        XCTAssertEqual(still.top, 200)
        let scrolled = still.following(shifted, moved: 250, offset: 1450, contentHeight: 2600, visible: 500, track: 400)
        XCTAssertGreaterThan(scrolled.top, 200)
        XCTAssertLessThan(abs(scrolled.top - shifted.top), 60)
    }

    func testNeverMovesAgainstTheScroll() {
        let knob = ChatScrollerKnob(top: 200, height: 100)
        // The map says the knob belongs far below, but the chat moved up.
        let target = ChatScrollerKnob(top: 280, height: 100)
        let next = knob.following(target, moved: -40, offset: 1000, contentHeight: 3000, visible: 500, track: 400)
        XCTAssertLessThanOrEqual(next.top, 200)
    }

    func testReachesEachEndWithTheChat() {
        let knob = ChatScrollerKnob(top: 40, height: 100)
        let atTop = ChatScrollerKnob(top: 0, height: 100)
        XCTAssertEqual(knob.following(atTop, moved: -30, offset: 0, contentHeight: 3000, visible: 500, track: 400), atTop)
        let atEnd = ChatScrollerKnob(top: 300, height: 100)
        XCTAssertEqual(knob.following(atEnd, moved: 30, offset: 2500, contentHeight: 3000, visible: 500, track: 400), atEnd)
    }
}
