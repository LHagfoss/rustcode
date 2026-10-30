import XCTest
@testable import RustCodeKit

/// #2211: new chats start in the default project; one chat can work in
/// another folder.
final class ProjectFolderTests: XCTestCase {
    private let home = URL(fileURLWithPath: "/Users/me/code/app", isDirectory: true)
    private let other = URL(fileURLWithPath: "/Users/me/code/site", isDirectory: true)

    func testAnOpenChatWorksInItsOwnFolder() {
        XCTAssertEqual(ProjectFolder.forChat(sessionCwd: other.path, draft: nil, default: home).path, other.path)
        XCTAssertEqual(ProjectFolder.forChat(sessionCwd: other.path, draft: home, default: home).path, other.path)
    }

    func testADraftUsesItsPickedFolderElseTheDefault() {
        XCTAssertEqual(ProjectFolder.forChat(sessionCwd: nil, draft: other, default: home), other)
        XCTAssertEqual(ProjectFolder.forChat(sessionCwd: nil, draft: nil, default: home), home)
        // A chat listed without a folder falls back the same way.
        XCTAssertEqual(ProjectFolder.forChat(sessionCwd: "", draft: nil, default: home), home)
    }

    func testPickingTheDefaultIsNoOverride() {
        XCTAssertNil(ProjectFolder.override(nil, default: home))
        XCTAssertNil(ProjectFolder.override(home, default: home))
        XCTAssertNil(ProjectFolder.override(URL(fileURLWithPath: "/Users/me/code/app/"), default: home))
        XCTAssertNil(ProjectFolder.override(URL(fileURLWithPath: "/Users/me/code/x/../app"), default: home))
        XCTAssertEqual(ProjectFolder.override(other, default: home), other)
    }

    /// #2306: the tooltip tells this chat's folder from the default.
    func testHelpNamesTheChatFolderAndTheDefault() {
        XCTAssertEqual(ProjectFolder.help(chat: home, default: home), "This thread's folder: /Users/me/code/app, the default project")
        XCTAssertEqual(ProjectFolder.help(chat: other, default: home),
                       "This thread's folder: /Users/me/code/site\nDefault project for new threads: /Users/me/code/app")
    }

    /// #2306: a missing or unreadable folder is refused before anything changes.
    func testOnlyAReadableFolderIsUsable() throws {
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent("rustcode-folder-\(UUID().uuidString)")
        XCTAssertFalse(ProjectFolder.isUsable(folder))
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: folder) }
        XCTAssertTrue(ProjectFolder.isUsable(folder))
        let file = folder.appendingPathComponent("file")
        try Data().write(to: file)
        XCTAssertFalse(ProjectFolder.isUsable(file), "a file is no folder")
        let locked = folder.appendingPathComponent("locked", isDirectory: true)
        try FileManager.default.createDirectory(at: locked, withIntermediateDirectories: true)
        try FileManager.default.setAttributes([.posixPermissions: 0o000], ofItemAtPath: locked.path)
        defer { try? FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: locked.path) }
        XCTAssertFalse(ProjectFolder.isUsable(locked))
    }
}
