import XCTest
@testable import RustCodeKit

/// The newest rows and the end of the chat stay out of the lazy stack
/// (#2209, #2212): a very tall row inside it threw off the stack's guessed
/// height, and the chat's end was scrolled into empty space and drawn blank.
final class ChatLayoutTests: XCTestCase {
    func testAShortChatIsAllEager() {
        XCTAssertEqual(ChatLayout.split(count: 0, firstShown: 0), 0)
        XCTAssertEqual(ChatLayout.split(count: 5, firstShown: 0), 0)
        XCTAssertEqual(ChatLayout.split(count: ChatLayout.eagerTail, firstShown: 0), 0)
    }

    func testALongChatKeepsItsNewestRowsEager() {
        let split = ChatLayout.split(count: 200, firstShown: 50)
        XCTAssertEqual(split, 200 - ChatLayout.eagerTail)
        XCTAssertEqual(200 - split, ChatLayout.eagerTail)
    }

    /// The lazy part never starts before the window or past the items.
    func testTheSplitStaysInsideTheWindow() {
        XCTAssertEqual(ChatLayout.split(count: 30, firstShown: 25), 25)
        XCTAssertEqual(ChatLayout.split(count: 30, firstShown: 40), 30)
        XCTAssertEqual(ChatLayout.split(count: 30, firstShown: -3), 10)
    }

    /// Sending a message or streaming a new item keeps the last item, the
    /// one being written, in the eager tail, and the lazy part only grows.
    func testNewItemsKeepTheLatestOutOfTheLazyStack() {
        var window = TranscriptWindow()
        let chat = UUID()
        window.open(identity: chat, count: 300)
        var previous = ChatLayout.split(count: 300, firstShown: window.firstShown(identity: chat, count: 300))
        for count in 301...400 {
            let first = window.firstShown(identity: chat, count: count)
            let split = ChatLayout.split(count: count, firstShown: first)
            XCTAssertLessThan(split, count, "the newest item is eager")
            XCTAssertGreaterThanOrEqual(split, first)
            XCTAssertGreaterThanOrEqual(split, previous)
            previous = split
        }
    }
}
