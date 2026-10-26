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

    private func editRun(_ calls: [(String, ToolStatus, String?)]) -> [ToolItem] {
        var transcript = Transcript()
        for (index, (title, status, path)) in calls.enumerated() {
            transcript.apply(.toolCall(id: "c\(index)", title: title, kind: nil, status: status, input: nil))
            if let path {
                transcript.apply(.toolCallUpdate(id: "c\(index)", title: nil, status: status, output: nil,
                                                 diffs: [FileDiff(path: path, oldText: "a\nb\n", newText: "a\nc\nd\n")]))
            }
        }
        return transcript.items.compactMap { item -> ToolItem? in
            if case .tool(let tool) = item.content { tool } else { nil }
        }
    }

    func testARunNamesTheFilesItEditedAndTheirLines() {
        let calls = editRun([("replace_file_content", .completed, "/p/src/game.js"), ("view_file", .completed, nil),
                             ("replace_file_content", .completed, "/p/src/main.jsx"),
                             ("replace_file_content", .completed, "/p/src/game.js"),
                             ("replace_file_content", .failed, nil)])
        XCTAssertEqual(ToolRunFold.edited(by: calls), ["/p/src/game.js", "/p/src/main.jsx"])
        XCTAssertEqual(ToolRunFold.lines(of: calls), .init(added: 6, removed: 3))
    }

    func testARunLeavesOutLinesWhenAnEditCameBackWithoutADiff() {
        let calls = editRun([("replace_file_content", .completed, "/p/a.js"), ("replace_file_content", .completed, nil)])
        XCTAssertEqual(ToolRunFold.edited(by: calls), ["/p/a.js"])
        XCTAssertNil(ToolRunFold.lines(of: calls))
        XCTAssertNil(ToolRunFold.lines(of: editRun([("view_file", .completed, nil)])))
    }

    private func calls(_ titles: [String], detail: Bool = true) -> [ToolItem] {
        var transcript = Transcript()
        for (index, title) in titles.enumerated() {
            transcript.apply(.toolCall(id: "c\(index)", title: title, kind: nil, status: .completed,
                                       input: .object(["summary": .string("Step \(index)")]),
                                       detail: detail ? "/p/src/file\(index).js" : nil))
        }
        return transcript.items.compactMap { item -> ToolItem? in
            if case .tool(let tool) = item.content { tool } else { nil }
        }
    }

    // A run says what it did in words: each kind once, with its count.
    func testARunSaysWhatItDid() {
        let run = calls(["view_file", "view_file", "grep", "run_command", "view_file", "mcp__worklog__list"])
        XCTAssertEqual(ToolPhrase.summary(run), "Read 3 files, searched code, ran 1 command, used 1 tool")
        XCTAssertEqual(ToolPhrase.phrase(run[2], now: false), "Step 2", "a search reads as what it looks for")
        XCTAssertEqual(ToolPhrase.summary(run.prefix(1)), "Read file0.js")
        XCTAssertEqual(ToolPhrase.phrase(run[3], now: true), "Running /p/src/file3.js")
        XCTAssertEqual(ToolPhrase.phrase(calls(["mcp__worklog__list"])[0], now: false), "Step 0")
        let edits = editRun([("replace_file_content", .completed, "/p/a.js"), ("replace_file_content", .completed, "/p/a.js"),
                             ("replace_file_content", .completed, "/p/b.js")])
        XCTAssertEqual(ToolPhrase.summary(edits), "Edited 2 files", "a file edited twice counts once")
        XCTAssertEqual(ToolPhrase.summary(edits.prefix(1)), "Edited a.js")
    }

    func testTheWorkedRowSaysHowLongTheTurnTook() {
        XCTAssertEqual(TurnFold(anchor: 0, calls: 3, failed: 0, isOpen: false, elapsed: 89).title, "Worked for 1m 29s")
        XCTAssertEqual(TurnFold(anchor: 0, calls: 3, failed: 0, isOpen: false, elapsed: 12).title, "Worked for 12s")
        XCTAssertEqual(TurnFold(anchor: 0, calls: 1, failed: 0, isOpen: false).title, "Worked · 1 tool call")
    }

    func testHighFoldsNothing() {
        let folding = ToolFolding(items: items("tttstt"), verbosity: .high, open: [], running: false)
        XCTAssertTrue(folding.hidden.isEmpty)
        XCTAssertTrue(folding.folds.isEmpty)
        XCTAssertTrue(folding.turns.isEmpty)
    }

    private func key(_ item: TranscriptItem) -> Int { TurnFold.key(for: item.id) }

    // #2435: a finished turn reads as one `Worked for…` row and its answer;
    // what the model said first is inside the row.
    func testAFinishedTurnFoldsIntoOneRow() {
        let list = items("urusttststs").suffix(9).map { $0 }
        let folding = ToolFolding(items: Array(list), verbosity: .low, open: [], running: false)
        let turn = folding.turn(for: list[1].id)
        XCTAssertEqual(turn?.calls, 4)
        XCTAssertEqual(turn?.isOpen, false)
        XCTAssertTrue(folding.hidden.isSuperset(of: [2, 3, 4, 5, 6, 7].map { list[$0].id }))
        for index in [0, 1, 8] { XCTAssertFalse(folding.hidden.contains(list[index].id), "\(index) shows") }
        XCTAssertTrue(folding.nested.isEmpty)
        // A turn of one call folds too, and a thought can carry the row.
        let one = items("usts")
        XCTAssertEqual(ToolFolding(items: one, verbosity: .low, open: [], running: false).turn(for: one[1].id)?.calls, 1)
        let thought = items("urtts")
        XCTAssertEqual(ToolFolding(items: thought, verbosity: .low, open: [], running: false).turn(for: thought[1].id)?.calls, 2)
    }

    // Opened, the work shows as it happened: what was said and thought
    // between steps, and each run of calls as one row of what it did.
    func testAnOpenedTurnShowsItsWork() {
        let list = items("usttrtrststs")
        let folding = ToolFolding(items: list, verbosity: .low, open: [key(list[1])], running: false)
        XCTAssertEqual(folding.turn(for: list[1].id)?.isOpen, true)
        XCTAssertEqual(folding.hidden, [list[3].id, list[4].id, list[5].id], "each run stays one row, its thoughts in it")
        XCTAssertEqual(folding.fold(for: list[2].id)?.label, "Ran 3 commands")
        XCTAssertEqual(folding.fold(for: list[8].id)?.label, "Ran cmd 8")
        XCTAssertEqual(folding.nested, Set((2...10).map { list[$0].id }))
        XCTAssertFalse(folding.hidden.contains(list[6].id), "the thought before the model speaks shows")
        XCTAssertFalse(folding.isNested(list[11].id), "the answer is the turn's own")
        let both = ToolFolding(items: list, verbosity: .low, open: [key(list[1]), list[2].id], running: false)
        XCTAssertEqual(both.hidden, [list[4].id], "an opened run shows its calls")
    }

    // A turn that stopped before its answer ends on its last words, under
    // the row rather than with the row as its end.
    func testATurnWithoutAnAnswerEndsOnItsLastWords() {
        let list = items("ustststt")
        let folding = ToolFolding(items: list, verbosity: .low, open: [], running: false)
        XCTAssertNotNil(folding.turn(for: list[1].id))
        XCTAssertFalse(folding.hidden.contains(list[5].id), "the last words show")
        XCTAssertTrue(folding.hidden.contains(list[3].id))
        XCTAssertTrue(folding.hidden.isSuperset(of: [list[6].id, list[7].id]))
    }

    // While it runs, each round's calls fold into a summary while the model's
    // messages stay visible and the current round's calls remain expanded.
    func testARunningTurnShowsItsRoundsAndMessages() {
        let list = items("urstrsttr")
        let folding = ToolFolding(items: list, verbosity: .low, open: [], running: true)
        XCTAssertTrue(folding.turns.isEmpty)
        XCTAssertTrue(folding.nested.isEmpty)
        for index in [1, 4, 8] { XCTAssertTrue(folding.hidden.contains(list[index].id), "thought \(index) waits") }
        for index in [2, 3, 5, 6] { XCTAssertFalse(folding.hidden.contains(list[index].id), "\(index) shows") }
        XCTAssertEqual(folding.fold(for: list[6].id)?.label, "Ran 2 commands")
        XCTAssertTrue(folding.hidden.contains(list[7].id))
        XCTAssertEqual(folding.activity, "Thinking", "the thought it is writing")
    }

    // The current round shows its calls, including a running call, while the
    // line under the turn says what is happening now.
    func testTheRunningCallIsVisibleAlongsideItsActivity() {
        var transcript = Transcript()
        transcript.apply(.userText("test it"))
        transcript.apply(.toolCall(id: "a", title: "run_command", kind: "execute", status: .completed, input: nil, detail: "npm run build"))
        transcript.apply(.toolCall(id: "b", title: "run_command", kind: "execute", status: .inProgress, input: nil, detail: "npm test"))
        let list = transcript.items
        let live = ToolFolding(items: list, verbosity: .low, open: [], running: true)
        XCTAssertEqual(live.activity, "Running npm test")
        XCTAssertFalse(live.hidden.contains(list[1].id))
        XCTAssertFalse(live.hidden.contains(list[2].id))
        XCTAssertNil(live.fold(for: list[1].id), "the current round is not folded")
        XCTAssertNil(ToolFolding(items: list, verbosity: .low, open: [], running: false).activity)
    }

    // A completed round folds as the next one starts; opening its summary
    // shows the calls again. Finished turns keep their separate turn summary.
    func testAFinishedRoundFoldsAsTheNextStarts() {
        let list = items("urttrt")
        let live = ToolFolding(items: list, verbosity: .low, open: [], running: true)
        XCTAssertEqual(live.fold(for: list[2].id)?.label, "Ran 2 commands")
        XCTAssertTrue(live.hidden.contains(list[3].id), "the finished round is folded")
        XCTAssertNil(live.fold(for: list[5].id))
        XCTAssertFalse(live.hidden.contains(list[5].id), "the current round shows its call")

        let opened = ToolFolding(items: list, verbosity: .low, open: [list[2].id], running: true)
        XCTAssertFalse(opened.hidden.contains(list[3].id))

        let thinking = ToolFolding(items: items("urttrtr"), verbosity: .low, open: [], running: true)
        XCTAssertEqual(thinking.fold(for: list[2].id)?.count, 3, "thoughts do not split a run")

        let done = ToolFolding(items: list, verbosity: .low, open: [], running: false)
        XCTAssertNotNil(done.turn(for: list[1].id), "the finished turn still folds as before")
    }

    func testTheActivityIsTheHeadingOfTheThought() {
        XCTAssertEqual(CurrentWork.heading(of: "**Checking tests**\n\nThe suite\n\n**Reading lint output**\n\nIt"),
                       "Reading lint output")
        XCTAssertEqual(CurrentWork.heading(of: "plain reasoning"), "Thinking")
    }

    // The screenshots a preview took come back as a message the engine adds,
    // which is part of the turn, not a new one.
    func testAToolsScreenshotDoesNotStartATurn() {
        var transcript = Transcript()
        transcript.apply(.userText("make it red"))
        transcript.apply(.agentText("Checking the page."))
        for id in ["a", "b"] {
            transcript.apply(.toolCall(id: id, title: "localhost_preview", kind: "fetch", status: .completed, input: nil))
        }
        transcript.apply(.userText("[Attached by the localhost_preview tool, not typed by the user: the screenshot(s) it just took]\n![image](file:///tmp/a.png)"))
        transcript.apply(.agentText("It is red now."))
        let list = transcript.items
        let folding = ToolFolding(items: list, verbosity: .low, open: [], running: false)
        XCTAssertEqual(folding.turns.count, 1)
        XCTAssertTrue(folding.hidden.contains(list[4].id), "the screenshot is inside the turn's row")
        XCTAssertFalse(folding.hidden.contains(list[5].id), "the answer shows")
    }

    // What the model said after its first words, before any call, is work.
    // #2435: the first words go in the row with the rest of the work, so
    // nothing but the row stands between the question and the answer.
    func testTheFirstWordsAreInTheRow() {
        let list = items("usrstts")
        let folding = ToolFolding(items: list, verbosity: .low, open: [], running: false)
        XCTAssertEqual(folding.turn(for: list[1].id)?.calls, 2)
        XCTAssertTrue(folding.hidden.isSuperset(of: (2...5).map { list[$0].id }))
        XCTAssertFalse(folding.hidden.contains(list[1].id), "the row is drawn on it")
        XCTAssertFalse(folding.hidden.contains(list[6].id), "the answer shows")
        let open = ToolFolding(items: list, verbosity: .low, open: [key(list[1])], running: false)
        XCTAssertFalse(open.hidden.contains(list[2].id), "opened, the thought shows between steps")
        let reply = items("urs")
        XCTAssertTrue(ToolFolding(items: reply, verbosity: .low, open: [], running: false).hidden.isEmpty,
                      "a reply without calls keeps its thought")
    }

    // At `edits` an edit is its own row with its diff; at `low` it joins the run.
    func testEditsAreTheirOwnRowAtEdits() {
        let list = items("utets")
        let opened = ToolFolding(items: list, verbosity: .edits, open: [key(list[1])], running: false)
        XCTAssertNil(opened.fold(for: list[2].id), "the edit shows with its diff")
        XCTAssertFalse(opened.hidden.contains(list[2].id))
        let low = ToolFolding(items: list, verbosity: .low, open: [], running: false)
        XCTAssertEqual(low.fold(for: list[1].id)?.label, "Ran 2 commands, edited 1 file")
    }

    func testReadsTheTerminalsSetting() {
        XCTAssertEqual(ToolVerbosity.configured(in: "theme = \"x\"\nverbosity = \"edits\"\n"), .edits)
        XCTAssertNil(ToolVerbosity.configured(in: "theme = \"x\""))
        XCTAssertNil(ToolVerbosity.configured(in: "verbosity = \"loud\""))
    }

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
        let low = ToolFolding(items: list, verbosity: .low, open: [], running: false)
        XCTAssertEqual(low.turn(for: list[0].id)?.calls, 3)
        XCTAssertTrue(low.hidden.contains(list[2].id))
        XCTAssertFalse(low.hidden.contains(list[7].id), "the agents after the answer show")
        let live = ToolFolding(items: Array(list.prefix(3)), verbosity: .low, open: [], running: true)
        XCTAssertFalse(live.hidden.contains(list[2].id), "agents started now show")
        XCTAssertFalse(live.hidden.contains(list[1].id), "current-round calls stay visible")
    }

    func testAThoughtDoesNotSplitARun() {
        let list = items("urtrtrt")
        let folding = ToolFolding(items: list, verbosity: .low, open: [key(list[2])], running: false)
        XCTAssertEqual(folding.fold(for: list[2].id)?.count, 3)
    }
}
