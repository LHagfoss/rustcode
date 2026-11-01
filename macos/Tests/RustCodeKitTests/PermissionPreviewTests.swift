import XCTest
@testable import RustCodeKit

/// A tool approval reads as words, a command and a diff, not JSON.
final class PermissionPreviewTests: XCTestCase {
    private func json(_ text: String) -> JSONValue {
        try! JSONDecoder().decode(JSONValue.self, from: Data(text.utf8))
    }

    func testCommandShowsTheCommandAndItsReason() {
        let preview = PermissionPreview(tool: "run_command", input: json("""
            {"command": "git switch -c fix/calc", "summary": "Create isolated fix branch", "network_access": true}
            """))
        XCTAssertEqual(preview.headline, "Run a command")
        XCTAssertEqual(preview.summary, "Create isolated fix branch")
        XCTAssertEqual(preview.body, .command("git switch -c fix/calc", cwd: nil))
        XCTAssertEqual(preview.notes, ["Needs network access"])
    }

    func testReplacementIsADiffWithoutGuessedLineNumbers() {
        let preview = PermissionPreview(tool: "replace_file_content", input: json("""
            {"path": "calc.py", "target_content": "def add(a, b):\\n    return a - b", "replacement_content": "def add(a, b):\\n    return a + b"}
            """))
        XCTAssertEqual(preview.headline, "Edit calc.py")
        guard case .edit(let change)? = preview.body else { return XCTFail("no diff") }
        XCTAssertEqual(change.added, 1)
        XCTAssertEqual(change.removed, 1)
        XCTAssertFalse(change.isNewFile)
        for case .line(let line) in change.rows { XCTAssertNil(line.number) }
    }

    func testMultiReplaceJoinsEditsWithAGap() {
        let preview = PermissionPreview(tool: "multi_replace_file_content", input: json("""
            {"path": "/p/a.rs", "replacements": [
              {"target_content": "a", "replacement_content": "b"},
              {"target_content": "c", "replacement_content": "d"}]}
            """))
        guard case .edit(let change)? = preview.body else { return XCTFail("no diff") }
        XCTAssertEqual(change.rows.filter { $0 == .gap(nil) }.count, 1)
        XCTAssertEqual(change.added, 2)
    }

    func testWriteShowsTheNewText() {
        let preview = PermissionPreview(tool: "write_to_file", input: json(#"{"path": "notes.md", "content": "one\ntwo\n"}"#))
        XCTAssertEqual(preview.headline, "Write notes.md")
        guard case .edit(let change)? = preview.body else { return XCTFail("no diff") }
        XCTAssertTrue(change.isNewFile)
        XCTAssertEqual(change.added, 2)
    }

    func testUnknownToolReadsAsWordsAndFields() {
        let preview = PermissionPreview(tool: "GitCommit", input: json(#"{"message": "fix: add", "all": true, "summary": "Commit"}"#))
        XCTAssertEqual(preview.headline, "Git commit")
        XCTAssertEqual(preview.body, .fields([.init(label: "All", value: "Yes"), .init(label: "Message", value: "fix: add")]))
        XCTAssertEqual(PermissionPreview.words("mcp__github__create_issue"), "Mcp github create issue")
    }
}
