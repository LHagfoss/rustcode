import XCTest
@testable import RustCodeKit

final class DiffTests: XCTestCase {
    func testToolUpdateParsesEngineDiffContent() throws {
        let json = """
        {"sessionId":"s","update":{"sessionUpdate":"tool_call_update","toolCallId":"e1","status":"completed",
         "content":[{"type":"content","content":{"type":"text","text":"ok"}},
                    {"type":"diff","path":"/repo/src/a.rs","oldText":"a\\nold\\n","newText":"a\\nnew\\n",
                     "_meta":{"rustcode":{"oldStart":10,"newStart":12}}},
                    {"type":"diff","path":"/repo/new.md","newText":"hi\\n"}],
         "rawOutput":{"content":"ok"}}}
        """
        let params = try JSONDecoder().decode(JSONValue.self, from: Data(json.utf8))
        guard case .toolCallUpdate(_, _, _, let output, let diffs, _, _) = SessionUpdate(notificationParams: params) else {
            return XCTFail("expected a tool update")
        }
        XCTAssertEqual(output, "ok")
        XCTAssertEqual(diffs, [
            FileDiff(path: "/repo/src/a.rs", oldText: "a\nold\n", newText: "a\nnew\n", oldStart: 10, newStart: 12),
            FileDiff(path: "/repo/new.md", oldText: nil, newText: "hi\n"),
        ])
    }

    func testLineDiffNumbersLinesAndPutsRemovalsFirst() {
        let lines = LineDiff.lines(old: ["a", "b", "c", "d"], new: ["a", "B", "c", "d", "e"], oldStart: 5, newStart: 7)
        XCTAssertEqual(lines, [
            DiffLine(kind: .context, text: "a", oldNumber: 5, newNumber: 7),
            DiffLine(kind: .removed, text: "b", oldNumber: 6),
            DiffLine(kind: .added, text: "B", newNumber: 8),
            DiffLine(kind: .context, text: "c", oldNumber: 7, newNumber: 9),
            DiffLine(kind: .context, text: "d", oldNumber: 8, newNumber: 10),
            DiffLine(kind: .added, text: "e", newNumber: 11),
        ])
    }

    func testLineDiffFindsTheCommonLinesInTheMiddle() {
        let kinds = LineDiff.lines(old: ["x", "keep", "y"], new: ["p", "keep", "q", "r"]).map(\.kind)
        XCTAssertEqual(kinds, [.removed, .added, .context, .removed, .added, .added])
    }

    /// #2247: a closing brace kept as context between edits left a removal
    /// stranded below it; the change reads as one block instead.
    func testLineDiffKeepsAChangeSplitByABraceTogether() {
        let old = ["fn a() {", "    let x = 1;", "    }", "    old_tail();", "}"]
        let new = ["fn a() {", "    let x = 2;", "    let y = 3;", "    }", "    new_tail();", "}"]
        let lines = LineDiff.lines(old: old, new: new, oldStart: 8937, newStart: 8937)
        XCTAssertEqual(lines.map(\.kind), [.context, .removed, .removed, .removed, .added, .added, .added, .added, .context])
        XCTAssertEqual(lines.filter { $0.kind == .removed }.map(\.text), ["    let x = 1;", "    }", "    old_tail();"])
        XCTAssertEqual(lines.filter { $0.kind == .added }.map(\.text), ["    let x = 2;", "    let y = 3;", "    }", "    new_tail();"])
        XCTAssertEqual(lines.filter { $0.kind == .removed }.compactMap(\.oldNumber), [8938, 8939, 8940])
        XCTAssertEqual(lines.filter { $0.kind == .added }.compactMap(\.newNumber), [8938, 8939, 8940, 8941])
        XCTAssertEqual(lines.last, DiffLine(kind: .context, text: "}", oldNumber: 8941, newNumber: 8942))
    }

    /// Real context between edits, or filler longer than the edits around
    /// it, still separates them.
    func testLineDiffKeepsMeaningfulContext() {
        XCTAssertEqual(LineDiff.lines(old: ["x", "keep", "y"], new: ["p", "keep", "q"]).map(\.kind),
                       [.removed, .added, .context, .removed, .added])
        XCTAssertEqual(LineDiff.lines(old: ["x", "", "", "y"], new: ["p", "", "", "q"]).map(\.kind),
                       [.removed, .added, .context, .context, .removed, .added])
    }

    func testHugeInputsFallBackToRemoveThenAdd() {
        let old = (0..<3000).map { "old \($0)" }
        let new = (0..<3000).map { "new \($0)" }
        let lines = LineDiff.lines(old: old, new: new)
        XCTAssertEqual(lines.count, 6000)
        XCTAssertEqual(lines.first?.kind, .removed)
        XCTAssertEqual(lines.last?.kind, .added)
    }

    func testFileChangeCountsFoldsAndJoinsHunks() {
        let whole = (1...20).map { "line \($0)" }
        var edited = whole
        edited[9] = "changed"
        let full = FileDiff(path: "/repo/a.txt", oldText: whole.joined(separator: "\n") + "\n",
                            newText: edited.joined(separator: "\n") + "\n")
        let change = FileChange.group([full])[0]
        XCTAssertEqual(change.added, 1)
        XCTAssertEqual(change.removed, 1)
        XCTAssertFalse(change.isNewFile)
        // Six unchanged lines before and after the change fold to three.
        XCTAssertEqual(change.rows.first, .gap(6))
        XCTAssertEqual(change.rows.last, .gap(7))
        XCTAssertEqual(change.rows.count, 1 + 3 + 2 + 3 + 1)

        let hunks = FileChange.group([
            FileDiff(path: "/r/b", oldText: "a\nb\n", newText: "a\nB\n", oldStart: 1, newStart: 1),
            FileDiff(path: "/r/b", oldText: "y\n", newText: "y\nz\n", oldStart: 40, newStart: 40),
            FileDiff(path: "/r/c", oldText: nil, newText: "new\n"),
        ])
        XCTAssertEqual(hunks.map(\.path), ["/r/b", "/r/c"])
        XCTAssertTrue(hunks[0].rows.contains(.gap(37)))
        XCTAssertEqual(hunks[0].added, 2)
        XCTAssertTrue(hunks[1].isNewFile)
        XCTAssertEqual(hunks[1].rows, [.line(DiffLine(kind: .added, text: "new", newNumber: 1))])
    }

    func testDisplayPathIsRelativeToTheProject() {
        let change = FileChange.group([FileDiff(path: "/repo/src/a.rs", oldText: "a\n", newText: "b\n")])[0]
        XCTAssertEqual(change.displayPath(relativeTo: "/repo"), "src/a.rs")
        XCTAssertEqual(change.displayPath(relativeTo: "/repo/"), "src/a.rs")
        XCTAssertEqual(change.displayPath(relativeTo: "/other"), "/repo/src/a.rs")
    }

    func testTranscriptKeepsTheChangesOfAToolCall() {
        var transcript = Transcript()
        transcript.apply(.toolCall(id: "e", title: "replace_file_content", kind: "edit", status: .inProgress, input: nil))
        transcript.apply(.toolCallUpdate(id: "e", title: nil, status: .completed, output: "done",
                                         diffs: [FileDiff(path: "/r/a", oldText: "a\n", newText: "b\n")]))
        transcript.apply(.toolCallUpdate(id: "e", title: nil, status: nil, output: nil))
        guard case .tool(let tool) = transcript.items[0].content else { return XCTFail("expected a tool card") }
        XCTAssertEqual(tool.changes.map(\.path), ["/r/a"])
        XCTAssertEqual(tool.changes.first?.added, 1)
    }

    private func gutters(_ change: FileChange) -> [String] {
        change.rows.compactMap { row in
            guard case .line(let line) = row else { return nil }
            return line.gutter(digits: change.gutterDigits)
        }
    }

    func testGutterNumbersAddedAndContextLinesInTheNewFile() {
        let change = FileChange.group([FileDiff(path: "/r/a", oldText: "a\nb\n", newText: "a\nnew\nb\n")])[0]
        XCTAssertEqual(gutters(change), [" 1", " 2", " 3"])
    }

    func testGutterNumbersARemovalInTheOldFile() {
        let change = FileChange.group([FileDiff(path: "/r/a", oldText: "a\ngone\nb\n", newText: "a\nb\n")])[0]
        XCTAssertEqual(gutters(change), [" 1", " 2", " 2"])
    }

    func testGutterNumbersANewFileFromOne() {
        let change = FileChange.group([FileDiff(path: "/r/a", oldText: nil, newText: "a\nb\n")])[0]
        XCTAssertTrue(change.isNewFile)
        XCTAssertEqual(gutters(change), [" 1", " 2"])
    }

    func testGutterDigitsFitTheWidestNumber() {
        let change = FileChange.group([FileDiff(path: "/r/a", oldText: "a\n", newText: "b\n", oldStart: 99, newStart: 120)])[0]
        XCTAssertEqual(change.gutterDigits, 3)
        XCTAssertEqual(gutters(change), [" 99", "120"])
    }

    func testAPathInsideTheProjectDropsItsPrefixAndOneOutsideStaysWhole() {
        XCTAssertEqual(ProjectPath.relative("/Users/a/code/src/game.js", to: "/Users/a/code"), "src/game.js")
        XCTAssertEqual(ProjectPath.relative("/Users/a/code/src/game.js", to: "/Users/a/code/"), "src/game.js")
        XCTAssertEqual(ProjectPath.relative("/Users/a/codex/game.js", to: "/Users/a/code"), "/Users/a/codex/game.js")
        XCTAssertEqual(ProjectPath.relative("/etc/hosts", to: "/Users/a/code"), "/etc/hosts")
        XCTAssertEqual(ProjectPath.relative("/etc/hosts", to: nil), "/etc/hosts")
    }
}
