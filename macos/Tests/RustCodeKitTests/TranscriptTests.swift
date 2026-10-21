import XCTest
@testable import RustCodeKit

final class TranscriptTests: XCTestCase {
    func testStreamedChunksJoinTheItemTheyContinue() {
        var transcript = Transcript()
        transcript.appendUser("hi")
        transcript.apply(.thoughtText("Let me "))
        transcript.apply(.thoughtText("think."))
        transcript.apply(.agentText("Hel"))
        transcript.apply(.agentText("lo"))
        XCTAssertEqual(transcript.items.map(\.content), [.user("hi"), .thinking("Let me think."), .assistant("Hello")])
    }

    func testLiveThoughtIsTimedUntilWhatFollowsIt() {
        let start = Date(timeIntervalSince1970: 1_000)
        var transcript = Transcript()
        transcript.apply(.thoughtText("Let me "), at: start)
        transcript.apply(.thoughtText("think."), at: start.addingTimeInterval(4))
        XCTAssertEqual(transcript.items[0].startedAt, start)
        XCTAssertNil(transcript.items[0].duration)
        transcript.apply(.agentText("Hello"), at: start.addingTimeInterval(12))
        XCTAssertEqual(transcript.items[0].duration, 12)
        XCTAssertNil(transcript.items[1].duration)
    }

    func testThoughtEndingTheTurnStopsAtTheEnd() {
        let start = Date(timeIntervalSince1970: 1_000)
        var transcript = Transcript()
        transcript.apply(.thoughtText("Hmm"), at: start)
        transcript.finishThinking(at: start.addingTimeInterval(3))
        transcript.finishThinking(at: start.addingTimeInterval(9))
        XCTAssertEqual(transcript.items[0].duration, 3)
    }

    func testReplayedThoughtHasNoTiming() {
        var transcript = Transcript()
        transcript.apply(.thoughtText("Hmm"))
        transcript.apply(.agentText("Done"))
        transcript.finishThinking(at: Date())
        XCTAssertNil(transcript.items[0].startedAt)
        XCTAssertNil(transcript.items[0].duration)
    }

    func testThoughtLabelReadsLikeADuration() {
        XCTAssertEqual(TranscriptItem.thoughtLabel(duration: 0.2), "Thought for 1s")
        XCTAssertEqual(TranscriptItem.thoughtLabel(duration: 12.4), "Thought for 12s")
        XCTAssertEqual(TranscriptItem.thoughtLabel(duration: 120), "Thought for 2m")
        XCTAssertEqual(TranscriptItem.thoughtLabel(duration: 95), "Thought for 1m 35s")
    }

    func testToolUpdatesChangeTheirCardInPlace() {
        var transcript = Transcript()
        transcript.apply(.toolCall(id: "t1", title: "run_command", kind: "execute", status: .inProgress, input: ["cmd": "ls"]))
        transcript.apply(.agentText("Running."))
        transcript.apply(.toolCallUpdate(id: "t1", title: nil, status: .completed, output: "a\nb"))
        XCTAssertEqual(transcript.items.count, 2)
        guard case .tool(let tool) = transcript.items[0].content else { return XCTFail("expected a tool card") }
        XCTAssertEqual(tool.title, "run_command")
        XCTAssertEqual(tool.status, .completed)
        XCTAssertEqual(tool.output, "a\nb")
        XCTAssertNotNil(tool.input)
    }

    private func tools(_ transcript: Transcript) -> [ToolItem] {
        transcript.items.compactMap { if case .tool(let tool) = $0.content { tool } else { nil } }
    }

    func testTurnEndFinishesACardWhoseUpdateNeverCame() {
        var transcript = Transcript()
        transcript.apply(.toolCall(id: "t1", title: "run_command", kind: "execute", status: .inProgress, input: ["cmd": "ls"]))
        transcript.apply(.toolCallUpdate(id: "t2", title: nil, status: .pending, output: nil))
        transcript.apply(.toolCallUpdate(id: "t3", title: nil, status: nil, output: nil, outputDelta: "a\n"))
        transcript.apply(.toolCallUpdate(id: "t3", title: nil, status: .inProgress, output: nil))
        transcript.apply(.agentText("Here is the chart."))
        transcript.finishTurn(at: Date())
        XCTAssertEqual(tools(transcript).map(\.status), [.completed, .completed, .completed])
        XCTAssertFalse(tools(transcript).contains(where: \.isBlank))
    }

    func testCancelledTurnFailsItsEmptyCardsAndKeepsOutput() {
        var transcript = Transcript()
        transcript.apply(.toolCall(id: "t1", title: "run_command", kind: nil, status: .inProgress, input: nil))
        transcript.apply(.toolCall(id: "t2", title: "run_command", kind: nil, status: .inProgress, input: nil))
        transcript.apply(.toolCallUpdate(id: "t2", title: nil, status: nil, output: nil, outputDelta: "half\n"))
        transcript.apply(.toolCall(id: "t3", title: "read_file", kind: nil, status: .completed, input: nil))
        transcript.finishTurn(at: Date(), failure: "Cancelled.")
        let cards = tools(transcript)
        XCTAssertEqual(cards.map(\.status), [.failed, .completed, .completed])
        XCTAssertEqual(cards[0].output, "Cancelled.")
        XCTAssertEqual(cards[1].output, "half\n")
        XCTAssertNil(cards[2].output)
    }

    func testOnlyARunningCardWithNothingToShowIsBlank() {
        var transcript = Transcript()
        transcript.apply(.toolCallUpdate(id: "t1", title: "Compressed release asset", status: .inProgress, output: nil))
        transcript.apply(.toolCall(id: "t2", title: "run_command", kind: nil, status: .inProgress, input: nil, detail: "cargo test"))
        transcript.apply(.toolCall(id: "t3", title: "run_command", kind: nil, status: .completed, input: nil))
        XCTAssertEqual(tools(transcript).map(\.isBlank), [true, false, false])
    }

    func testRunningToolShowsItsDetailAndStreamsOutputUntilTheResult() {
        var transcript = Transcript()
        transcript.apply(.toolCall(id: "t1", title: "run_command", kind: nil, status: .inProgress, input: nil, detail: "cargo test"))
        transcript.apply(.toolCallUpdate(id: "t1", title: nil, status: nil, output: nil, outputDelta: "Compiling\n"))
        transcript.apply(.toolCallUpdate(id: "t1", title: nil, status: nil, output: nil, outputDelta: "Running\n"))
        guard case .tool(let live) = transcript.items[0].content else { return XCTFail("expected a tool card") }
        XCTAssertEqual(live.detail, "cargo test")
        XCTAssertEqual(live.output, "Compiling\nRunning\n")
        transcript.apply(.toolCallUpdate(id: "t1", title: nil, status: .completed, output: "ok"))
        guard case .tool(let done) = transcript.items[0].content else { return XCTFail("expected a tool card") }
        XCTAssertEqual(done.output, "ok")
        XCTAssertEqual(done.detail, "cargo test")
    }

    func testFindMatchesMessagesAndToolOutputIgnoringCase() {
        var transcript = Transcript()
        transcript.appendUser("Fix the Parser")
        transcript.apply(.toolCall(id: "t", title: "grep", kind: nil, status: .completed, input: nil))
        transcript.apply(.toolCallUpdate(id: "t", title: nil, status: nil, output: "src/parser.rs"))
        transcript.apply(.agentText("Done."))
        XCTAssertEqual(transcript.matches("parser"), [0, 1])
        XCTAssertEqual(transcript.matches("  "), [])
    }

    func testSummaryAfterProseIsSplitOff() {
        let text = #"Done here.{"summary":"Caught up.","next_action":null}"#
        let (prose, summary) = AssistantSummary.split(text)
        XCTAssertEqual(prose, "Done here.")
        XCTAssertEqual(summary, AssistantSummary(summary: "Caught up.", nextAction: nil))
        XCTAssertNil(AssistantSummary.split(#"Use {"a": 1} here"#).summary)
        XCTAssertNil(AssistantSummary.split(#"{"summary":"x","extra":1}"#).summary)
    }

    func testCommandOutputJoinsItsCommand() {
        var transcript = Transcript()
        transcript.appendCommand("/status")
        transcript.apply(.agentText("**Status**\n\nok"))
        XCTAssertEqual(transcript.items.map(\.content), [.command(input: "/status", output: "**Status**\n\nok")])
    }

    func testUpdateForUnknownCallStillShows() {
        var transcript = Transcript()
        transcript.apply(.toolCallUpdate(id: "late", title: nil, status: .failed, output: "boom"))
        guard case .tool(let tool) = transcript.items.first?.content else { return XCTFail("expected a tool card") }
        XCTAssertEqual(tool.status, .failed)
        XCTAssertEqual(tool.output, "boom")
    }

    func testMarkdownSplitsBlocksAndKeepsUnterminatedFence() {
        XCTAssertEqual(MarkdownBlock.parse("# Notes\n- one\n```swift\nlet a = 1"), [
            .heading(level: 1, text: "Notes"),
            .listItem(marker: "•", text: "one"),
            .code(language: "swift", text: "let a = 1"),
        ])
    }
}

final class PresentationTests: XCTestCase {
    func testToolCardsNameTheToolAndWhatItActedOn() {
        XCTAssertEqual(ToolPresentation.title("run_command"), "Run")
        XCTAssertEqual(ToolPresentation.title("mcp__github__create_issue"), "github · create_issue")
        XCTAssertEqual(ToolPresentation.title("Read src/main.rs"), "Read src/main.rs")
        XCTAssertEqual(ToolPresentation.detail(input: ["command": "git log\n--oneline"], output: nil), "git log")
        XCTAssertEqual(ToolPresentation.detail(input: nil, output: "\nfirst\nsecond"), "first")
    }

    func testInlineThinkingIsSplitFromTheAnswer() {
        XCTAssertEqual(AssistantBlock.parse("<think>\nplan\n</think>\nAnswer"), [
            .thinking(text: "\nplan\n", isComplete: true),
            .markdown("\nAnswer"),
        ])
    }
}

final class TranscriptIdentityTests: XCTestCase {
    /// Item ids restart at 0 in every chat, so views key a chat on its identity.
    func testEachChatHasItsOwnIdentity() {
        var first = Transcript()
        first.appendUser("hi")
        var second = Transcript()
        second.appendUser("hello")
        XCTAssertEqual(first.items.first?.id, second.items.first?.id)
        XCTAssertNotEqual(first.identity, second.identity)
        let before = first.identity
        first.appendUser("more")
        XCTAssertEqual(first.identity, before, "a growing chat keeps its identity")
        first.removeAll()
        XCTAssertNotEqual(first.identity, before, "a cleared chat is a new one")
    }
}
