import XCTest
@testable import RustCodeKit

/// #2593: Settings keeps a section sidebar, compact in a narrow window, and
/// centers the section in a column with responsive margins.
final class SettingsLayoutTests: XCTestCase {
    func testRoomyWindowCentersACappedColumn() {
        let layout = SettingsLayout(width: 1400)
        XCTAssertFalse(layout.compact)
        XCTAssertEqual(layout.sidebar, 230)
        XCTAssertEqual(layout.column, 720)
        XCTAssertEqual(layout.margin, 58.5, accuracy: 0.001)
        // Centered: the same room either side.
        let leading = layout.columnMinX(in: 1400)
        XCTAssertEqual(leading, 1400 - 230 - 720 - leading, accuracy: 0.001)
    }

    func testMinimumWindowUsesTheCompactSidebar() {
        let layout = SettingsLayout(width: 850)
        XCTAssertTrue(layout.compact)
        XCTAssertEqual(layout.sidebar, 64)
        XCTAssertEqual(layout.column, 707.4, accuracy: 0.001)
        XCTAssertGreaterThanOrEqual(layout.columnMinX(in: 850), layout.margin)
    }

    func testColumnStaysInsideTheMarginsAtEveryWidth() {
        for width in stride(from: 0.0, through: 2600, by: 10) {
            let layout = SettingsLayout(width: width)
            XCTAssertEqual(layout.compact, width < 880, "\(width)")
            XCTAssertGreaterThanOrEqual(layout.margin, 24, "\(width)")
            XCTAssertLessThanOrEqual(layout.margin, 64, "\(width)")
            XCTAssertLessThanOrEqual(layout.column, SettingsLayout.maxColumn, "\(width)")
            XCTAssertGreaterThanOrEqual(layout.column, 0, "\(width)")
            if layout.column > 0 {
                XCTAssertGreaterThanOrEqual(layout.columnMinX(in: width), layout.margin - 0.001, "\(width)")
            }
        }
    }
}
