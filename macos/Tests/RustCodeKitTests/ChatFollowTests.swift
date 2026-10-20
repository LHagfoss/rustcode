import XCTest
@testable import RustCodeKit

final class ChatFollowTests: XCTestCase {
    private func at(_ offset: Double, distance: Double, height: Double = 600) -> ChatFollow.Geometry {
        ChatFollow.Geometry(offset: offset, containerHeight: height, distanceFromBottom: distance, canScroll: true)
    }

    /// A chat resting at its end, following.
    private func atEnd() -> ChatFollow {
        var follow = ChatFollow()
        _ = follow.update(at(1_000, distance: 0), readerScrolling: false)
        return follow
    }

    func testScrollingUpLetsGoAndShowsLatest() {
        var follow = atEnd()
        follow.readerScrolledUp()
        XCTAssertFalse(follow.follows)
        // Still within the threshold of the end: nowhere for Latest to go.
        XCTAssertFalse(follow.update(at(990, distance: 10), readerScrolling: true))
        XCTAssertFalse(follow.follows)
        XCTAssertFalse(follow.showsLatest)
        XCTAssertFalse(follow.update(at(700, distance: 300), readerScrolling: true))
        XCTAssertTrue(follow.showsLatest)
    }

    /// Streaming: a token lands between the reader's scroll events, so the
    /// geometry changes without moving while the chat is still near its end.
    func testOutputBetweenScrollEventsDoesNotPullTheReaderBack() {
        var follow = atEnd()
        follow.readerScrolledUp()
        _ = follow.update(at(990, distance: 10), readerScrolling: true)
        // The reply grew; the offset stayed put.
        XCTAssertFalse(follow.update(at(990, distance: 16), readerScrolling: true))
        XCTAssertFalse(follow.follows)
        XCTAssertFalse(follow.transcriptChanged(readerScrolling: true))
        // Still anchored to the end, the growth even moves the chat down.
        _ = follow.update(at(996, distance: 4), readerScrolling: true, towardEnd: false)
        XCTAssertFalse(follow.follows)
        _ = follow.update(at(960, distance: 50), readerScrolling: true)
        XCTAssertFalse(follow.follows)
        XCTAssertTrue(follow.showsLatest)
    }

    func testTheBounceAtTheEndKeepsFollowing() {
        var follow = atEnd()
        // Momentum down overshoots the end and springs back up.
        _ = follow.update(at(1_030, distance: -30), readerScrolling: true, towardEnd: true)
        _ = follow.update(at(1_010, distance: -10), readerScrolling: true, towardEnd: true)
        XCTAssertTrue(follow.follows)
        _ = follow.update(at(1_000, distance: 0), readerScrolling: true, towardEnd: true)
        XCTAssertTrue(follow.follows)
    }

    func testDraggingTheScrollerUpLetsGo() {
        var follow = atEnd()
        // A scroller drag sends no wheel event, only the geometry moving up.
        _ = follow.update(at(980, distance: 20), readerScrolling: true)
        XCTAssertFalse(follow.follows)
        _ = follow.update(at(500, distance: 500), readerScrolling: true)
        XCTAssertTrue(follow.showsLatest)
    }

    func testScrollingBackToTheEndHidesLatestWithoutAClick() {
        var follow = atEnd()
        follow.readerScrolledUp()
        _ = follow.update(at(400, distance: 600), readerScrolling: true)
        XCTAssertTrue(follow.showsLatest)
        // Down again; the last input lands short and momentum settles the rest.
        _ = follow.update(at(960, distance: 40), readerScrolling: true, towardEnd: true)
        XCTAssertTrue(follow.showsLatest)
        _ = follow.update(at(990, distance: 10), readerScrolling: true, towardEnd: true)
        XCTAssertTrue(follow.follows)
        XCTAssertFalse(follow.showsLatest)
    }

    func testRestingAtTheEndFollowsOnceTheScrollSettles() {
        var follow = atEnd()
        follow.readerScrolledUp()
        _ = follow.update(at(992, distance: 8), readerScrolling: true)
        // The bounce brings the chat back to rest at the end itself.
        _ = follow.update(at(1_000, distance: 0), readerScrolling: true)
        XCTAssertFalse(follow.follows)
        follow.scrollSettled()
        XCTAssertTrue(follow.follows)
    }

    func testOutputArrivingForAReaderLeftAtTheEndFollowsAgain() {
        var follow = atEnd()
        follow.readerScrolledUp()
        _ = follow.update(at(999, distance: 1), readerScrolling: true)
        // No idle phase came, but the reader stopped at the end.
        XCTAssertTrue(follow.transcriptChanged(readerScrolling: false))
        XCTAssertTrue(follow.follows)
    }

    /// #2162: after Latest, a small scroll up stays let go instead of being
    /// pulled back to the end once it settles or output arrives.
    func testASmallScrollUpAfterLatestStaysLetGo() {
        var follow = atEnd()
        follow.readerScrolledUp()
        _ = follow.update(at(0, distance: 5_000), readerScrolling: true)
        follow.jumpToLatest()
        XCTAssertFalse(follow.update(at(5_000, distance: 0), readerScrolling: false))
        XCTAssertTrue(follow.follows)
        // A gentle flick up, well within the threshold, then momentum stops.
        follow.readerScrolledUp()
        XCTAssertFalse(follow.update(at(4_992, distance: 8), readerScrolling: true))
        follow.scrollSettled()
        XCTAssertFalse(follow.follows)
        // Output arrives, the geometry updates with nobody scrolling: still let go.
        XCTAssertFalse(follow.transcriptChanged(readerScrolling: false))
        XCTAssertFalse(follow.update(at(4_992, distance: 20), readerScrolling: false))
        XCTAssertFalse(follow.follows)
        // A second flick goes further up, and the reader reads on undisturbed.
        follow.readerScrolledUp()
        _ = follow.update(at(4_980, distance: 32), readerScrolling: true)
        follow.scrollSettled()
        XCTAssertFalse(follow.update(at(4_980, distance: 60), readerScrolling: false))
        XCTAssertFalse(follow.follows)
        XCTAssertTrue(follow.showsLatest)
    }

    /// #2162: rows above the end take their real height as a small scroll up
    /// after Latest lays them out; that must not scroll the chat to the end.
    func testLateRowHeightsAfterASmallScrollUpDoNotPullBack() {
        var follow = atEnd()
        follow.readerScrolledUp()
        _ = follow.update(at(4_990, distance: 10), readerScrolling: true)
        follow.scrollSettled()
        XCTAssertFalse(follow.update(at(5_030, distance: 10), readerScrolling: false))
        XCTAssertFalse(follow.update(at(5_030, distance: 40), readerScrolling: false))
        XCTAssertFalse(follow.follows)
    }

    func testScrollingDownToTheEndStillFollowsWithinTheThreshold() {
        var follow = atEnd()
        follow.readerScrolledUp()
        _ = follow.update(at(500, distance: 500), readerScrolling: true)
        _ = follow.update(at(990, distance: 10), readerScrolling: true, towardEnd: true)
        XCTAssertTrue(follow.follows)
        XCTAssertFalse(follow.showsLatest)
    }

    func testOneLatestPressHidesTheButtonAndCatchesUpWithLateLayout() {
        var follow = atEnd()
        follow.readerScrolledUp()
        _ = follow.update(at(0, distance: 5_000), readerScrolling: true)
        XCTAssertTrue(follow.showsLatest)
        follow.jumpToLatest()
        XCTAssertFalse(follow.showsLatest)
        // The jump lands short: lazy rows below took their real height late.
        // Following, short of the end, with no reader input: scroll again.
        XCTAssertTrue(follow.update(at(4_600, distance: 400), readerScrolling: false))
        XCTAssertFalse(follow.showsLatest)
        XCTAssertFalse(follow.update(at(5_000, distance: 0), readerScrolling: false))
        XCTAssertTrue(follow.follows)
        XCTAssertFalse(follow.showsLatest)
    }

    func testStreamingWhileReadingOlderOutputLeavesTheReaderInPlace() {
        var follow = atEnd()
        follow.readerScrolledUp()
        _ = follow.update(at(300, distance: 700), readerScrolling: true)
        // Output keeps growing below; the offset stays where the reader is.
        for growth in stride(from: 100.0, through: 2_000, by: 100) {
            XCTAssertFalse(follow.update(at(300, distance: 700 + growth), readerScrolling: false))
            XCTAssertFalse(follow.transcriptChanged(readerScrolling: false))
        }
        XCTAssertFalse(follow.follows)
        XCTAssertTrue(follow.showsLatest)
    }

    func testStreamingWhileFollowingKeepsUpWithTheEnd() {
        var follow = atEnd()
        // A big row arrives before the anchor catches up.
        XCTAssertTrue(follow.update(at(1_000, distance: 300), readerScrolling: false))
        XCTAssertTrue(follow.follows)
        XCTAssertFalse(follow.showsLatest)
        XCTAssertTrue(follow.transcriptChanged(readerScrolling: false))
    }

    func testAResizeIsNotTheReaderScrollingUp() {
        var follow = atEnd()
        _ = follow.update(at(700, distance: 300, height: 900), readerScrolling: true)
        XCTAssertTrue(follow.follows)
    }

    func testAShortChatNeverShowsLatest() {
        var follow = ChatFollow()
        _ = follow.update(ChatFollow.Geometry(offset: 0, containerHeight: 600, distanceFromBottom: -200, canScroll: false),
                          readerScrolling: false)
        follow.readerScrolledUp()
        XCTAssertTrue(follow.follows)
        XCTAssertFalse(follow.showsLatest)
    }

    func testAFindMatchLetsGo() {
        var follow = atEnd()
        follow.letGo()
        _ = follow.update(at(200, distance: 800), readerScrolling: false)
        XCTAssertFalse(follow.follows)
        XCTAssertTrue(follow.showsLatest)
    }
}

final class StreamingSplitTests: XCTestCase {
    private func split(_ text: String, settled: Int) -> (String, String) {
        let length = StreamingSplit.settledLength(of: text, settled: settled)
        guard length > 0 else { return ("", text) }
        return (String(text.prefix(length)), String(text.dropFirst(length + 1)))
    }

    func testNothingSettledFadesEverything() {
        XCTAssertEqual(StreamingSplit.settledLength(of: "one\ntwo", settled: 0), 0)
    }

    func testSettledParagraphsSplitOffAtTheLastBreak() {
        XCTAssertEqual(split("one\ntwo\nthree", settled: 10).0, "one\ntwo")
        XCTAssertEqual(split("one\ntwo\nthree", settled: 10).1, "three")
    }

    func testABreakInsideTheSettledTextNotPastIt() {
        XCTAssertEqual(split("one\ntwo\nthree", settled: 5).0, "one")
        XCTAssertEqual(split("one\ntwo\nthree", settled: 5).1, "two\nthree")
    }

    func testABlankLineStaysWithTheTail() {
        // Prose runs set a small blank line between blocks; it opens the tail
        // so the two pieces stack with the gap between them.
        XCTAssertEqual(split("one\n\ntwo", settled: 9).0, "one")
        XCTAssertEqual(split("one\n\ntwo", settled: 9).1, "\ntwo")
        XCTAssertEqual(split("one\n\ntwo", settled: 5).0, "one")
    }

    func testNoBreakMeansNoSplit() {
        XCTAssertEqual(StreamingSplit.settledLength(of: "one long paragraph", settled: 12), 0)
        XCTAssertEqual(StreamingSplit.settledLength(of: "\nleading", settled: 5), 0)
    }

    func testSettledPastTheEndIsClamped() {
        XCTAssertEqual(split("one\ntwo", settled: 99).0, "one")
    }

    func testALongReplyKeepsOnlyItsLastParagraphFading() {
        let paragraphs = (1...2_000).map { "Paragraph \($0) of a long streamed reply." }
        let text = paragraphs.joined(separator: "\n")
        let (head, tail) = split(text, settled: text.count - 5)
        XCTAssertEqual(tail, paragraphs.last)
        XCTAssertEqual(head, paragraphs.dropLast().joined(separator: "\n"))
    }
}
