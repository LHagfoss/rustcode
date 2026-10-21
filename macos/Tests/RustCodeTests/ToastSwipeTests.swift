import XCTest
@testable import RustCode

/// Toasts sit centered at the top of the chat (#2213): they leave upward or
/// to either side, never down into the chat.
final class ToastSwipeTests: XCTestCase {
    func testShortDragSpringsBack() {
        let drag = CGSize(width: 30, height: -20)
        XCTAssertFalse(ToastSwipe.dismisses(translation: drag, predicted: drag))
        let left = CGSize(width: -30, height: -20)
        XCTAssertFalse(ToastSwipe.dismisses(translation: left, predicted: left))
    }

    func testDragFarEnoughUpOrToEitherSideDismisses() {
        XCTAssertTrue(ToastSwipe.dismisses(translation: CGSize(width: 90, height: 0), predicted: .zero))
        XCTAssertTrue(ToastSwipe.dismisses(translation: CGSize(width: -90, height: 0), predicted: .zero))
        XCTAssertTrue(ToastSwipe.dismisses(translation: CGSize(width: 0, height: -90), predicted: .zero))
    }

    func testFlickOutDismissesBeforeTheDistance() {
        XCTAssertTrue(ToastSwipe.dismisses(translation: CGSize(width: 20, height: 0), predicted: CGSize(width: 240, height: 0)))
        XCTAssertTrue(ToastSwipe.dismisses(translation: CGSize(width: -20, height: 0), predicted: CGSize(width: -240, height: 0)))
    }

    func testDragDownNeverDismisses() {
        let down = CGSize(width: 0, height: 300)
        XCTAssertFalse(ToastSwipe.dismisses(translation: down, predicted: down))
    }

    func testDragDownIsResisted() {
        let offset = ToastSwipe.offset(for: CGSize(width: 0, height: 300))
        XCTAssertGreaterThan(offset.height, 0)
        XCTAssertLessThan(offset.height, 24)
        // The ways out follow the pointer exactly.
        XCTAssertEqual(ToastSwipe.offset(for: CGSize(width: 50, height: -10)), CGSize(width: 50, height: -10))
        XCTAssertEqual(ToastSwipe.offset(for: CGSize(width: -50, height: -10)), CGSize(width: -50, height: -10))
    }

    func testExitFollowsTheDragsMainAxis() {
        XCTAssertGreaterThan(ToastSwipe.exit(for: CGSize(width: 90, height: -10)).width, 0)
        XCTAssertLessThan(ToastSwipe.exit(for: CGSize(width: -90, height: -10)).width, 0)
        XCTAssertLessThan(ToastSwipe.exit(for: CGSize(width: 10, height: -90)).height, 0)
    }

    func testFadeGrowsTowardEitherSide() {
        XCTAssertEqual(ToastSwipe.fade(for: .zero), 0)
        XCTAssertEqual(ToastSwipe.fade(for: CGSize(width: 120, height: 0)), ToastSwipe.fade(for: CGSize(width: -120, height: 0)))
        XCTAssertGreaterThan(ToastSwipe.fade(for: CGSize(width: -120, height: 0)), 0)
        XCTAssertEqual(ToastSwipe.fade(for: CGSize(width: 0, height: 20)), 0, "a drag down does not fade it")
    }
}
