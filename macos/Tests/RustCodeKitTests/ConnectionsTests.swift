import Foundation
import XCTest
@testable import RustCodeKit

final class ConnectionsTests: XCTestCase {
    /// The shape `engine/src/connections.rs` serializes
    /// (`the_snapshot_wire_shape_matches_the_mac_app`).
    private let wire: JSONValue = [
        "github": [
            "cliInstalled": true,
            "accounts": [
                ["host": "github.com", "login": "amy", "active": true, "valid": true, "scopes": ["gist", "repo"]],
                ["host": "github.com", "login": "zed", "active": false, "valid": false, "scopes": []],
            ],
            "pending": ["code": "37B2-1C7B", "url": "https://github.com/login/device"],
            "error": nil,
        ],
        "accounts": [
            ["provider": "openai", "account": "me@example.com", "display": "Me", "method": "chat_gpt",
             "active": true, "expiresAt": 1_900_000_000, "needsSignIn": true],
            ["provider": "anthropic", "account": "default", "display": "", "method": "api_key",
             "active": false, "expiresAt": nil],
        ],
    ]

    func testSnapshotDecodesGitHubAndModelAccounts() throws {
        let snapshot = try ConnectionsSnapshot(json: wire)
        XCTAssertTrue(snapshot.github.cliInstalled)
        XCTAssertEqual(snapshot.github.active?.login, "amy")
        XCTAssertEqual(snapshot.github.active?.missingScopes, ["read:org"])
        XCTAssertEqual(snapshot.github.pending, .init(code: "37B2-1C7B", url: "https://github.com/login/device"))
        XCTAssertEqual(snapshot.accounts.map(\.methodLabel), ["ChatGPT sign-in", "API key"])
        XCTAssertEqual(snapshot.accounts.map(\.id), ["openai/me@example.com", "anthropic/default"])
        // An older engine sends no flag (#2765).
        XCTAssertEqual(snapshot.accounts.map(\.needsSignIn), [true, nil])
    }

    func testNoActiveAccountWhenSignedOut() throws {
        let snapshot = try ConnectionsSnapshot(json: ["github": ["cliInstalled": false, "accounts": [], "pending": nil, "error": nil],
                                                      "accounts": []])
        XCTAssertNil(snapshot.github.active)
        XCTAssertFalse(snapshot.github.cliInstalled)
    }

    /// Refresh names the account the way sign-out does, on the engine's method.
    func testRefreshNamesItsAccount() {
        XCTAssertEqual(ACPMethod.accountRefresh, "_rustcode/connections/account/refresh")
        let params = ACPRequests.accountRefresh(provider: "openai", account: "me@example.com")
        XCTAssertEqual(params["provider"], "openai")
        XCTAssertEqual(params["account"], "me@example.com")
    }
}
