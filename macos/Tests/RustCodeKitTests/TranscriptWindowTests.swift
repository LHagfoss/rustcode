import XCTest
@testable import RustCodeKit

final class TranscriptWindowTests: XCTestCase {
    private let chat = UUID()
    private let other = UUID()

    func testAShortChatShowsEverything() {
        let window = TranscriptWindow()
        XCTAssertEqual(window.firstShown(identity: chat, count: 40), 0)
        XCTAssertFalse(window.hasEarlier(identity: chat, count: 40))
    }

    func testALongChatOpensOnItsNewestItems() {
        var window = TranscriptWindow()
        window.open(identity: chat, count: 3_000)
        XCTAssertEqual(window.firstShown(identity: chat, count: 3_000), 3_000 - TranscriptWindow.initialCount)
        XCTAssertTrue(window.hasEarlier(identity: chat, count: 3_000))
    }

    /// Streamed items join at the bottom; the top of the window stays put,
    /// so nothing the reader is looking at slides away.
    func testNewItemsGrowTheWindowAtTheBottom() {
        var window = TranscriptWindow()
        window.open(identity: chat, count: 1_000)
        let first = window.firstShown(identity: chat, count: 1_000)
        XCTAssertEqual(window.firstShown(identity: chat, count: 1_050), first)
        // Opening the same chat again changes nothing.
        window.open(identity: chat, count: 1_050)
        XCTAssertEqual(window.firstShown(identity: chat, count: 1_050), first)
    }

    func testScrollingBackAddsAPageAtATimeUntilTheStart() {
        var window = TranscriptWindow()
        window.open(identity: chat, count: 400)
        XCTAssertEqual(window.firstShown(identity: chat, count: 400), 250)
        XCTAssertTrue(window.showEarlier(identity: chat, count: 400))
        XCTAssertEqual(window.firstShown(identity: chat, count: 400), 150)
        XCTAssertTrue(window.showEarlier(identity: chat, count: 400))
        XCTAssertTrue(window.showEarlier(identity: chat, count: 400))
        XCTAssertEqual(window.firstShown(identity: chat, count: 400), 0)
        XCTAssertFalse(window.showEarlier(identity: chat, count: 400))
        XCTAssertFalse(window.hasEarlier(identity: chat, count: 400))
    }

    /// Another chat opens on its own newest items, whatever the last one showed.
    func testSwitchingChatsStartsAgain() {
        var window = TranscriptWindow()
        window.open(identity: chat, count: 2_000)
        window.showAll(identity: chat, count: 2_000)
        XCTAssertEqual(window.firstShown(identity: other, count: 900), 900 - TranscriptWindow.initialCount)
        window.open(identity: other, count: 900)
        XCTAssertEqual(window.firstShown(identity: other, count: 900), 750)
    }

    /// A find match above the window joins it with a little context above.
    func testRevealBringsAMatchIntoTheWindow() {
        var window = TranscriptWindow()
        window.open(identity: chat, count: 3_000)
        XCTAssertFalse(window.reveal(2_990, identity: chat, count: 3_000), "already shown")
        XCTAssertTrue(window.reveal(40, identity: chat, count: 3_000))
        XCTAssertEqual(window.firstShown(identity: chat, count: 3_000), 40 - TranscriptWindow.revealMargin)
        XCTAssertTrue(window.reveal(3, identity: chat, count: 3_000))
        XCTAssertEqual(window.firstShown(identity: chat, count: 3_000), 0)
    }

    func testShowAllIsForHome() {
        var window = TranscriptWindow()
        window.open(identity: chat, count: 3_000)
        XCTAssertTrue(window.showAll(identity: chat, count: 3_000))
        XCTAssertEqual(window.firstShown(identity: chat, count: 3_000), 0)
        XCTAssertFalse(window.showAll(identity: chat, count: 3_000))
    }

    /// Paging back through a chat shows every item once, in order.
    func testPagingBackCoversTheWholeChatWithoutGaps() {
        var window = TranscriptWindow()
        let count = 2_250
        window.open(identity: chat, count: count)
        var shown = Array(window.firstShown(identity: chat, count: count)..<count)
        while window.hasEarlier(identity: chat, count: count) {
            let before = window.firstShown(identity: chat, count: count)
            window.showEarlier(identity: chat, count: count)
            shown = Array(window.firstShown(identity: chat, count: count)..<before) + shown
        }
        XCTAssertEqual(shown, Array(0..<count))
    }
}
