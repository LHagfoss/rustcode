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

    func testFindMatchesMessagesAndToolOutputIgnoringCase() {
        var transcript = Transcript()
        transcript.appendUser("Fix the Parser")
        transcript.apply(.toolCall(id: "t", title: "grep", kind: nil, status: .completed, input: nil))
        transcript.apply(.toolCallUpdate(id: "t", title: nil, status: nil, output: "src/parser.rs"))
        transcript.apply(.agentText("Done."))
        XCTAssertEqual(transcript.matches("parser"), [0, 1])
        XCTAssertEqual(transcript.matches("  "), [])
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
