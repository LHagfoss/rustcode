import Testing
@testable import RustCodeRemote

@Suite struct AssistantBlockTests {
    @Test func separatesThinkingFromTheAnswerAndPreservesMultiplePhases() {
        #expect(AssistantBlock.parse("Before<think>Plan</think>Answer<think>Check</think>Done") == [
            .markdown("Before"), .thinking(text: "Plan", isComplete: true),
            .markdown("Answer"), .thinking(text: "Check", isComplete: true), .markdown("Done"),
        ])
    }

    @Test func holdsSplitTagsDuringStreaming() {
        let full = "<think>Plan</think>Answer"
        for end in full.indices {
            let prefix = String(full[..<end])
            let blocks = AssistantBlock.parse(prefix, isStreaming: true)
            // A partial delimiter must never flash as response text.
            for block in blocks {
                if case .markdown(let text) = block { #expect(!text.contains("<")) }
                if case .thinking(let text, _) = block { #expect(!text.contains("<")) }
            }
        }
        #expect(AssistantBlock.parse("<think>Plan</thi", isStreaming: true) == [
            .thinking(text: "Plan", isComplete: false),
        ])
    }

    @Test func unterminatedThinkingRemainsSeparateAfterTheTurnEnds() {
        #expect(AssistantBlock.parse("<think>Plan") == [.thinking(text: "Plan", isComplete: false)])
        #expect(AssistantBlock.parse("A partial <thi") == [.markdown("A partial <thi")])
    }

    @Test func dropsEmptyReasoningOnceItCanNoLongerFill() {
        #expect(AssistantBlock.parse("<think></think>Answer") == [.markdown("Answer")])
        #expect(AssistantBlock.parse("<think>\n  \n</think>Answer<think> </think>") == [.markdown("Answer")])
        #expect(AssistantBlock.parse("<think>") == [])
        // Streaming: an opened block with no text yet says it is thinking, but only while open.
        #expect(AssistantBlock.parse("<think>", isStreaming: true) == [.thinking(text: "", isComplete: false)])
        #expect(AssistantBlock.parse("<think></think>Ans", isStreaming: true) == [.markdown("Ans")])
        #expect(AssistantBlock.parse("<think>Plan</think>", isStreaming: true) == [.thinking(text: "Plan", isComplete: true)])
    }

    @Test func leavesLiteralTagsInsideCodeAlone() {
        for text in ["Use `<think>` and `</think>`.", "````xml\n<think>literal</think>\n```\n````", "~~~xml\n<think>literal</think>\n~~~", "Escaped \\<think>literal"] {
            #expect(AssistantBlock.parse(text) == [.markdown(text)])
        }
    }

    @Test func windowsCodeFencesKeepThinkingTagsLiteral() {
        #expect(AssistantBlock.parse("~~~xml\r\n<think>literal</think>\r\n~~~\r\n<think>Plan</think>Answer") == [
            .markdown("~~~xml\n<think>literal</think>\n~~~\n"),
            .thinking(text: "Plan", isComplete: true), .markdown("Answer"),
        ])
    }

    @Test func unmatchedBackticksDoNotSwallowTheAnswer() {
        #expect(AssistantBlock.parse("<think>Print the ` character.</think>Answer") == [
            .thinking(text: "Print the ` character.", isComplete: true), .markdown("Answer"),
        ])
    }

    @Test func streamingInlineCodeNeverFlashesAsThinking() {
        let text = "Use `<think>literal</think>`."
        for end in text.indices {
            let prefix = String(text[..<end])
            #expect(AssistantBlock.parse(prefix, isStreaming: true) == (prefix.isEmpty ? [] : [.markdown(prefix)]))
        }
    }

    @Test func delimitersStartNewMarkdownSegmentsForFences() {
        #expect(AssistantBlock.parse("<think>~~~xml\n<think>literal</think>\n~~~\n</think>Answer") == [
            .thinking(text: "~~~xml\n<think>literal</think>\n~~~\n", isComplete: true), .markdown("Answer"),
        ])
        #expect(AssistantBlock.parse("<think>Plan</think>~~~xml\n<think>literal</think>\n~~~") == [
            .thinking(text: "Plan", isComplete: true), .markdown("~~~xml\n<think>literal</think>\n~~~"),
        ])
    }
}
