import XCTest
@testable import RustCodeKit

final class FolderSuggestionTests: XCTestCase {
    private func call(_ id: Int, cwd: String?) -> TranscriptItem {
        let input = cwd.map { #"{"command": "gh issue list", "cwd": "\#($0)"}"# } ?? #"{"command": "ls"}"#
        return TranscriptItem(id: id, content: .tool(ToolItem(callId: "c\(id)", title: "run_command", input: input)))
    }

    private let folder = URL(fileURLWithPath: "/Users/me/code/discord_bot")

    func testMostCallsElsewhereSuggestThatFolder() {
        let items = (0..<4).map { call($0, cwd: "/Users/me/code/rustcode") } + [call(9, cwd: nil)]
        XCTAssertEqual(FolderSuggestion.suggest(items: items, folder: folder)?.path, "/Users/me/code/rustcode")
    }

    func testAQuickLookElsewhereOrCallsInsideSuggestNothing() {
        let few = (0..<2).map { call($0, cwd: "/Users/me/code/rustcode") }
        XCTAssertNil(FolderSuggestion.suggest(items: few, folder: folder))
        let mixed = (0..<3).map { call($0, cwd: "/Users/me/code/rustcode") } + (3..<7).map { call($0, cwd: nil) }
        XCTAssertNil(FolderSuggestion.suggest(items: mixed, folder: folder), "not most calls")
        let inside = (0..<4).map { call($0, cwd: "/Users/me/code/discord_bot/src") }
        XCTAssertNil(FolderSuggestion.suggest(items: inside, folder: folder))
    }

    func testSubfoldersOfOneProjectCountTogether() {
        let items = [call(0, cwd: "/r/app/macos"), call(1, cwd: "/r/app/ios"), call(2, cwd: "/r/app")]
        let suggestion = FolderSuggestion.suggest(items: items, folder: folder) { url in
            url.path.hasPrefix("/r/app") ? URL(fileURLWithPath: "/r/app") : url
        }
        XCTAssertEqual(suggestion?.path, "/r/app")
    }

    func testProjectRootIsTheFolderWithGit() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let nested = root.appendingPathComponent("a/b")
        try FileManager.default.createDirectory(at: nested, withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: root.appendingPathComponent(".git"), withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        XCTAssertEqual(FolderSuggestion.projectRoot(of: nested).path, root.standardizedFileURL.path)
    }
}
