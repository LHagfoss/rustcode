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
        XCTAssertEqual(SidebarWidth.clamp(300), 300)
        XCTAssertEqual(SidebarWidth.clamp(0), SidebarWidth.range.lowerBound)
        XCTAssertEqual(SidebarWidth.clamp(10_000), SidebarWidth.range.upperBound)
        XCTAssertEqual(SidebarWidth.clamp(-50), SidebarWidth.range.lowerBound)
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
        // #2598: an agent's conversation in the chat's place scrolls itself.
        XCTAssertFalse(ChatScrollInput.wheelScrollsChat(at: CGPoint(x: 700, y: 300), chat: chat, composer: composer,
                                                        replaced: true))
    }

    /// #2598: scrolling an agent's conversation does not let go of the end
    /// of the chat under it, so the chat's turn writing meanwhile does not
    /// leave it far up the transcript for when the reader comes back.
    func testScrollingAnAgentLeavesTheChatFollowing() {
        let input = ChatScrollInput.shared
        let (savedReplaced, savedScrollUp) = (input.isReplaced, input.onScrollUp)
        defer { input.isReplaced = savedReplaced; input.onScrollUp = savedScrollUp }
        var follow = ChatFollow()
        _ = follow.update(geometry(offset: 1000, fromBottom: 0), readerScrolling: false)
        input.onScrollUp = { follow.readerScrolledUp() }

        input.isReplaced = { true }
        input.debugObserve(deltaY: 40)
        XCTAssertFalse(input.isActive)
        XCTAssertTrue(follow.follows)
        // The chat's turn writes on: it stays at its end.
        XCTAssertTrue(follow.transcriptChanged(readerScrolling: input.isActive))

        // Back at the chat, the same wheel is the reader's.
        input.isReplaced = { false }
        input.debugObserve(deltaY: 40)
        XCTAssertTrue(input.isActive)
        XCTAssertFalse(follow.follows)
    }
}
