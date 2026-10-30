import XCTest
import RustCodeKit
@testable import RustCode

/// Picked diff lines ride along with the typed message (#2643).
@MainActor
final class CodeReferencePromptTests: XCTestCase {
    func testReferencesFollowTheTypedTextAndPrecedeAttachedFiles() {
        let reference = CodeReference(intent: .ask, root: "/nonexistent", path: "a.swift", side: .old, lines: 3...4,
                                      scope: "uncommitted changes", baseCommit: nil, excerpt: ["old()", "gone()"])
        let workspace = URL(fileURLWithPath: "/w")
        let text = DesktopModel.prompt("Why was this removed?", attaching: [URL(fileURLWithPath: "/w/b.txt")],
                                       references: [reference], in: workspace)
        XCTAssertTrue(text.hasPrefix("Why was this removed?\n\nQuestion about lines 3-4 removed from `a.swift`"), text)
        XCTAssertTrue(text.contains("```\nold()\ngone()\n```"), text)
        XCTAssertTrue(text.hasSuffix("Attached files:\n- `b.txt`"), text)
        // A reference alone is a message.
        XCTAssertTrue(DesktopModel.prompt("", attaching: [], references: [reference], in: workspace).hasPrefix("Question about"))
    }
}
