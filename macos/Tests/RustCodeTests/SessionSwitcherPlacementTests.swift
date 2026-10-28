import XCTest
@testable import RustCode

final class SessionSwitcherPlacementTests: XCTestCase {
    /// #2528: lower than the old fixed 72 pt in ordinary windows, a full
    /// panel never past the bottom, and never under the title bar.
    func testThePanelSitsLowerInTallerWindowsAndAlwaysFits() {
        var previous = 0.0
        for height in [480.0, 640, 800, 1000, 1400] {
            let top = SessionSwitcherOverlay.topInset(windowHeight: height)
            XCTAssertGreaterThanOrEqual(top, 72, "\(height)")
            XCTAssertGreaterThanOrEqual(top, previous, "\(height)")
            if height >= 640 {
                XCTAssertGreaterThan(top, 72, "\(height)")
                XCTAssertLessThanOrEqual(top + SessionSwitcherOverlay.tallestPanel, height - 16, "\(height)")
            }
            previous = top
        }
    }

    func testThePanelNarrowsToACompactWindow() {
        XCTAssertEqual(SessionSwitcherOverlay.panelWidth(windowWidth: 1200), 600)
        XCTAssertEqual(SessionSwitcherOverlay.panelWidth(windowWidth: 500), 444)
    }
}
