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

    func testAnotherChatStartsWithoutTheLastChatsHeights() {
        // The last chat's rows 0...9 were tall replies; the next chat reuses
        // those ids for short rows not measured yet.
        var map = map(Dictionary(uniqueKeysWithValues: (0..<10).map { ($0, 2_000.0) }), rows: Array(0..<10))
        map.forgetHeights()
        map.setRows(Array(0..<10))
        XCTAssertNil(map.height(of: 3))
        XCTAssertEqual(map.top(of: 3), 20 + 3 * (ChatScrollMap.defaultEstimate + 10))
        // The rows the view is on are the ones drawn, not stand-ins.
        var drawn = DrawnRows()
        _ = drawn.update(map: &map, offset: 200, visible: 300)
        XCTAssertTrue(Set(map.rows(within: 200...500)).isSubset(of: drawn.ids))
    }

    func testUnmeasuredRowsTakeTheAverage() {
        var map = map([1: 100, 3: 50], rows: [1, 2, 3])
        XCTAssertEqual(map.estimate, 75)
        XCTAssertEqual(map.top(of: 3), 20 + 100 + 10 + 75 + 10)
    }

    /// #2481: one very long reply among short rows does not make every row
    /// not measured yet stand in at hundreds of points.
    func testOneLongReplyDoesNotInflateTheGuess() {
        let map = map([0: 124, 1: 42, 2: 1_791], rows: [0, 1, 2, 3])
        XCTAssertEqual(map.estimate, 124)
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

/// Older rows away from the view stand in at a height that changes only
/// when their row is measured (#2310).
final class DrawnRowsTests: XCTestCase {
    private func map(rows: Int, height: Double = 100) -> ChatScrollMap {
        var map = ChatScrollMap(spacing: 0, topPadding: 0, bottomPadding: 0)
        map.setRows(Array(0..<rows))
        for row in 0..<rows { map.measured(row, height: height) }
        return map
    }

    func testAStandInKeepsItsGuessUntilItsRowIsMeasured() {
        var map = ChatScrollMap(spacing: 0, topPadding: 0, bottomPadding: 0)
        map.setRows([0, 1, 2])
        map.measured(0, height: 100)
        XCTAssertEqual(map.standInHeight(of: 2), 100)
        // Another row measured moves the average, not the stand-in.
        map.measured(1, height: 400)
        XCTAssertEqual(map.standInHeight(of: 2), 100)
        XCTAssertEqual(map.top(of: 2), 500)
        XCTAssertEqual(map.laidOutHeight(of: 2), 100)
        map.measured(2, height: 30)
        XCTAssertEqual(map.standInHeight(of: 2), 30)
    }

    func testRowsWithinARange() {
        var map = map(rows: 10)
        XCTAssertEqual(map.rows(within: 250...420), [2, 3, 4])
        XCTAssertEqual(map.rows(within: -50...20), [0])
    }

    func testDrawsTheViewAndTwoScreensEitherSide() {
        var map = map(rows: 100)
        var drawn = DrawnRows()
        XCTAssertTrue(drawn.update(map: &map, offset: 5000, visible: 300))
        // 5000 - 600 ... 5300 + 600
        XCTAssertEqual(drawn.ids, Set(44...59))
    }

    /// Scrolling within the slack draws the same rows; nearer the edge it
    /// draws afresh around the view.
    func testRedrawsOnlyNearTheEdgeOfWhatIsDrawn() {
        var map = map(rows: 100)
        var drawn = DrawnRows()
        _ = drawn.update(map: &map, offset: 5000, visible: 300)
        XCTAssertFalse(drawn.update(map: &map, offset: 4800, visible: 300))
        XCTAssertTrue(drawn.update(map: &map, offset: 4500, visible: 300))
        XCTAssertTrue(drawn.ids.contains(39))
        drawn.invalidate()
        XCTAssertFalse(drawn.update(map: &map, offset: 4500, visible: 300), "same rows")
    }

    /// #2598: the chat's turn kept writing, then folded, while one of its
    /// agents was read in its place. Back at the chat, the rows in view are
    /// drawn at once, not left as stand-ins until a scroll.
    func testRowsInViewAreDrawnOnTheWayBackFromAnAgent() {
        var map = map(rows: 100)
        var drawn = DrawnRows()
        _ = drawn.update(map: &map, offset: 5000, visible: 300)
        // The turn's steps fold away and new rows arrive below.
        map.setRows(Array(0..<40) + Array(100..<130))
        drawn.invalidate()
        let end = map.contentHeight() - 300
        XCTAssertTrue(drawn.update(map: &map, offset: end, visible: 300))
        XCTAssertTrue(Set(map.rows(within: end...(end + 300))).isSubset(of: drawn.ids))
    }

    /// #2481: a turn opened with its rows guessed tall; once they measure
    /// short, rows below move into view and are drawn without a scroll.
    func testRowsMovedIntoViewByNewHeightsAreDrawn() {
        var map = map(rows: 20)
        var drawn = DrawnRows()
        _ = drawn.update(map: &map, offset: 0, visible: 300)
        XCTAssertEqual(drawn.ids, Set(0...9))
        for row in 0..<10 { map.measured(row, height: 10) }
        XCTAssertTrue(drawn.update(map: &map, offset: 0, visible: 300))
        // Rows 10... now start at 100, 200, …: drawn to two screens below.
        XCTAssertEqual(drawn.ids, Set(0...18))
    }

    /// Rows only leaving the view as heights change keep the set as it is.
    func testRowsMovedAwayByNewHeightsKeepTheSet() {
        var map = map(rows: 20)
        var drawn = DrawnRows()
        _ = drawn.update(map: &map, offset: 0, visible: 300)
        map.measured(0, height: 300)
        XCTAssertFalse(drawn.update(map: &map, offset: 0, visible: 300))
        XCTAssertEqual(drawn.ids, Set(0...9))
    }
}
