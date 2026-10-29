import XCTest
@testable import RustCodeKit

final class OpenSessionsTests: XCTestCase {
    private let models = ModelOptions(current: "gpt", choices: [])

    private func transcript(_ texts: String...) -> Transcript {
        var transcript = Transcript()
        for text in texts { transcript.apply(.userText(text)) }
        return transcript
    }

    private func texts(_ transcript: Transcript?) -> [String] {
        (transcript?.items ?? []).compactMap { if case .user(let text) = $0.content { text } else { nil } }
    }

    func testAnUpdateToALoadingChatGoesIntoItsReplayShownOrNot() {
        var chats = OpenSessions()
        chats.beginLoad("a")
        XCTAssertEqual(chats.route(.userText("one"), in: "a", selected: "a"), .replay)
        XCTAssertEqual(chats.route(.userText("two"), in: "a", selected: "b"), .replay)
        XCTAssertEqual(texts(chats.replay("a")), ["one", "two"])
        chats.finishLoad("a", models: models)
        chats.endLoad("a")
        XCTAssertFalse(chats.isLoading("a"))
        XCTAssertNil(chats.replay("a"))
        XCTAssertEqual(texts(chats.snapshot("a")?.transcript), ["one", "two"])
        XCTAssertEqual(chats.snapshot("a")?.selectedModel, "gpt")
    }

    func testReportsNoticesAndStopsSkipTheReplay() {
        var chats = OpenSessions()
        chats.beginLoad("a")
        XCTAssertEqual(chats.route(.userText("report"), isReport: true, in: "a", selected: "a"), .shown)
        XCTAssertEqual(chats.route(.notice("slow"), in: "a", selected: "a"), .shown)
        XCTAssertEqual(chats.route(.stopped("cut"), in: "a", selected: "b"), .ignored)
        XCTAssertEqual(texts(chats.replay("a")), [])
    }

    func testAnUpdateToABackgroundChatKeepsItCurrent() {
        var chats = OpenSessions()
        chats.stash("a", transcript: transcript("one"), models: models, selectedModel: "gpt")
        XCTAssertEqual(chats.route(.userText("two"), in: "a", selected: "b"), .background)
        XCTAssertEqual(texts(chats.snapshot("a")?.transcript), ["one", "two"])
        // A notice or a stopped turn is shown as a toast, never in the chat.
        XCTAssertEqual(chats.route(.notice("slow"), in: "a", selected: "b"), .background)
        XCTAssertEqual(chats.snapshot("a")?.transcript.items.count, 2)
        XCTAssertEqual(chats.route(.userText("x"), in: "z", selected: "b"), .ignored)
    }

    func testAnUpdateToTheShownChatIsLeftToTheCaller() {
        var chats = OpenSessions()
        chats.stash("a", transcript: transcript("one"), models: models, selectedModel: nil)
        XCTAssertEqual(chats.route(.userText("two"), in: "a", selected: "a"), .shown)
        XCTAssertEqual(texts(chats.snapshot("a")?.transcript), ["one"])
    }

    func testLoadingTheSameChatTwiceStartsItsReplayOver() {
        var chats = OpenSessions()
        chats.beginLoad("a")
        _ = chats.route(.userText("stale"), in: "a", selected: "a")
        // The engine was replaced mid-load: the new one loads it again.
        chats.reset()
        chats.beginLoad("a")
        _ = chats.route(.userText("fresh"), in: "a", selected: "a")
        chats.finishLoad("a", models: models)
        XCTAssertEqual(texts(chats.snapshot("a")?.transcript), ["fresh"])
    }

    func testSwitchingAwayMidLoadKeepsTheReplay() {
        var chats = OpenSessions()
        chats.beginLoad("a")
        _ = chats.route(.userText("one"), in: "a", selected: "a")
        // Picking another chat stashes the empty screen, which must not
        // replace the replay or be remembered as the chat.
        chats.stash("a", transcript: Transcript(), models: .empty, selectedModel: nil)
        XCTAssertFalse(chats.isOpen("a"))
        _ = chats.route(.userText("two"), in: "a", selected: "b")
        chats.finishLoad("a", models: models)
        chats.endLoad("a")
        XCTAssertEqual(texts(chats.snapshot("a")?.transcript), ["one", "two"])
        // Once in, leaving it stashes what was shown.
        chats.stash("a", transcript: transcript("one", "two", "three"), models: models, selectedModel: "gpt")
        XCTAssertEqual(texts(chats.snapshot("a")?.transcript), ["one", "two", "three"])
    }

    func testAFailedLoadLeavesTheChatClosed() {
        var chats = OpenSessions()
        chats.beginLoad("a")
        chats.endLoad("a")
        XCTAssertFalse(chats.isOpen("a"))
        XCTAssertFalse(chats.isLoading("a"))
        XCTAssertEqual(chats.route(.userText("late"), in: "a", selected: "b"), .ignored)
    }

    func testForgetOnTrashOrMoveMakesTheNextPickLoadAgain() {
        var chats = OpenSessions()
        chats.stash("a", transcript: transcript("one"), models: models, selectedModel: nil)
        chats.stash("b", transcript: transcript("two"), models: models, selectedModel: nil)
        chats.forget("a")
        XCTAssertNil(chats.snapshot("a"))
        XCTAssertEqual(Set(chats.openIds), ["b"])
        XCTAssertEqual(chats.route(.userText("late"), in: "a", selected: "b"), .ignored)
        XCTAssertEqual(chats.route(.userText("late"), in: "a", selected: "a"), .shown)
    }

    func testUpdateTranscriptOnlyTouchesOpenChats() {
        var chats = OpenSessions()
        XCTAssertFalse(chats.updateTranscript(of: "a") { $0.apply(.userText("x")) })
        chats.stash("a", transcript: Transcript(), models: models, selectedModel: nil)
        XCTAssertTrue(chats.updateTranscript(of: "a") { $0.apply(.userText("x")) })
        XCTAssertEqual(texts(chats.snapshot("a")?.transcript), ["x"])
    }

    /// #2595: a background chat's settings follow `/fast` run there and a
    /// setting's reply that lands after another chat was picked.
    func testABackgroundChatKeepsItsOwnModelOptions() {
        var chats = OpenSessions()
        chats.stash("a", transcript: Transcript(), models: models, selectedModel: "gpt")
        let fastOn: JSONValue = [["id": "model", "type": "select", "currentValue": "plan", "options": [["value": "plan"]]],
                                 ["id": "fast", "type": "boolean", "currentValue": true]]
        XCTAssertEqual(chats.route(.configOptions(fastOn), in: "a", selected: "b"), .background)
        XCTAssertEqual(chats.snapshot("a")?.models.fast, true)
        XCTAssertEqual(chats.snapshot("a")?.selectedModel, "plan")
        XCTAssertTrue(chats.snapshot("a")?.transcript.items.isEmpty ?? false)
        var off = ModelOptions(configOptions: fastOn)
        off.fast = false
        XCTAssertTrue(chats.updateModels(of: "a", off))
        XCTAssertEqual(chats.snapshot("a")?.models.fast, false)
        XCTAssertFalse(chats.updateModels(of: "closed", off), "a chat not open is left alone")
    }
}
