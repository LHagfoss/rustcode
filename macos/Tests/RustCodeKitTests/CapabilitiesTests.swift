import Foundation
import XCTest
@testable import RustCodeKit

final class CapabilitiesTests: XCTestCase {
    /// The shape `engine/src/capabilities.rs` serializes
    /// (`the_snapshot_wire_shape_matches_the_mac_app`).
    private let wire: JSONValue = [
        "skills": [
            ["name": "deploy", "description": "Ship it", "path": "/p/.rustcode/skills/deploy",
             "source": "workspace", "enabled": true],
            ["name": "review", "description": "Review code", "path": "/h/.agents/skills/review",
             "source": "universal", "enabled": false],
        ],
        "mcpServers": [
            ["name": "github", "transport": "stdio", "target": "npx server --token •••", "enabled": true,
             "scope": "user", "connection": "connected", "tools": ["issues", "pulls"], "error": nil],
            ["name": "local", "transport": "http", "target": "https://mcp.example.com/mcp", "enabled": true,
             "scope": "project", "connection": "failed", "tools": [], "error": "timed out"],
            ["name": "off", "transport": "stdio", "target": "off", "enabled": false,
             "scope": "user", "connection": "disabled", "tools": [], "error": nil],
            ["name": "idle", "transport": "stdio", "target": "idle", "enabled": true,
             "scope": "user", "connection": "not_connected", "tools": [], "error": nil],
        ],
        "configPath": "/h/.config/rustcode/config.toml",
    ]

    func testSnapshotDecodesInstalledVersusEnabledAndConfiguredVersusConnected() throws {
        let snapshot = try CapabilitySnapshot(json: wire)
        XCTAssertEqual(snapshot.skills.map(\.enabled), [true, false])
        XCTAssertEqual(snapshot.skills[0].sourceLabel, "This project")
        XCTAssertEqual(snapshot.mcpServers.map(\.connection), [.connected, .failed, .disabled, .notConnected])
        XCTAssertEqual(snapshot.mcpServers[0].statusLabel, "Connected · 2 tools")
        XCTAssertTrue(snapshot.mcpServers[0].canToggle)
        XCTAssertFalse(snapshot.mcpServers[1].canToggle, "a project server is changed in its own file")
        XCTAssertEqual(snapshot.mcpServers[1].error, "timed out")
        XCTAssertEqual(snapshot.configPath, "/h/.config/rustcode/config.toml")
    }

    func testSearchMatchesNamesDescriptionsAndTools() throws {
        let snapshot = try CapabilitySnapshot(json: wire)
        XCTAssertEqual(snapshot.skills(matching: "CODE").map(\.name), ["review"])
        XCTAssertEqual(snapshot.skills(matching: "  ").count, 2)
        XCTAssertEqual(snapshot.mcpServers(matching: "pulls").map(\.name), ["github"])
        XCTAssertEqual(snapshot.mcpServers(matching: "example.com").map(\.name), ["local"])
    }

    func testSetRequestNamesTheKindAndState() {
        XCTAssertEqual(ACPRequests.setCapability(.mcp, name: "github", enabled: false),
                       ["kind": "mcp", "name": "github", "enabled": false])
        XCTAssertEqual(ACPMethod.setCapability, "_rustcode/capabilities/set")
    }

    func testCommandLineSplitsLikeAShell() throws {
        XCTAssertEqual(try MCPServerDraft.splitCommandLine("npx -y  @x/files-mcp").get(), ["npx", "-y", "@x/files-mcp"])
        XCTAssertEqual(try MCPServerDraft.splitCommandLine(#"zsh -c 'source "$HOME/.zshrc" && exec npx'"#).get(),
                       ["zsh", "-c", #"source "$HOME/.zshrc" && exec npx"#])
        XCTAssertEqual(try MCPServerDraft.splitCommandLine(#"a\ b "c d" ''"#).get(), ["a b", "c d", ""])
        XCTAssertThrowsError(try MCPServerDraft.splitCommandLine("echo 'open").get())
    }

    func testDraftBuildsTheAddRequestForEachTransport() throws {
        var draft = MCPServerDraft()
        XCTAssertEqual(draft.problem, "Enter a name.")
        draft.name = " files "
        XCTAssertEqual(draft.problem, "Enter the command that starts the server.")
        draft.command = "npx -y files-mcp"
        draft.environment = "API_KEY=1\n\n  DEBUG=0 "
        XCTAssertNil(draft.problem)
        XCTAssertEqual(try draft.request().get(), [
            "name": "files", "transport": "stdio", "target": ["npx", "-y", "files-mcp"], "env": ["API_KEY=1", "DEBUG=0"],
        ])

        draft.transport = .http
        draft.url = "mcp.example.com"
        XCTAssertEqual(draft.problem, "The URL must start with http:// or https://.")
        draft.url = "https://mcp.example.com/mcp"
        draft.headers = "Authorization: Bearer t"
        XCTAssertEqual(try draft.request().get(), [
            "name": "files", "transport": "http", "target": ["https://mcp.example.com/mcp"],
            "headers": ["Authorization: Bearer t"],
        ])
        draft.name = "my files"
        XCTAssertEqual(draft.problem, "A name cannot contain spaces.")
    }
}
