import XCTest
@testable import RustCodeKit

/// #2552: the chat column, its padding, the timeline rail and the composer
/// adapt to the chat's width at set breakpoints.
final class ChatColumnLayoutTests: XCTestCase {
    /// The rail never reaches the column's text, and the column never
    /// scrolls sideways (it fits inside the padding).
    private func assertFits(_ width: Double, file: StaticString = #filePath, line: UInt = #line) {
        let layout = ChatColumnLayout(width: width)
        let columnStart = layout.columnMinX(in: width)
        XCTAssertGreaterThanOrEqual(columnStart, layout.padding - 0.001, "\(width)", file: file, line: line)
        XCTAssertLessThanOrEqual(layout.column + 2 * layout.padding, max(width, 2 * layout.padding) + 0.001,
                                 "\(width)", file: file, line: line)
        if let rail = layout.rail {
            XCTAssertGreaterThanOrEqual(rail.x, 0, file: file, line: line)
            XCTAssertLessThan(rail.x + rail.width, columnStart, "rail meets the text at \(width)", file: file, line: line)
            XCTAssertGreaterThan(rail.tick, 0, file: file, line: line)
        }
    }

    func testWideMediumAndNarrowWidths() {
        let wide = ChatColumnLayout(width: 1400)
        XCTAssertEqual(wide.size, .wide)
        XCTAssertEqual(wide.column, 980)
        XCTAssertEqual(wide.padding, 28)
        XCTAssertEqual(wide.rail?.width, 28)
        XCTAssertEqual(wide.rail?.x, 10)
        XCTAssertFalse(wide.compactComposer)

        let medium = ChatColumnLayout(width: 800)
        XCTAssertEqual(medium.size, .medium)
        XCTAssertEqual(medium.column, 744)
        XCTAssertEqual(medium.rail?.width, 18)
        XCTAssertFalse(medium.compactComposer)

        let narrow = ChatColumnLayout(width: 480)
        XCTAssertEqual(narrow.size, .narrow)
        XCTAssertEqual(narrow.padding, 20)
        XCTAssertEqual(narrow.column, 440)
        XCTAssertEqual(narrow.rail?.width, 14)
        XCTAssertTrue(narrow.compactComposer)

        let tiny = ChatColumnLayout(width: 320)
        XCTAssertEqual(tiny.padding, 16)
        XCTAssertNil(tiny.rail)
        XCTAssertFalse(tiny.showsRail(turns: 5))
    }

    /// The rail used to go below 848 points, the width that left 44 beside a
    /// 760-point column; it now stays down to 360, in a narrower form.
    func testTheRailStaysWhereItUsedToGo() {
        for width in [847.0, 700, 600, 599, 400, 360] {
            XCTAssertTrue(ChatColumnLayout(width: width).showsRail(turns: 2), "\(width)")
        }
        XCTAssertFalse(ChatColumnLayout(width: 359).showsRail(turns: 2))
        XCTAssertFalse(ChatColumnLayout(width: 1400).showsRail(turns: 1))
    }

    func testBreakpointsAreExact() {
        XCTAssertEqual(ChatColumnLayout(width: 1072).size, .wide)
        XCTAssertEqual(ChatColumnLayout(width: 1071).size, .medium)
        XCTAssertEqual(ChatColumnLayout(width: 600).size, .medium)
        XCTAssertEqual(ChatColumnLayout(width: 599).size, .narrow)
    }

    func testEveryWidthFits() {
        for width in stride(from: 100.0, through: 2400, by: 7) { assertFits(width) }
        for width in [359.0, 360, 599, 600, 1071, 1072] { assertFits(width) }
    }
}
