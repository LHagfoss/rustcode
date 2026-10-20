import XCTest
import RustCodeKit
@testable import RustCode

@MainActor
final class QueueTests: XCTestCase {
    private typealias Prompt = DesktopModel.QueuedPrompt

    func testMessagesQueuedWhileAChatStartsMoveToIt() {
        let starting = Prompt(sessionId: DesktopModel.newChatQueue, typed: "second", text: "second", images: [])
        let other = Prompt(sessionId: "other", typed: "x", text: "x", images: [])
        let adopted = DesktopModel.adoptQueued([starting, other], into: "new")
        XCTAssertEqual(adopted.map(\.sessionId), ["new", "other"])
        // Same entry, so it can still be edited or removed by id.
        XCTAssertEqual(adopted[0].id, starting.id)
        XCTAssertEqual(adopted[0].typed, "second")
    }
}

final class SidebarEntryTests: XCTestCase {
    /// One list keyed by chat, so pinning moves a row instead of making a new one.
    func testPinnedChatsComeFirstUnderTheirHeader() {
        let a = SessionSummary(id: "a", cwd: "/p"), b = SessionSummary(id: "b", cwd: "/p")
        XCTAssertEqual(SidebarEntry.entries(pinned: [], unpinned: [a, b]).map(\.id), ["a", "b"])
        let ids = SidebarEntry.entries(pinned: [b], unpinned: [a]).map(\.id)
        XCTAssertEqual(ids.filter { !$0.hasPrefix("\u{0}") }, ["b", "a"])
        XCTAssertEqual(Set(ids).count, ids.count)
    }
}
