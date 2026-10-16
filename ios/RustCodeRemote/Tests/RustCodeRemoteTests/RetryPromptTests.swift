import Testing
@testable import RustCodeRemote

@Suite struct RetryPromptTests {
    @Test func latestPromptIsTheMostRecentNonBlankUserMessage() {
        let items = [
            TranscriptItem(id: "1", .user("first")),
            TranscriptItem(id: "2", .assistant(AssistantText(text: "answer"))),
            TranscriptItem(id: "3", .user("  second\n")),
            TranscriptItem(id: "4", .tool(ToolCall(name: "run_command", summary: "pwd", status: .succeeded))),
            TranscriptItem(id: "5", .user("   ")),
            TranscriptItem(id: "6", .notice("note")),
        ]
        #expect(RetryPrompt.latest(in: items) == "second")
    }

    @Test func noPromptWhenTheTranscriptHasNone() {
        #expect(RetryPrompt.latest(in: []) == nil)
        #expect(RetryPrompt.latest(in: [TranscriptItem(id: "a", .assistant(AssistantText(text: "hi")))]) == nil)
    }

    @Test func retryKeepsImageReferencesSoAttachmentsGoAgain() {
        let raw = "Look at this\n\n![image](file:///tmp/remote-images/\(String(repeating: "a", count: 64))/img_1.png)"
        #expect(RetryPrompt.text(of: TranscriptItem(id: "u", .user(raw))) == raw)
        #expect(RetryPrompt.text(of: TranscriptItem(id: "a", .assistant(AssistantText(text: raw)))) == nil)
    }
}
