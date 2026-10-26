import AppKit
import XCTest
@testable import RustCode

final class ChatLinksTests: XCTestCase {
    func testUserMessageLinksUseSingleUnderline() {
        let text = ChatLinks.linked("Read https://example.com/docs and https://example.com/other.")
        assertSingleUnderline(on: text)
    }

    func testFormattedMarkdownLinksUseSingleUnderline() {
        let text = MarkdownText.inline("**[bold](https://example.com/bold)** and *[italic](https://example.com/italic)*")
        assertSingleUnderline(on: text)
    }

    func testBareAddressInMarkdownIsLinkedButCodeIsNot() {
        let text = MarkdownText.inline("https://example.com/plain and `https://example.com/code`")
        let links = text.runs.filter { $0.link != nil }
        XCTAssertEqual(links.count, 1)
        assertSingleUnderline(on: text)
    }

    private func assertSingleUnderline(on text: AttributedString, file: StaticString = #filePath, line: UInt = #line) {
        let links = text.runs.filter { $0.link != nil }
        XCTAssertFalse(links.isEmpty, "Expected at least one link", file: file, line: line)
        for link in links {
            XCTAssertEqual(text[link.range].appKit.underlineStyle, .single, file: file, line: line)
        }
    }
}
