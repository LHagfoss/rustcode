import XCTest
@testable import RustCodeKit

final class MarkdownBlockTests: XCTestCase {
    /// #2432: GitHub takes one dash a column, aligned or not; such a table
    /// read as a paragraph of pipes.
    func testShortSeparatorsMakeATable() {
        let blocks = MarkdownBlock.parse("| Option | Speed |\n|:--|-:|\n| `a|b` | fast |\n| grep | slow |")
        XCTAssertEqual(blocks, [.table(headers: ["Option", "Speed"], rows: [["`a|b`", "fast"], ["grep", "slow"]])])
        XCTAssertEqual(MarkdownBlock.parse("a | b\n-|-\n1 | 2"), [.table(headers: ["a", "b"], rows: [["1", "2"]])])
    }

    func testAPipeWithoutASeparatorIsProse() {
        XCTAssertEqual(MarkdownBlock.parse("use a | b here\nand more"), [.paragraph("use a | b here\nand more")])
    }

    /// A fence in a list item is indented with it; its code is not.
    func testAFenceInAListItemLosesTheItemsIndent() {
        let blocks = MarkdownBlock.parse("1. Install it:\n\n   ```sh\n   brew install rg\n     nested\n   ```")
        XCTAssertEqual(blocks, [.listItem(marker: "1.", text: "Install it:"),
                                .code(language: "sh", text: "brew install rg\n  nested")])
    }

    func testTildeFencesAndPipesInCodeStayCode() {
        XCTAssertEqual(MarkdownBlock.parse("~~~rust\nlet x = a | b;\n~~~"), [.code(language: "rust", text: "let x = a | b;")])
        XCTAssertEqual(MarkdownBlock.parse("```\n| a | b |\n|---|---|\n```"), [.code(language: nil, text: "| a | b |\n|---|---|")])
    }
}
