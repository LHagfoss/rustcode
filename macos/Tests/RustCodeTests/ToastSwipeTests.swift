import XCTest
@testable import RustCode

final class ToastSwipeTests: XCTestCase {
    func testShortDragSpringsBack() {
        let drag = CGSize(width: 30, height: -20)
        XCTAssertFalse(ToastSwipe.dismisses(translation: drag, predicted: drag))
    }

    func testDragFarEnoughRightOrUpDismisses() {
        XCTAssertTrue(ToastSwipe.dismisses(translation: CGSize(width: 90, height: 0), predicted: .zero))
        XCTAssertTrue(ToastSwipe.dismisses(translation: CGSize(width: 0, height: -90), predicted: .zero))
    }

    func testFlickOutDismissesBeforeTheDistance() {
        XCTAssertTrue(ToastSwipe.dismisses(translation: CGSize(width: 20, height: 0), predicted: CGSize(width: 240, height: 0)))
    }

    func testDragLeftOrDownNeverDismisses() {
        let left = CGSize(width: -300, height: 0)
        let down = CGSize(width: 0, height: 300)
        XCTAssertFalse(ToastSwipe.dismisses(translation: left, predicted: left))
        XCTAssertFalse(ToastSwipe.dismisses(translation: down, predicted: down))
    }

    func testDragAgainstTheWayOutIsResisted() {
        let offset = ToastSwipe.offset(for: CGSize(width: -300, height: 300))
        XCTAssertLessThan(abs(offset.width), 24)
        XCTAssertLessThan(offset.height, 24)
        XCTAssertLessThan(offset.width, 0)
        XCTAssertGreaterThan(offset.height, 0)
        // The way out follows the pointer exactly.
        XCTAssertEqual(ToastSwipe.offset(for: CGSize(width: 50, height: -10)), CGSize(width: 50, height: -10))
    }

    func testExitFollowsTheDragsMainAxis() {
        XCTAssertGreaterThan(ToastSwipe.exit(for: CGSize(width: 90, height: -10)).width, 0)
        XCTAssertLessThan(ToastSwipe.exit(for: CGSize(width: 10, height: -90)).height, 0)
    }
}
