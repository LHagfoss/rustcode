import XCTest
@testable import RustCodeKit

/// `>` quotes (#2525): consecutive lines are one quote, a bare `>` splits
/// its paragraphs, and inline Markdown is left for the renderer.
final class MarkdownQuoteTests: XCTestCase {
    private func parse(_ text: String) -> [MarkdownBlock] { MarkdownBlock.parse(text) }

    func testAShortQuote() {
        XCTAssertEqual(parse("> Ship it."), [.quote("Ship it.")])
    }

    func testConsecutiveLinesAreOneQuoteWithTheirParagraphs() {
        let text = """
        Handover:

        > You are taking over the **release**.
        > Read [the plan](https://example.com) first.
        >
        > Then run `scripts/release.sh`.
        >

        After the quote.
        """
        XCTAssertEqual(parse(text), [
            .paragraph("Handover:"),
            .quote("You are taking over the **release**.\nRead [the plan](https://example.com) first.\n\nThen run `scripts/release.sh`."),
            .paragraph("After the quote."),
        ])
    }

    func testQuotesSeparatedByABlankLineStaySeparate() {
        XCTAssertEqual(parse("> one\n\n> two"), [.quote("one"), .quote("two")])
    }

    func testABareMarkerAloneIsNoQuote() {
        XCTAssertEqual(parse(">\n> text"), [.quote("text")])
        XCTAssertEqual(parse(">"), [])
    }
}
