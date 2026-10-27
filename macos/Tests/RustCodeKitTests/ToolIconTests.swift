import XCTest
@testable import RustCodeKit

final class ToolIconTests: XCTestCase {
    private func call(_ title: String, detail: String? = nil) -> ToolItem {
        ToolItem(callId: "1", title: title, status: .completed, inputDetail: detail)
    }

    func testEachKindHasItsSymbol() {
        XCTAssertEqual(ToolIcon.symbol(for: call("run_command")), "terminal")
        XCTAssertEqual(ToolIcon.symbol(for: call("view_file")), "doc.text")
        XCTAssertEqual(ToolIcon.symbol(for: call("grep")), "magnifyingglass")
        XCTAssertEqual(ToolIcon.symbol(for: call("search_web")), "globe")
    }

    /// A page the agent read shows its site's icon; local and odd addresses do not.
    func testAReadPageShowsItsSiteIcon() {
        XCTAssertEqual(ToolIcon.siteIcon(for: call("read_url", detail: "https://docs.github.com/en/rest?x=1#y"))?.absoluteString,
                       "https://docs.github.com/favicon.ico")
        XCTAssertNil(ToolIcon.siteIcon(for: call("read_url", detail: "http://localhost:3000/")))
        XCTAssertNil(ToolIcon.siteIcon(for: call("read_url", detail: "file:///etc/passwd")))
        XCTAssertNil(ToolIcon.siteIcon(for: call("view_file", detail: "https://example.com")))
    }
}
