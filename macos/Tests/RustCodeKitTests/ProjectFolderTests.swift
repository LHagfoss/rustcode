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
}
