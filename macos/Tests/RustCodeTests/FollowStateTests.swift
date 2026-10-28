import Combine
import XCTest
import RustCodeKit
@testable import RustCode

/// #2260: the chat view redraws for what it shows of `ChatFollow`, not for
/// every scrolled frame or streamed token.
@MainActor
final class FollowStateTests: XCTestCase {
    private func geometry(offset: Double, fromBottom: Double) -> ChatFollow.Geometry {
        ChatFollow.Geometry(offset: offset, containerHeight: 600, distanceFromBottom: fromBottom, canScroll: true)
    }

    func testOnlyFollowingAndLatestPublish() {
        let state = FollowState()
        var changes = 0
        let watch = state.objectWillChange.sink { changes += 1 }
        defer { watch.cancel() }

        // Output growing under a chat held at its end: geometry only.
        for step in 0..<50 {
            _ = state.value.update(geometry(offset: Double(step) * 20, fromBottom: 0), readerScrolling: false)
        }
        XCTAssertEqual(changes, 0)

        // The reader scrolls up: the chat lets go and Latest shows, once.
        state.value.readerScrolledUp()
        for step in 0..<50 {
            _ = state.value.update(geometry(offset: 1000 - Double(step) * 10, fromBottom: 200 + Double(step) * 10),
                                   readerScrolling: true)
        }
        XCTAssertFalse(state.follows)
        XCTAssertTrue(state.showsLatest)
        XCTAssertEqual(changes, 2)

        // Latest: following again, and the button goes.
        state.value.jumpToLatest()
        XCTAssertTrue(state.follows)
        XCTAssertFalse(state.showsLatest)
        XCTAssertEqual(changes, 4)
    }

    /// #2417: a stored sidebar width is clamped to the usable range, so an
    /// invalid value (a stale or hand-edited default) never strands the bar.
    func testSidebarWidthIsClamped() {
        XCTAssertEqual(SidebarResizeHandle.clamp(300), 300)
        XCTAssertEqual(SidebarResizeHandle.clamp(0), SidebarResizeHandle.range.lowerBound)
        XCTAssertEqual(SidebarResizeHandle.clamp(10_000), SidebarResizeHandle.range.upperBound)
        XCTAssertEqual(SidebarResizeHandle.clamp(-50), SidebarResizeHandle.range.lowerBound)
    }

    /// #2554: a wheel beside or below the composer scrolls the chat, so it is
    /// the reader's scroll and lets go of the end; one over the composer or a
    /// card above it scrolls that, not the chat.
    func testAWheelBesideTheComposerIsTheReaders() {
        let chat = CGRect(x: 280, y: 52, width: 900, height: 700)
        let composer = CGRect(x: 400, y: 620, width: 660, height: 110)
        XCTAssertTrue(ChatScrollInput.wheelScrollsChat(at: CGPoint(x: 700, y: 300), chat: chat, composer: composer))
        // Beside the composer, and in the gap under it.
        XCTAssertTrue(ChatScrollInput.wheelScrollsChat(at: CGPoint(x: 320, y: 680), chat: chat, composer: composer))
        XCTAssertTrue(ChatScrollInput.wheelScrollsChat(at: CGPoint(x: 700, y: 745), chat: chat, composer: composer))
        XCTAssertFalse(ChatScrollInput.wheelScrollsChat(at: CGPoint(x: 700, y: 650), chat: chat, composer: composer))
        // The title bar and the sidebar are not the chat.
        XCTAssertFalse(ChatScrollInput.wheelScrollsChat(at: CGPoint(x: 700, y: 20), chat: chat, composer: composer))
        XCTAssertFalse(ChatScrollInput.wheelScrollsChat(at: CGPoint(x: 100, y: 300), chat: chat, composer: composer))
    }
}
