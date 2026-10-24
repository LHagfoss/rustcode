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

    func testHighFoldsNothing() {
        let folding = ToolFolding(items: items("tttstt"), verbosity: .high, open: [], running: false)
        XCTAssertTrue(folding.hidden.isEmpty)
        XCTAssertTrue(folding.folds.isEmpty)
    }

    func testLowFoldsEveryTurnALoneCallToo() {
        let list = items("utxtsutsut")
        let folding = ToolFolding(items: list, verbosity: .low, open: [], running: false)
        let first = list[1].id
        XCTAssertEqual(folding.fold(for: first),
                       ToolRunFold(run: first, count: 3, failed: 1, isOpen: false, label: "Run ×3"))
        XCTAssertEqual(folding.hidden, [list[2].id, list[3].id])
        // A turn with one call is still `1 tool call`.
        XCTAssertEqual(folding.fold(for: list[6].id)?.count, 1)
        XCTAssertEqual(folding.fold(for: list[6].id)?.label, "Run · cmd 6")
        XCTAssertNotNil(folding.fold(for: list[9].id))
    }

    // A turn read as sentence, call, sentence, call…, a paragraph per call.
    // Finished, it reads as what the model said first, one row for all its
    // calls, and its answer.
    func testAFinishedTurnIsItsFirstWordsOneRowAndItsAnswer() {
        let list = items("usttststs")
        let folding = ToolFolding(items: list, verbosity: .low, open: [], running: false)
        XCTAssertEqual(folding.fold(for: list[2].id)?.count, 4)
        XCTAssertEqual(folding.folds.count, 1)
        XCTAssertEqual(folding.hidden, Set([3, 4, 5, 6, 7].map { list[$0].id }))
        XCTAssertFalse(folding.hidden.contains(list[1].id), "what it said first shows")
        XCTAssertFalse(folding.hidden.contains(list[8].id), "the answer shows")
        // Opened, the row shows everything in between.
        let open = ToolFolding(items: list, verbosity: .low, open: [list[2].id], running: false)
        XCTAssertTrue(open.hidden.isEmpty)
        XCTAssertTrue(ToolFolding(items: list, verbosity: .high, open: [], running: false).hidden.isEmpty)
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
        // The first call of a live turn folds even alone.
        let two = items("tt")
        XCTAssertEqual(ToolFolding(items: two, verbosity: .low, open: [], running: true).fold(for: two[0].id)?.count, 1)
    }

    // While the turn runs, its current round shows: the running call and what
    // the model says about it. Earlier rounds are in the row.
    func testWhileRunningTheCurrentRoundShows() {
        let list = items("ststst")
        let folding = ToolFolding(items: list, verbosity: .low, open: [], running: true)
        XCTAssertEqual(folding.fold(for: list[1].id)?.count, 2)
        XCTAssertEqual(folding.hidden, Set([2, 3].map { list[$0].id }))
        // The first call running alone folds nothing.
        XCTAssertTrue(ToolFolding(items: Array(list.prefix(2)), verbosity: .low, open: [], running: true)
            .hidden.isEmpty)
        // What it says after the current call shows while it streams.
        let speaking = ToolFolding(items: Array(list.prefix(5)), verbosity: .low, open: [], running: true)
        XCTAssertFalse(speaking.hidden.contains(list[4].id))
        XCTAssertFalse(speaking.hidden.contains(list[2].id), "it is about the current call")
    }

    // At `edits`, a running turn shows its edits; a finished one folds them.
    func testEditsShowWhileTheTurnRunsAndFoldWhenItIsDone() {
        let list = items("utetes")
        let live = ToolFolding(items: list, verbosity: .edits, open: [], running: true)
        XCTAssertEqual(live.fold(for: list[1].id)?.count, 2)
        XCTAssertFalse(live.hidden.contains(list[2].id), "the earlier edit shows")
        XCTAssertFalse(live.hidden.contains(list[4].id), "the current edit shows")
        XCTAssertTrue(live.hidden.contains(list[3].id))
        let done = ToolFolding(items: list, verbosity: .edits, open: [], running: false)
        XCTAssertEqual(done.fold(for: list[1].id)?.count, 4)
        XCTAssertEqual(done.hidden, Set([2, 3, 4].map { list[$0].id }))
        // At low, a running turn folds its edits too.
        XCTAssertTrue(ToolFolding(items: list, verbosity: .low, open: [], running: true).hidden.contains(list[2].id))
    }

    func testReadsTheTerminalsSetting() {
        XCTAssertEqual(ToolVerbosity.configured(in: "theme = \"x\"\nverbosity = \"edits\"\n"), .edits)
        XCTAssertNil(ToolVerbosity.configured(in: "theme = \"x\""))
        XCTAssertNil(ToolVerbosity.configured(in: "verbosity = \"loud\""))
    }

    /// Agents started together are one row at every verbosity, and never
    /// fold into the turn's row (#2327).
    func testSpawnedAgentsGroupAndStayOutOfTheRow() {
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
        XCTAssertEqual(low.fold(for: list[0].id)?.count, 3, "the calls around the agents are one row")
        XCTAssertTrue(low.hidden.contains(list[5].id))
        // While a turn runs, an agent at the end is still shown.
        let live = ToolFolding(items: Array(list.prefix(3)), verbosity: .low, open: [], running: true)
        XCTAssertEqual(live.agents(for: list[2].id), [list[2].id])
        XCTAssertEqual(live.fold(for: list[0].id)?.count, 1)
    }

    func testLowShowsOnlyEachTurnsFirstThought() {
        let list = items("urstrstrsurtrt")
        let folding = ToolFolding(items: list, verbosity: .low, open: [], running: false)
        for index in [4, 7, 12] { XCTAssertTrue(folding.hidden.contains(list[index].id)) }
        // Each turn's first thought, first words, row, and answer show.
        for index in [1, 2, 3, 8, 10, 11] { XCTAssertFalse(folding.hidden.contains(list[index].id)) }
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
