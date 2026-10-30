import XCTest
@testable import RustCode

final class ComposerEscapeTests: XCTestCase {
    func testAnOpenPanelClosesFirst() {
        XCTAssertEqual(ComposerEscape.action(panelOpen: true, hasDraft: true, busy: true), .closePanel)
    }

    func testADraftClearsBeforeTheTurnStops() {
        XCTAssertEqual(ComposerEscape.action(panelOpen: false, hasDraft: true, busy: true), .clearDraft)
        XCTAssertEqual(ComposerEscape.action(panelOpen: false, hasDraft: true, busy: false), .clearDraft)
    }

    func testAnEmptyComposerStopsARunningTurn() {
        XCTAssertEqual(ComposerEscape.action(panelOpen: false, hasDraft: false, busy: true), .stopTurn)
    }

    func testAnEmptyIdleComposerLeavesEscapeAlone() {
        XCTAssertEqual(ComposerEscape.action(panelOpen: false, hasDraft: false, busy: false), .ignore)
    }
}
