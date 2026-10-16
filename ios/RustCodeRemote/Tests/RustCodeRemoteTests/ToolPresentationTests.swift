import Testing
@testable import RustCodeRemote

@Suite struct ToolPresentationTests {
    @Test func knownToolsUseTheVerbTheTerminalShows() {
        #expect(ToolPresentation.title("view_file") == "Read")
        #expect(ToolPresentation.title("replace_file_content") == "Edit")
        #expect(ToolPresentation.title("write_to_file") == "Write")
        #expect(ToolPresentation.title("run_command") == "Run")
        #expect(ToolPresentation.title("grep") == "Search")
        #expect(ToolPresentation.title("search_web") == "Web search")
    }

    @Test func mcpToolsShowServerAndTool() {
        #expect(ToolPresentation.title("mcp__github__create_issue") == "github · create_issue")
        // Not the host's MCP shape: fall through to the generic rule.
        #expect(ToolPresentation.title("mcp__broken") == "Mcp broken")
    }

    @Test func unknownToolsStayRecognisable() {
        #expect(ToolPresentation.title("openapi_call") == "Openapi call")
        #expect(ToolPresentation.title("explorer") == "Explorer")
        #expect(ToolPresentation.title("") == "")
    }

    @Test func descriptionPrefersDetailThenWhatTheRowAlreadyHas() {
        #expect(ToolPresentation.description(for: ToolCall(name: "view_file", summary: "src/a.rs", status: .running)) == "src/a.rs")
        #expect(ToolPresentation.description(for: ToolCall(name: "run_command", summary: "\n  cargo test\nmore", status: .running)) == "cargo test")
        #expect(ToolPresentation.description(for: ToolCall(name: "run_command", summary: "", output: "\nok: 3 passed\n", status: .succeeded)) == "ok: 3 passed")
        let long = String(repeating: "x", count: 300)
        #expect(ToolPresentation.description(for: ToolCall(name: "grep", summary: " ", output: long, status: .succeeded))?.count == 120)
        #expect(ToolPresentation.description(for: ToolCall(name: "mcp__github__create_issue", summary: "", status: .running)) == "MCP tool")
        #expect(ToolPresentation.description(for: ToolCall(name: "view_file", summary: "", status: .running)) == nil)
    }
}
