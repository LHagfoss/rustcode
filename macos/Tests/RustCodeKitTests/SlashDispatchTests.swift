import XCTest
@testable import RustCodeKit

/// #2164: which commands run at once while a chat's turn is running, and
/// which queue behind it like a message.
final class SlashDispatchTests: XCTestCase {
    private let commands: [SlashCommand] = {
        guard case .availableCommands(let list) = SessionUpdate(notificationParams: ["update": [
            "sessionUpdate": "available_commands_update",
            "availableCommands": [
                ["name": "status", "description": "", "_meta": ["rustcode": ["action": "execute", "report": true]]],
                ["name": "context", "description": "", "input": ["hint": "tokens"],
                 "_meta": ["rustcode": ["action": "execute", "report": true]]],
                ["name": "compact", "description": "", "_meta": ["rustcode": ["action": "execute"]]],
                ["name": "cancel", "description": "", "_meta": ["rustcode": ["action": "execute"]]],
                ["name": "new", "description": "", "_meta": ["rustcode": ["action": "new_session"]]],
                ["name": "remote", "description": "", "_meta": ["rustcode": ["action": "remote"]]],
                // An app action never counts as a report, whatever it says.
                ["name": "model", "description": "", "_meta": ["rustcode": ["action": "model_picker", "report": true]]],
            ],
        ]]) else { return [] }
        return SlashCommand.withAppCommands(list)
    }()

    private func dispatch(_ typed: String, plain: Bool = true) -> String {
        switch SlashCommand.dispatch(typed, in: commands, plain: plain) {
        case .app(let command): "app \(command.name)"
        case .cancel: "cancel"
        case .report(let command): "report \(command.name)"
        case .prompt: "prompt"
        }
    }

    func testEngineAdvertisesWhichHostCommandsAreReports() {
        XCTAssertEqual(commands.filter(\.report).map(\.name), ["/status", "/context"])
    }

    func testAppActionsAndCancelRunAtOnce() {
        XCTAssertEqual(dispatch("/new"), "app /new")
        XCTAssertEqual(dispatch("/model gpt"), "app /model")
        XCTAssertEqual(dispatch("/remote"), "app /remote")
        XCTAssertEqual(dispatch("/terminal"), "app /terminal")
        XCTAssertEqual(dispatch(" /CANCEL "), "cancel")
    }

    func testBareReportsRunBesideTheTurn() {
        XCTAssertEqual(dispatch("/status"), "report /status")
        XCTAssertEqual(dispatch("/Context"), "report /context")
    }

    func testSessionChangingCommandsAndPromptsQueue() {
        XCTAssertEqual(dispatch("/compact"), "prompt")
        // Arguments make /context change the session.
        XCTAssertEqual(dispatch("/context 256k"), "prompt")
        XCTAssertEqual(dispatch("/cancel now"), "prompt")
        XCTAssertEqual(dispatch("explain /status"), "prompt")
        XCTAssertEqual(dispatch("fix the bug"), "prompt")
        // Attached files make it a message.
        XCTAssertEqual(dispatch("/status", plain: false), "prompt")
    }

    func testReportAnswersAreTaggedApartFromTurnText() {
        let report: JSONValue = ["sessionId": "s", "update": [
            "sessionUpdate": "agent_message_chunk", "content": ["type": "text", "text": "**Status**"],
            "_meta": ["rustcode": ["report": true]],
        ]]
        let turn: JSONValue = ["sessionId": "s", "update": [
            "sessionUpdate": "agent_message_chunk", "content": ["type": "text", "text": "working"],
        ]]
        XCTAssertTrue(SessionUpdate.isReport(notificationParams: report))
        XCTAssertFalse(SessionUpdate.isReport(notificationParams: turn))
        guard case .agentText(let text) = SessionUpdate(notificationParams: report) else { return XCTFail("text") }
        XCTAssertEqual(text, "**Status**")
    }

    func testReportAnswersStayWithTheChatTheyWereTypedIn() {
        var reports = PendingReports()
        let first = reports.begin("/status", in: "a")
        let second = reports.begin("/usage", in: "a")
        let other = reports.begin("/tasks", in: "b")
        // The user switched to b: b's answer is b's, a's stay a's, in order.
        XCTAssertEqual(reports.answer(in: "b"), "/tasks")
        XCTAssertEqual(reports.answer(in: "a"), "/status")
        XCTAssertEqual(reports.answer(in: "a"), "/usage")
        XCTAssertNil(reports.answer(in: "a"), "a stray answer is not claimed")
        XCTAssertEqual(reports.finish(first, in: "a")?.answered, true)
        XCTAssertNil(reports.finish(other, in: "a"), "ids belong to their chat")
        XCTAssertEqual(reports.finish(other, in: "b")?.input, "/tasks")
        XCTAssertTrue(reports.finish(second, in: "a")!.answered)
        XCTAssertFalse(reports.isWaiting(in: "a"))
    }
}
