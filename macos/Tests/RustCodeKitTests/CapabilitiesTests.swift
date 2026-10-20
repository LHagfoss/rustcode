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
}
