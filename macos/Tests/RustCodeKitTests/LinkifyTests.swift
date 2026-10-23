import XCTest
@testable import RustCodeKit

final class LinkifyTests: XCTestCase {
    private func linked(_ text: String, skipCodeSpans: Bool = false) -> [String] {
        Linkify.links(in: text, skipCodeSpans: skipCodeSpans).map { link in
            String(Array(text)[link.range])
        }
    }

    func testFindsAddressesWithASchemeOrWww() {
        XCTAssertEqual(linked("See https://example.com/a?b=1 and www.rust-lang.org."),
                       ["https://example.com/a?b=1", "www.rust-lang.org"])
        XCTAssertEqual(Linkify.links(in: "www.rust-lang.org").first?.url.absoluteString, "http://www.rust-lang.org")
    }

    func testFileNamesAreNotLinks() {
        XCTAssertEqual(linked("Edit README.md and Package.swift"), [])
        XCTAssertEqual(linked("plain words"), [])
    }

    func testCodeSpansKeepTheirAddresses() {
        XCTAssertEqual(linked("run `curl https://a.example` then open https://b.example", skipCodeSpans: true),
                       ["https://b.example"])
        XCTAssertEqual(linked("```\ncurl https://a.example\n```\nhttps://b.example", skipCodeSpans: true),
                       ["https://b.example"])
        // An unclosed backtick opens no span.
        XCTAssertEqual(linked("it's `odd https://b.example", skipCodeSpans: true), ["https://b.example"])
    }

    func testOffsetsCountCharactersNotUTF16() {
        let text = "👋 https://example.com"
        XCTAssertEqual(Linkify.links(in: text).first?.range, 2..<21)
    }
}
