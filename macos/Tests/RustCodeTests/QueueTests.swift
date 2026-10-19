import XCTest
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
