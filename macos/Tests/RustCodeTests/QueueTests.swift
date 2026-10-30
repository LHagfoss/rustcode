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
    private func entries(pinned: Set<String>, lifecycle: SidebarLifecycle = SidebarLifecycle(),
                         open: Set<SidebarGroups.Kind> = []) -> [String] {
        let sessions = ["a", "b", "c"].map { SessionSummary(id: $0, cwd: "/p") }
        let groups = SidebarGroups(ids: sessions.map(\.id), pinned: pinned, lifecycle: lifecycle, now: Date(),
                                   byAttention: true) { _ in .idle }
        return SidebarEntry.entries(groups, sessions: Dictionary(uniqueKeysWithValues: sessions.map { ($0.id, $0) }),
                                    openShelves: open).map(\.id)
    }

    /// One list keyed by chat, so pinning moves a row instead of making a new one.
    func testPinnedChatsComeFirstUnderTheirHeader() {
        XCTAssertEqual(entries(pinned: []), ["a", "b", "c"])
        let ids = entries(pinned: ["b"])
        XCTAssertEqual(ids.filter { !$0.hasPrefix("\u{0}") }, ["b", "a", "c"])
        XCTAssertEqual(Set(ids).count, ids.count)
    }

    /// A folded shelf shows only its heading; unfolded, its chats follow it.
    func testAFoldedShelfHidesItsChats() {
        var lifecycle = SidebarLifecycle()
        lifecycle.markDone("c", at: Date())
        XCTAssertEqual(entries(pinned: [], lifecycle: lifecycle).filter { !$0.hasPrefix("\u{0}") }, ["a", "b"])
        XCTAssertEqual(entries(pinned: [], lifecycle: lifecycle, open: [.done]).last, "c")
    }
}
