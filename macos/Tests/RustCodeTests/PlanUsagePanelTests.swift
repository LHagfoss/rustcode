import XCTest
@testable import RustCode

/// #2551: the usage panel floats over the meter and stays inside the chat.
@MainActor
final class PlanUsagePanelTests: XCTestCase {
    func testThePanelCentresOverTheMeterWithinTheChat() {
        let half = PlanUsagePanel.width / 2
        XCTAssertEqual(PlanUsagePanel.leading(meterMidX: 600, in: 1200), 600 - half)
        // Near the right edge, as beside the model chip, it stops short of it.
        XCTAssertEqual(PlanUsagePanel.leading(meterMidX: 1190, in: 1200), 1200 - PlanUsagePanel.width - 12)
        XCTAssertEqual(PlanUsagePanel.leading(meterMidX: 10, in: 1200), 12)
        // A chat narrower than the panel keeps its leading margin.
        XCTAssertEqual(PlanUsagePanel.leading(meterMidX: 100, in: 200), 12)
    }
}
