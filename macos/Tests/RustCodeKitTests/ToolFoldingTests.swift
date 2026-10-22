import XCTest
@testable import RustCodeKit

final class ToolFoldingTests: XCTestCase {
    /// `t` a finished call, `x` a failed one, `e` an edit, `s` some text.
    private func items(_ shape: String) -> [TranscriptItem] {
        var transcript = Transcript()
        for (index, kind) in shape.enumerated() {
            let id = "call-\(index)"
            switch kind {
            case "s":
                transcript.apply(.agentText("step \(index)"))
            default:
                transcript.apply(.toolCall(id: id, title: "run_command", kind: "execute",
                                           status: kind == "x" ? .failed : .completed, input: nil, detail: "cmd \(index)"))
                if kind == "e" {
                    transcript.apply(.toolCallUpdate(id: id, title: nil, status: .completed, output: nil,
                                                     diffs: [FileDiff(path: "/r/a", oldText: "a\n", newText: "b\n")]))
                }
            }
        }
        return transcript.items
    }

    func testHighFoldsNothing() {
        let folding = ToolFolding(items: items("tttstt"), verbosity: .high, open: [], running: false)
        XCTAssertTrue(folding.hidden.isEmpty)
        XCTAssertTrue(folding.folds.isEmpty)
    }

    func testLowFoldsEachRunOfTwoOrMore() {
        let list = items("txtsts tt".replacingOccurrences(of: " ", with: ""))
        let folding = ToolFolding(items: list, verbosity: .low, open: [], running: false)
        let first = list[0].id
        XCTAssertEqual(folding.fold(for: first), ToolRunFold(run: first, count: 3, failed: 1, isOpen: false))
        XCTAssertEqual(folding.hidden, [list[1].id, list[2].id, list[7].id])
        // A lone call between texts stays as it is.
        XCTAssertNil(folding.fold(for: list[4].id))
        XCTAssertNotNil(folding.fold(for: list[6].id))
    }

    func testAnOpenedRunHidesNothing() {
        let list = items("ttt")
        let folding = ToolFolding(items: list, verbosity: .low, open: [list[0].id], running: false)
        XCTAssertTrue(folding.hidden.isEmpty)
        XCTAssertEqual(folding.fold(for: list[0].id)?.isOpen, true)
    }

    func testWhileRunningOnlyTheCurrentCallShows() {
        let list = items("sttt")
        let folding = ToolFolding(items: list, verbosity: .low, open: [], running: true)
        XCTAssertEqual(folding.fold(for: list[1].id)?.count, 2)
        XCTAssertEqual(folding.hidden, [list[2].id])
        XCTAssertFalse(folding.hidden.contains(list[3].id), "the current call shows")
        // The first call of a live run folds even alone.
        let two = items("tt")
        XCTAssertEqual(ToolFolding(items: two, verbosity: .low, open: [], running: true).fold(for: two[0].id)?.count, 1)
    }

    func testEditsStayOutOfTheRuns() {
        let list = items("ttettt")
        let folding = ToolFolding(items: list, verbosity: .edits, open: [], running: false)
        XCTAssertEqual(folding.fold(for: list[0].id)?.count, 2)
        XCTAssertNil(folding.fold(for: list[2].id), "the edit shows on its own")
        XCTAssertFalse(folding.hidden.contains(list[2].id))
        XCTAssertEqual(folding.fold(for: list[3].id)?.count, 3)
    }

    func testReadsTheTerminalsSetting() {
        XCTAssertEqual(ToolVerbosity.configured(in: "theme = \"x\"\nverbosity = \"edits\"\n"), .edits)
        XCTAssertNil(ToolVerbosity.configured(in: "theme = \"x\""))
        XCTAssertNil(ToolVerbosity.configured(in: "verbosity = \"loud\""))
    }
}
