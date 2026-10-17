import XCTest
@testable import RustCodeKit

final class SyntaxHighlighterTests: XCTestCase {
    private func kinds(_ line: String, _ language: String, open: Bool = false) -> [(String, SyntaxHighlighter.Kind)] {
        var inBlock = open
        return SyntaxHighlighter(language: language)!.tokens(in: line, inBlockComment: &inBlock)
            .map { (String(line[$0.range]), $0.kind) }
    }

    func testSwiftLineHasKeywordsTypesStringsNumbersAndComments() {
        let tokens = kinds(#"let view = Text("hi", count: 42) // done"#, "swift")
        XCTAssertEqual(tokens.map(\.0), ["let", "Text", #""hi""#, "42", "// done"])
        XCTAssertEqual(tokens.map(\.1), [.keyword, .type, .string, .number, .comment])
    }

    func testBlockCommentCarriesAcrossLines() {
        let highlighter = SyntaxHighlighter(language: "rs")!
        var inBlock = false
        _ = highlighter.tokens(in: "fn a() {} /* start", inBlockComment: &inBlock)
        XCTAssertTrue(inBlock)
        let next = highlighter.tokens(in: "still */ let x = 1;", inBlockComment: &inBlock)
        XCTAssertFalse(inBlock)
        XCTAssertEqual(next.first?.kind, .comment)
        XCTAssertTrue(next.contains { $0.kind == .keyword })
    }

    func testRustLifetimeIsNotAString() {
        let tokens = kinds("fn get<'a>(x: &'a str) -> char { 'c' }", "rust")
        XCTAssertEqual(tokens.filter { $0.1 == .string }.map(\.0), ["'c'"])
    }

    func testPythonUsesHashCommentsAndSingleQuotedStrings() {
        let tokens = kinds("def f(): return 'hello'  # note", "py")
        XCTAssertEqual(tokens.map(\.1), [.keyword, .keyword, .string, .comment])
    }

    func testLanguageFromPathAndUnknownLanguage() {
        XCTAssertNotNil(SyntaxHighlighter(path: "macos/Sources/App.swift"))
        XCTAssertNotNil(SyntaxHighlighter(path: "Makefile"))
        XCTAssertNil(SyntaxHighlighter(path: "notes.txt"))
        XCTAssertNil(SyntaxHighlighter(language: "brainfuck"))
    }
}
