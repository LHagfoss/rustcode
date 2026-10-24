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
        XCTAssertTrue(folding.turns.isEmpty)
    }

    private func key(_ item: TranscriptItem) -> Int { TurnFold.key(for: item.id) }

    // A finished turn reads as what the model said first, one row for its
    // work, and its answer (#2357).
    func testAFinishedTurnFoldsIntoOneRow() {
        let list = items("usttststs")
        let folding = ToolFolding(items: list, verbosity: .low, open: [], running: false)
        let turn = folding.turn(for: list[2].id)
        XCTAssertEqual(turn?.calls, 4)
        XCTAssertEqual(turn?.steps, 3)
        XCTAssertEqual(turn?.isOpen, false)
        XCTAssertTrue(folding.hidden.isSuperset(of: [3, 4, 5, 6, 7].map { list[$0].id }))
        for index in [1, 2, 8] { XCTAssertFalse(folding.hidden.contains(list[index].id), "\(index) shows") }
        XCTAssertTrue(folding.nested.isEmpty)
    }

    // Opened, the row lists the steps: each thing said and its own folded run.
    func testAnOpenedTurnListsItsSteps() {
        let list = items("usttststs")
        let folding = ToolFolding(items: list, verbosity: .low, open: [key(list[2])], running: false)
        XCTAssertEqual(folding.turn(for: list[2].id)?.isOpen, true)
        XCTAssertEqual(folding.hidden, [list[3].id], "each step's run stays folded")
        XCTAssertEqual(folding.fold(for: list[2].id)?.count, 2)
        XCTAssertEqual(folding.fold(for: list[5].id)?.count, 1)
        XCTAssertEqual(folding.nested, Set([3, 4, 5, 6, 7].map { list[$0].id }))
        XCTAssertFalse(folding.isNested(list[8].id), "the answer is the turn's own")
        // A step opened inside the turn shows its calls.
        let both = ToolFolding(items: list, verbosity: .low, open: [key(list[2]), list[2].id], running: false)
        XCTAssertTrue(both.hidden.isEmpty)
    }

    // What the model said after its first words, before any call, is a step.
    func testOnlyTheFirstWordsStayOutOfTheRow() {
        let list = items("ursrstts")
        let folding = ToolFolding(items: list, verbosity: .low, open: [], running: false)
        XCTAssertEqual(folding.turn(for: list[4].id)?.calls, 2)
        XCTAssertTrue(folding.hidden.isSuperset(of: [list[5].id, list[6].id]))
        XCTAssertFalse(folding.hidden.contains(list[2].id))
        XCTAssertFalse(folding.hidden.contains(list[7].id))
    }

    func testATurnWithOneCallIsItsStep() {
        let list = items("utxtsutsut")
        let folding = ToolFolding(items: list, verbosity: .low, open: [], running: false)
        XCTAssertEqual(folding.turn(for: list[1].id)?.calls, 3)
        XCTAssertNil(folding.turn(for: list[6].id))
        XCTAssertEqual(folding.fold(for: list[6].id)?.count, 1)
        XCTAssertEqual(folding.fold(for: list[6].id)?.label, "Run · cmd 6")
        XCTAssertNotNil(folding.fold(for: list[9].id))
    }

    func testWhileRunningOnlyTheCurrentCallShows() {
        let list = items("sttt")
        let folding = ToolFolding(items: list, verbosity: .low, open: [], running: true)
        XCTAssertTrue(folding.turns.isEmpty)
        XCTAssertEqual(folding.fold(for: list[1].id)?.count, 2)
        XCTAssertEqual(folding.hidden, [list[2].id])
        let two = items("tt")
        XCTAssertEqual(ToolFolding(items: two, verbosity: .low, open: [], running: true).fold(for: two[0].id)?.count, 1)
    }

    // A running turn shows its steps as they happen.
    func testWhileRunningTheStepsShow() {
        let list = items("ststst")
        let folding = ToolFolding(items: list, verbosity: .low, open: [], running: true)
        XCTAssertTrue(folding.turns.isEmpty)
        XCTAssertTrue(folding.hidden.isEmpty)
        XCTAssertEqual(folding.fold(for: list[1].id)?.count, 1)
        XCTAssertEqual(folding.fold(for: list[3].id)?.count, 1)
        XCTAssertNil(folding.fold(for: list[5].id), "the current call shows")
    }

    // At `edits` an edit is its own step with its diff, during the turn and
    // inside its row once opened; at `low` it folds with the calls.
    func testEditsAreTheirOwnStepAtEdits() {
        let list = items("utetes")
        let live = ToolFolding(items: list, verbosity: .edits, open: [], running: true)
        XCTAssertTrue(live.hidden.isEmpty)
        let done = ToolFolding(items: list, verbosity: .edits, open: [], running: false)
        XCTAssertEqual(done.turn(for: list[1].id)?.calls, 4)
        XCTAssertEqual(done.turn(for: list[1].id)?.steps, 4)
        XCTAssertEqual(done.hidden, Set([2, 3, 4].map { list[$0].id }))
        let opened = ToolFolding(items: list, verbosity: .edits, open: [key(list[1])], running: false)
        XCTAssertTrue(opened.hidden.isEmpty, "the edits show with their diffs")
        let low = ToolFolding(items: list, verbosity: .low, open: [], running: true)
        XCTAssertEqual(low.hidden, Set([2, 3].map { list[$0].id }))
    }

    func testReadsTheTerminalsSetting() {
        XCTAssertEqual(ToolVerbosity.configured(in: "theme = \"x\"\nverbosity = \"edits\"\n"), .edits)
        XCTAssertNil(ToolVerbosity.configured(in: "theme = \"x\""))
        XCTAssertNil(ToolVerbosity.configured(in: "verbosity = \"loud\""))
    }

    /// Agents started together are one row at every verbosity, and never
    /// fold into the turn's row (#2327).
    /// Agents started together are one row at every verbosity (#2327); a
    /// finished turn's row holds them with its other work.
    func testSpawnedAgentsGroup() {
        let list = items("ttaaatsa")
        for verbosity in ToolVerbosity.allCases {
            let folding = ToolFolding(items: list, verbosity: verbosity, open: [], running: false)
            XCTAssertEqual(folding.agents(for: list[2].id), [list[2].id, list[3].id, list[4].id], "\(verbosity)")
            XCTAssertEqual(folding.agents(for: list[7].id), [list[7].id])
            XCTAssertNil(folding.agents(for: list[3].id))
            XCTAssertTrue(folding.hidden.isSuperset(of: [list[3].id, list[4].id]))
            XCTAssertNil(folding.fold(for: list[2].id))
        }
        let high = ToolFolding(items: list, verbosity: .high, open: [], running: false)
        XCTAssertFalse(high.hidden.contains(list[2].id))
        let low = ToolFolding(items: list, verbosity: .low, open: [], running: false)
        XCTAssertEqual(low.turn(for: list[0].id)?.calls, 3)
        XCTAssertTrue(low.hidden.contains(list[2].id))
        XCTAssertFalse(low.hidden.contains(list[7].id), "the agents after the answer show")
        let live = ToolFolding(items: Array(list.prefix(3)), verbosity: .low, open: [], running: true)
        XCTAssertEqual(live.agents(for: list[2].id), [list[2].id])
        XCTAssertFalse(live.hidden.contains(list[2].id))
        XCTAssertEqual(live.fold(for: list[0].id)?.count, 1)
    }

    func testLowShowsOnlyEachTurnsFirstThought() {
        let list = items("urstrstrsurtrt")
        let folding = ToolFolding(items: list, verbosity: .low, open: [], running: false)
        for index in [4, 7, 12] { XCTAssertTrue(folding.hidden.contains(list[index].id)) }
        for index in [1, 2, 3, 8, 10, 11] { XCTAssertFalse(folding.hidden.contains(list[index].id)) }
    }

    func testAHiddenThoughtDoesNotSplitARun() {
        let list = items("urtrtrt")
        let folding = ToolFolding(items: list, verbosity: .low, open: [], running: false)
        XCTAssertEqual(folding.fold(for: list[2].id)?.count, 3)
        XCTAssertEqual(folding.turn(for: list[2].id)?.steps, 1)
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
