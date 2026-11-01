import XCTest
@testable import RustCodeKit

final class TurnCompletionTests: XCTestCase {
    private func run(_ transcript: inout Transcript, id: String, command: String, status: ToolStatus?) {
        transcript.apply(.toolCall(id: id, title: "run_command", kind: nil, status: .inProgress, input: nil, detail: command))
        if let status { transcript.apply(.toolCallUpdate(id: id, title: nil, status: status, output: "out")) }
    }

    func testRecognizesChecks() {
        for command in ["cargo test --workspace", "scripts/swift-test.sh", "npm run test:unit", "pytest -q",
                        "cargo clippy", "cd app && npx tsc --noEmit", "make lint-all", "swift build"] {
            XCTAssertTrue(TurnCompletion.isCheck(command), command)
        }
        for command in ["ls -la", "git status", "cat test.txt", "echo latest"] {
            XCTAssertFalse(TurnCompletion.isCheck(command), command)
        }
    }

    func testAFinishedTurnSummarizesEditsAndObservedChecks() {
        var transcript = Transcript()
        let start = Date()
        transcript.appendUser("fix it")
        transcript.beginTurn(at: start)
        transcript.apply(.toolCall(id: "e1", title: "edit_file", kind: "edit", status: .inProgress, input: nil))
        transcript.apply(.toolCallUpdate(id: "e1", title: nil, status: .completed, output: "ok",
                                         diffs: [FileDiff(path: "/p/a.rs", oldText: "a\n", newText: "b\nc\n")]))
        run(&transcript, id: "c1", command: "cargo test", status: .completed)
        run(&transcript, id: "c2", command: "swift build", status: .failed)
        run(&transcript, id: "c3", command: "ls", status: .failed)
        run(&transcript, id: "c4", command: "cargo clippy", status: nil)
        transcript.apply(.agentText("Done."))
        transcript.finishTurn(at: start.addingTimeInterval(5))

        let completions = TurnCompletion.byLastReply(transcript.items)
        XCTAssertEqual(completions.count, 1)
        let completion = try! XCTUnwrap(completions.values.first)
        XCTAssertEqual(completion.edited, ["/p/a.rs"])
        XCTAssertEqual(completion.files, [TurnCompletion.FileStat(path: "/p/a.rs", added: completion.added, removed: completion.removed)])
        XCTAssertEqual(completion.checks.map(\.summary.label), ["Ran tests", "Built", "Linted"])
        XCTAssertEqual(completion.checks.map(\.outcome), [.passed, .failed, .unknown])
        XCTAssertEqual(completion.failedChecks, 1)
        XCTAssertEqual(completion.failedCommands, 1)
        XCTAssertFalse(completion.cancelled)
        XCTAssertEqual(completions.keys.first, transcript.items.last?.id, "drawn under the turn's last reply")
    }

    func testAStoppedTurnMarksUnreportedChecksCancelledAndAPlainTurnHasNone() {
        var transcript = Transcript()
        let start = Date()
        transcript.appendUser("check")
        transcript.beginTurn(at: start)
        run(&transcript, id: "c1", command: "cargo test", status: nil)
        transcript.apply(.agentText("Stopping."))
        transcript.finishTurn(at: start.addingTimeInterval(1), cancelled: true)
        transcript.appendUser("hi")
        transcript.beginTurn(at: start.addingTimeInterval(2))
        transcript.apply(.agentText("Hello."))
        transcript.finishTurn(at: start.addingTimeInterval(3))

        let completions = TurnCompletion.byLastReply(transcript.items)
        XCTAssertEqual(completions.count, 1, "a turn with no edits or checks has no card")
        XCTAssertEqual(completions.values.first?.checks.map(\.outcome), [.cancelled])
    }

    /// Two suites that both read "Ran tests" stay apart: the Swift failure
    /// is not hidden by the cargo pass, while a rerun of the same command
    /// still replaces its earlier failure.
    func testLastRunsMatchByCommandNotLabel() {
        var transcript = Transcript()
        let start = Date()
        transcript.appendUser("fix it")
        transcript.beginTurn(at: start)
        run(&transcript, id: "c1", command: "scripts/swift-test.sh", status: .failed)
        run(&transcript, id: "c2", command: "cargo test --workspace", status: .failed)
        run(&transcript, id: "c3", command: "cargo  test   --workspace", status: .completed)
        transcript.apply(.agentText("Done."))
        transcript.finishTurn(at: start.addingTimeInterval(5))

        let completion = try! XCTUnwrap(TurnCompletion.byLastReply(transcript.items).values.first)
        XCTAssertEqual(completion.lastRuns.map(\.outcome), [.failed, .passed])
        XCTAssertEqual(completion.lastRuns.map(\.command), ["scripts/swift-test.sh", "cargo  test   --workspace"])
    }

    /// A commit message fed through a here-document is not a command.
    func testHeredocBodyIsNotACommand() {
        let command = "git commit -F - <<'EOF'\ntest: cover parser\n\nmake it pass\nEOF\ngit push"
        XCTAssertEqual(GitPipeline.segments(command),
                       ["git commit -F - <<'EOF'\ntest: cover parser\n\nmake it pass\nEOF", "git push"])
        XCTAssertFalse(TurnCompletion.isCheck(command))
        XCTAssertEqual(GitPipeline.segments("cat <<< \"x\"; ls"), ["cat <<< \"x\"", "ls"])
    }
}
