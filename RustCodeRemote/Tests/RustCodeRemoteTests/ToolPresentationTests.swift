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
}
