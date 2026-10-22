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
}
