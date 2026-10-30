import XCTest
@testable import RustCode

final class SettingsSearchTests: XCTestCase {
    func testAnEmptyQueryListsEverySection() {
        XCTAssertEqual(SettingsSection.matching(""), SettingsSection.allCases)
        XCTAssertEqual(SettingsSection.matching("   "), SettingsSection.allCases)
    }

    func testAQueryMatchesNamesAndKeywordsIgnoringCase() {
        XCTAssertEqual(SettingsSection.matching("MCP"), [.mcp])
        XCTAssertEqual(SettingsSection.matching("shortcut"), [.keybinds])
        XCTAssertEqual(SettingsSection.matching("dark"), [.general])
        XCTAssertEqual(SettingsSection.matching("smart ui"), [.general])
    }

    func testEveryWordMustMatchAndOrderFollowsTheSidebar() {
        XCTAssertEqual(SettingsSection.matching("version"), [.engine, .about])
        XCTAssertEqual(SettingsSection.matching("version updates"), [.about])
        XCTAssertEqual(SettingsSection.matching("zzz"), [])
    }
}
