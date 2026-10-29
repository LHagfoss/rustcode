import AppKit
import RustCodeKit
import SwiftUI
import XCTest
@testable import RustCode

@MainActor
final class ProseTextTests: XCTestCase {
    private func attributes(of string: NSAttributedString, at text: String) -> [NSAttributedString.Key: Any] {
        let range = (string.string as NSString).range(of: text)
        XCTAssertNotEqual(range.location, NSNotFound, "\(text) is in the string")
        return string.attributes(at: range.location, effectiveRange: nil)
    }

    func testInlineStylesBecomeAppKitFonts() {
        let styled = ProseStyle.appKit(MarkdownText.inline("plain **bold** *italic* `code`"))
        XCTAssertEqual(styled.string, "plain bold italic code")
        let body = NSFont.preferredFont(forTextStyle: .body)
        let plain = attributes(of: styled, at: "plain")[.font] as? NSFont
        XCTAssertEqual(plain?.pointSize, body.pointSize)
        XCTAssertFalse(plain?.fontDescriptor.symbolicTraits.contains(.bold) ?? true)
        let bold = attributes(of: styled, at: "bold")[.font] as? NSFont
        XCTAssertTrue(bold?.fontDescriptor.symbolicTraits.contains(.bold) ?? false)
        let italic = attributes(of: styled, at: "italic")[.font] as? NSFont
        XCTAssertTrue(italic?.fontDescriptor.symbolicTraits.contains(.italic) ?? false)
        let code = attributes(of: styled, at: "code")[.font] as? NSFont
        XCTAssertTrue(code?.fontDescriptor.symbolicTraits.contains(.monoSpace) ?? false)
        XCTAssertEqual((attributes(of: styled, at: "plain")[.paragraphStyle] as? NSParagraphStyle)?.lineSpacing, 2)
    }

    func testLinksKeepTheirAddressUnderlineAndTooltip() {
        let styled = ProseStyle.appKit(MarkdownText.inline("see [the docs](https://example.com/docs)"))
        let link = attributes(of: styled, at: "the docs")
        XCTAssertEqual(link[.link] as? URL, URL(string: "https://example.com/docs"))
        XCTAssertEqual(link[.underlineStyle] as? Int, NSUnderlineStyle.single.rawValue)
        XCTAssertEqual(link[.toolTip] as? String, "https://example.com/docs")
        XCTAssertNil(attributes(of: styled, at: "see")[.link])
    }

    func testHeadingsAndListMarkersKeepTheirLook() {
        var heading = AttributedString("Title")
        heading.font = .title2.bold()
        var bullet = AttributedString("• ")
        bullet.foregroundColor = .secondary
        let styled = ProseStyle.appKit(heading + AttributedString("\n") + bullet + AttributedString("item"))
        let title = attributes(of: styled, at: "Title")[.font] as? NSFont
        XCTAssertEqual(title?.pointSize, NSFont.preferredFont(forTextStyle: .title2).pointSize)
        XCTAssertTrue(title?.fontDescriptor.symbolicTraits.contains(.bold) ?? false)
        XCTAssertEqual(attributes(of: styled, at: "•")[.foregroundColor] as? NSColor, .secondaryLabelColor)
        XCTAssertEqual(attributes(of: styled, at: "item")[.foregroundColor] as? NSColor, .labelColor)
    }

    func testFindMarksEveryMatchIgnoringCase() {
        let base = ProseStyle.appKit(AttributedString("Parser and parser, PARSER."))
        let marked = ProseStyle.highlight(base, query: "parser")
        var ranges: [NSRange] = []
        marked.enumerateAttribute(.backgroundColor, in: NSRange(location: 0, length: marked.length)) { value, range, _ in
            if value != nil { ranges.append(range) }
        }
        XCTAssertEqual(ranges, [NSRange(location: 0, length: 6), NSRange(location: 11, length: 6), NSRange(location: 19, length: 6)])
        XCTAssertNil(base.attribute(.backgroundColor, at: 0, effectiveRange: nil), "the cached run stays unmarked")
    }

    func testContentCachesTheFindHighlightAndHeights() {
        let content = ProseContent(AttributedString(String(repeating: "word ", count: 200)))
        XCTAssertTrue(content.attributed(highlighting: "") === content.attributed(highlighting: ""))
        XCTAssertTrue(content.attributed(highlighting: "word") === content.attributed(highlighting: " word "))
        let narrow = content.height(for: 200)
        let wide = content.height(for: 800)
        XCTAssertGreaterThan(narrow, wide, "narrower wraps to more lines")
        XCTAssertGreaterThan(wide, 0)
        XCTAssertEqual(content.height(for: 200) { _ in XCTFail("measured again"); return 0 }, narrow)
    }

    func testOneRunSelectedAtATimeAndClearEndsIt() {
        let first = ProseNSTextView.make()
        let second = ProseNSTextView.make()
        let selection = ProseSelection.shared
        for (view, item) in [(first, 1), (second, 2)] {
            view.delegate = selection
            view.run = TranscriptSelection.Run(item: item, segment: 0)
            view.string = "Some text to select"
            selection.register(view)
        }
        defer {
            selection.unregister(first)
            selection.unregister(second)
        }
        first.setSelectedRange(NSRange(location: 0, length: 4))
        XCTAssertEqual(selection.selection.bounds?.start, .init(item: 1, segment: 0, offset: 0))
        second.setSelectedRange(NSRange(location: 5, length: 4))
        XCTAssertEqual(first.selectedRange().length, 0, "selecting in another run ends the first")
        XCTAssertEqual(selection.selection.bounds?.end, .init(item: 2, segment: 0, offset: 9))
        XCTAssertTrue(selection.clear())
        XCTAssertEqual(second.selectedRange().length, 0)
        XCTAssertTrue(selection.selection.isEmpty)
        XCTAssertFalse(selection.clear(), "nothing left to end")
    }

    func testARedrawnRunRestoresItsHighlight() {
        let view = ProseNSTextView.make()
        let selection = ProseSelection.shared
        view.delegate = selection
        view.run = TranscriptSelection.Run(item: 7, segment: 0)
        view.string = "Selected before it undrew"
        selection.register(view)
        view.setSelectedRange(NSRange(location: 9, length: 6))
        selection.unregister(view)

        // Drawn again: a new view for the same run.
        let redrawn = ProseNSTextView.make()
        redrawn.delegate = selection
        redrawn.run = TranscriptSelection.Run(item: 7, segment: 0)
        selection.register(redrawn)
        defer { selection.unregister(redrawn) }
        selection.replacing(redrawn) { redrawn.string = "Selected before it undrew" }
        XCTAssertEqual(redrawn.selectedRange(), NSRange(location: 9, length: 6))
        selection.clear()
    }
}
