import RustCodeKit
import XCTest

final class TranscriptSelectionTests: XCTestCase {
    private typealias Run = TranscriptSelection.Run
    private typealias Position = TranscriptSelection.Position

    private func at(_ item: Int, _ segment: Int, _ offset: Int) -> Position {
        Position(item: item, segment: segment, offset: offset)
    }

    func testPositionsOrderByItemThenSegmentThenOffset() {
        XCTAssertLessThan(at(1, 5, 90), at(2, 0, 0))
        XCTAssertLessThan(at(2, 0, 90), at(2, 1, 0))
        XCTAssertLessThan(at(2, 1, 3), at(2, 1, 4))
        XCTAssertEqual([at(3, 0, 0), at(1, 2, 0), at(1, 0, 7), at(1, 0, 2)].sorted(),
                       [at(1, 0, 2), at(1, 0, 7), at(1, 2, 0), at(3, 0, 0)])
    }

    func testANewSelectionIsEmpty() {
        var selection = TranscriptSelection()
        XCTAssertTrue(selection.isEmpty)
        XCTAssertNil(selection.bounds)
        selection.begin(at: at(1, 0, 4))
        XCTAssertTrue(selection.isEmpty, "a press alone selects nothing")
        XCTAssertNil(selection.range(in: Run(item: 1, segment: 0), length: 10))
        selection.extend(to: at(1, 0, 6))
        XCTAssertFalse(selection.isEmpty)
    }

    func testExtendWithoutAPressStartsThere() {
        var selection = TranscriptSelection()
        selection.extend(to: at(1, 0, 3))
        XCTAssertEqual(selection.anchor, at(1, 0, 3))
        XCTAssertTrue(selection.isEmpty)
    }

    func testBoundsAreOrderedWhicheverWayTheDragWent() {
        let down = TranscriptSelection(from: at(1, 0, 2), to: at(3, 1, 5))
        let up = TranscriptSelection(from: at(3, 1, 5), to: at(1, 0, 2))
        XCTAssertEqual(down.bounds?.start, at(1, 0, 2))
        XCTAssertEqual(down.bounds?.end, at(3, 1, 5))
        XCTAssertEqual(up.bounds?.start, at(1, 0, 2))
        XCTAssertEqual(up.bounds?.end, at(3, 1, 5))
        XCTAssertEqual(up.anchor, at(3, 1, 5), "the anchor stays where the drag began")
    }

    func testWithinOneRun() {
        let selection = TranscriptSelection(from: at(4, 2, 9), to: at(4, 2, 3))
        XCTAssertEqual(selection.range(in: Run(item: 4, segment: 2), length: 20), 3..<9)
        XCTAssertNil(selection.range(in: Run(item: 4, segment: 1), length: 20))
        XCTAssertNil(selection.range(in: Run(item: 4, segment: 3), length: 20))
        XCTAssertNil(selection.range(in: Run(item: 5, segment: 2), length: 20))
    }

    func testAcrossSegmentsOfOneItem() {
        let selection = TranscriptSelection(from: at(4, 0, 5), to: at(4, 2, 3))
        XCTAssertEqual(selection.range(in: Run(item: 4, segment: 0), length: 12), 5..<12)
        XCTAssertEqual(selection.range(in: Run(item: 4, segment: 1), length: 30), 0..<30, "a run between shows whole")
        XCTAssertEqual(selection.range(in: Run(item: 4, segment: 2), length: 8), 0..<3)
        XCTAssertNil(selection.range(in: Run(item: 4, segment: 3), length: 8))
    }

    func testAcrossItems() {
        let selection = TranscriptSelection(from: at(9, 1, 4), to: at(2, 0, 6))
        XCTAssertNil(selection.range(in: Run(item: 1, segment: 0), length: 10))
        XCTAssertEqual(selection.range(in: Run(item: 2, segment: 0), length: 10), 6..<10)
        XCTAssertEqual(selection.range(in: Run(item: 2, segment: 1), length: 10), 0..<10)
        XCTAssertEqual(selection.range(in: Run(item: 5, segment: 3), length: 7), 0..<7)
        XCTAssertEqual(selection.range(in: Run(item: 9, segment: 0), length: 7), 0..<7)
        XCTAssertEqual(selection.range(in: Run(item: 9, segment: 1), length: 7), 0..<4)
        XCTAssertNil(selection.range(in: Run(item: 9, segment: 2), length: 7))
        XCTAssertTrue(selection.contains(Run(item: 5, segment: 0)))
        XCTAssertFalse(selection.contains(Run(item: 10, segment: 0)))
    }

    func testEndsAtARunsEdgesShowNothingThere() {
        // Ending at offset 0 of the next run, or starting at the end of a
        // run, leaves that run unhighlighted.
        let selection = TranscriptSelection(from: at(1, 0, 10), to: at(2, 0, 0))
        XCTAssertNil(selection.range(in: Run(item: 1, segment: 0), length: 10))
        XCTAssertNil(selection.range(in: Run(item: 2, segment: 0), length: 10))
        XCTAssertTrue(selection.contains(Run(item: 1, segment: 0)))
    }

    func testOffsetsPastAShorterTextAreHeldToItsLength() {
        let selection = TranscriptSelection(from: at(1, 0, 2), to: at(1, 0, 50))
        XCTAssertEqual(selection.range(in: Run(item: 1, segment: 0), length: 6), 2..<6)
        let beyond = TranscriptSelection(from: at(1, 0, 20), to: at(1, 0, 50))
        XCTAssertNil(beyond.range(in: Run(item: 1, segment: 0), length: 6))
    }

    func testClear() {
        var selection = TranscriptSelection(from: at(1, 0, 0), to: at(3, 0, 4))
        selection.clear()
        XCTAssertTrue(selection.isEmpty)
        XCTAssertNil(selection.anchor)
        XCTAssertNil(selection.focus)
        XCTAssertFalse(selection.contains(Run(item: 2, segment: 0)))
        XCTAssertEqual(selection, TranscriptSelection())
    }

    func testCopyTextJoinsTheSelectedPartsInChatOrder() {
        let selection = TranscriptSelection(from: at(3, 0, 4), to: at(1, 0, 6))
        let runs: [(run: Run, text: String)] = [
            (Run(item: 3, segment: 0), "End of it all"),
            (Run(item: 1, segment: 0), "First paragraph"),
            (Run(item: 0, segment: 0), "Before"),
            (Run(item: 2, segment: 1), "Second part"),
            (Run(item: 2, segment: 0), "Middle"),
            (Run(item: 4, segment: 0), "After"),
        ]
        XCTAssertEqual(selection.text(of: runs), "paragraph\n\nMiddle\n\nSecond part\n\nEnd ")
    }

    func testCopyTextCountsUTF16LikeNSRange() {
        // The emoji is two UTF-16 units, as NSRange counts it.
        let text = "a 👋 b"
        let selection = TranscriptSelection(from: at(1, 0, 2), to: at(1, 0, 4))
        XCTAssertEqual(selection.text(of: [(Run(item: 1, segment: 0), text)]), "👋")
        XCTAssertEqual((text as NSString).substring(with: NSRange(location: 2, length: 2)), "👋")
    }

    func testCopyTextOfNothingIsEmpty() {
        XCTAssertEqual(TranscriptSelection().text(of: [(Run(item: 1, segment: 0), "text")]), "")
        let elsewhere = TranscriptSelection(from: at(5, 0, 0), to: at(5, 0, 3))
        XCTAssertEqual(elsewhere.text(of: [(Run(item: 1, segment: 0), "text")]), "")
    }
}
