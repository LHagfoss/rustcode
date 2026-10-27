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

    /// A `github` call shows GitHub's icon and says what it read.
    func testAGitHubCallShowsGitHubAndWhatItRead() {
        XCTAssertEqual(ToolIcon.siteIcon(for: call("github"))?.absoluteString, "https://github.com/favicon.ico")
        XCTAssertEqual(ToolPresentation.title("github"), "GitHub")
        XCTAssertEqual(ToolPresentation.detail(input: ["action": "issue", "number": 7, "repo": "o/r"], output: nil), "Issue #7 · o/r")
        XCTAssertEqual(ToolPresentation.detail(input: ["action": "list_prs", "query": "label:bug"], output: nil), "Pull requests · label:bug")
        XCTAssertEqual(ToolPresentation.detail(input: ["action": "run_log", "number": 123], output: nil), "Run log #123")
        // Another tool's `action` argument is not mistaken for a GitHub call.
        XCTAssertEqual(ToolPresentation.detail(input: ["action": "start", "command": "npm test"], output: nil), "npm test")
    }
}
