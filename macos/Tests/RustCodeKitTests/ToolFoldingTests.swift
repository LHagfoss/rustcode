import XCTest
@testable import RustCodeKit

final class ToolFoldingTests: XCTestCase {
    /// `t` a finished call, `x` a failed one, `e` an edit, `a` a spawned
    /// agent, `s` some text, `r` a thought, `u` a message the user sent.
    private func items(_ shape: String) -> [TranscriptItem] {
        var transcript = Transcript()
        for (index, kind) in shape.enumerated() {
            let id = "call-\(index)"
            switch kind {
            case "s":
                transcript.apply(.agentText("step \(index)"))
            case "r":
                transcript.apply(.thoughtText("thought \(index)", summary: true))
            case "u":
                transcript.apply(.userText("ask \(index)"))
            default:
                transcript.apply(.toolCall(id: id, title: kind == "a" ? "spawn_agent" : "run_command", kind: "execute",
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

    func testLowFoldsEveryRunALoneCallToo() {
        let list = items("txtsts tt".replacingOccurrences(of: " ", with: ""))
        let folding = ToolFolding(items: list, verbosity: .low, open: [], running: false)
        let first = list[0].id
        XCTAssertEqual(folding.fold(for: first),
                       ToolRunFold(run: first, count: 3, failed: 1, isOpen: false, label: "Run ×3"))
        XCTAssertEqual(folding.hidden, [list[1].id, list[2].id, list[7].id])
        // A lone call between texts is still `1 tool call`.
        XCTAssertEqual(folding.fold(for: list[4].id)?.count, 1)
        XCTAssertEqual(folding.fold(for: list[4].id)?.label, "Run · cmd 4")
        XCTAssertNotNil(folding.fold(for: list[6].id))
    }

    func testTheLabelNamesEachToolOnceAndALoneCallsSummary() {
        var transcript = Transcript()
        for (index, title) in ["web_search", "web_search", "fetch_url", "view_file", "grep"].enumerated() {
            transcript.apply(.toolCall(id: "c\(index)", title: title, kind: nil, status: .completed,
                                       input: .object(["summary": .string("Step \(index)")])))
        }
        let calls = transcript.items.compactMap { item -> ToolItem? in
            if case .tool(let tool) = item.content { tool } else { nil }
        }
        XCTAssertEqual(ToolRunFold.label(for: calls), "Web search ×2, Fetch url, Read, 1 more")
        XCTAssertEqual(ToolRunFold.label(for: calls.prefix(1)), "Web search · Step 0")
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

    /// Agents started together are one row at every verbosity, and never
    /// fold into a run of tool calls (#2327).
    func testSpawnedAgentsGroupAndStayOutOfTheRuns() {
        let list = items("ttaaatsa")
        for verbosity in ToolVerbosity.allCases {
            let folding = ToolFolding(items: list, verbosity: verbosity, open: [], running: false)
            XCTAssertEqual(folding.agents(for: list[2].id), [list[2].id, list[3].id, list[4].id], "\(verbosity)")
            XCTAssertEqual(folding.agents(for: list[7].id), [list[7].id])
            XCTAssertNil(folding.agents(for: list[3].id))
            XCTAssertTrue(folding.hidden.isSuperset(of: [list[3].id, list[4].id]))
            XCTAssertFalse(folding.hidden.contains(list[2].id))
            XCTAssertNil(folding.fold(for: list[2].id))
        }
        let low = ToolFolding(items: list, verbosity: .low, open: [], running: false)
        XCTAssertEqual(low.fold(for: list[0].id)?.count, 2, "the calls before the agents fold on their own")
        XCTAssertEqual(low.fold(for: list[5].id)?.count, 1, "and so does the call after them")
        // While a turn runs, an agent at the end is still shown.
        let live = ToolFolding(items: Array(list.prefix(3)), verbosity: .low, open: [], running: true)
        XCTAssertEqual(live.agents(for: list[2].id), [list[2].id])
        XCTAssertEqual(live.fold(for: list[0].id)?.count, 2)
    }

    // A turn read as reasoning box, sentence, call, reasoning box, sentence,
    // call…; it now shows its first thought, then what was said and done.
    func testLowShowsOnlyEachTurnsFirstThought() {
        let list = items("urstrstrsurtrt")
        let folding = ToolFolding(items: list, verbosity: .low, open: [], running: false)
        let thoughts = list.filter { if case .thinking = $0.content { true } else { false } }
        XCTAssertEqual(thoughts.count, 5)
        // The first thought of each turn stays; the rest hide.
        XCTAssertFalse(folding.hidden.contains(list[1].id))
        XCTAssertTrue(folding.hidden.contains(list[4].id))
        XCTAssertTrue(folding.hidden.contains(list[7].id))
        XCTAssertFalse(folding.hidden.contains(list[10].id))
        XCTAssertTrue(folding.hidden.contains(list[12].id))
        // Every sentence and each run's first call still show.
        for index in [2, 3, 5, 6, 8, 11] { XCTAssertFalse(folding.hidden.contains(list[index].id)) }
    }

    func testAHiddenThoughtDoesNotSplitARun() {
        let list = items("urtrtrt")
        let folding = ToolFolding(items: list, verbosity: .low, open: [], running: false)
        XCTAssertEqual(folding.fold(for: list[2].id)?.count, 3)
    }

    func testTheLiveThoughtShowsAndHighShowsEveryThought() {
        let list = items("urtsr")
        let live = ToolFolding(items: list, verbosity: .low, open: [], running: true)
        XCTAssertFalse(live.hidden.contains(list[4].id))
        let done = ToolFolding(items: list, verbosity: .low, open: [], running: false)
        XCTAssertTrue(done.hidden.contains(list[4].id))
        let high = ToolFolding(items: list, verbosity: .high, open: [], running: false)
        XCTAssertTrue(high.hidden.isEmpty)
    }
}
