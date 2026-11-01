import XCTest
@testable import RustCodeKit

final class DesktopAutomationTests: XCTestCase {
    func testRefsBelongOnlyToIssuingSessionAndApp() {
        var ledger = DesktopRefLedger()
        ledger.record(session: "one", app: "Finder", tree: ["children": [["ref_id": "@s1:e1"]]])
        XCTAssertTrue(ledger.contains(session: "one", app: "Finder", ref: "@s1:e1"))
        XCTAssertFalse(ledger.contains(session: "two", app: "Finder", ref: "@s1:e1"))
        XCTAssertFalse(ledger.contains(session: "one", app: "Mail", ref: "@s1:e1"))
        XCTAssertFalse(ledger.contains(session: "one", app: "Finder", ref: "@s1:e2"))
        ledger.record(session: "one", app: "Finder", tree: ["ref_id": "@s2:e1"])
        XCTAssertFalse(ledger.contains(session: "one", app: "Finder", ref: "@s1:e1"))
        ledger.record(session: "two", app: "Mail", tree: ["ref_id": "@s3:e1", "role": "button", "name": "Send"], window: "Draft")
        XCTAssertEqual(ledger.describe(session: "two", app: "Mail", ref: "@s3:e1"), "Draft · button · Send")
        ledger.clear(session: "one")
        XCTAssertFalse(ledger.contains(session: "one", app: "Finder", ref: "@s2:e1"))
        XCTAssertTrue(ledger.contains(session: "two", app: "Mail", ref: "@s3:e1"))
        ledger.clear(session: "two", app: "Mail")
        XCTAssertFalse(ledger.contains(session: "two", app: "Mail", ref: "@s3:e1"))
    }
}
