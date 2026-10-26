import Foundation
import XCTest
@testable import RustCodeKit

final class ProtocolTests: XCTestCase {
    func testLineFramerHandlesSplitAndMultipleLines() throws {
        var framer = LineFramer(maxLineBytes: 32)
        XCTAssertEqual(try framer.append(Data("{\"a\":".utf8)), [])
        XCTAssertEqual(try framer.append(Data("1}\n{\"b\":2}\n".utf8)), [Data("{\"a\":1}".utf8), Data("{\"b\":2}".utf8)])
    }

    func testLineFramerRejectsOversizedUnterminatedLine() {
        var framer = LineFramer(maxLineBytes: 3)
        XCTAssertThrowsError(try framer.append(Data("1234".utf8)))
    }

    func testRPCRequestRoundTripsAndKeepsJSONRPCEnvelope() throws {
        let message = RPCMessage.request(id: .number(7), method: "initialize", params: ["protocolVersion": 1])
        let data = try message.encodedLine()
        XCTAssertEqual(data.last, 10)
        XCTAssertEqual(try RPCMessage.decode(Data(data.dropLast())), message)
    }

    func testAgentProtocolVersionMustMatchRatherThanTrustEcho() throws {
        XCTAssertNoThrow(try AgentInfo(initializeResult: ["protocolVersion": 1]))
        XCTAssertThrowsError(try AgentInfo(initializeResult: ["protocolVersion": 2]))
    }

    func testQuestionIsDistinguishedFromToolApprovalAndIncludesCancel() throws {
        let request = PermissionRequest(rpcId: .number(12), params: [
            "sessionId": "s1",
            "toolCall": ["toolCallId": "question:q1", "title": "Plan: Choose", "rawInput": [
                "header": "Plan", "question": "Choose", "options": ["A", "B"], "multiple": false,
            ]],
            "options": [
                ["optionId": "question_option_0", "name": "A", "kind": "allow_once"],
                ["optionId": "question_cancel", "name": "Cancel", "kind": "reject_once"],
            ],
            "_meta": ["rustcode": ["kind": "question", "version": 2]],
        ])
        XCTAssertEqual(request?.question?.text, "Choose")
        XCTAssertEqual(request?.answerOptions.map(\.id), ["question_option_0"])
        XCTAssertEqual(request?.rejectOption?.id, "question_cancel")
    }

    func testCommandResultSplitsTitleAndPicksToastOrSheet() {
        let brief = CommandResult(input: "/compact", output: "**Compacted**\n\nSaved 12k tokens.")
        XCTAssertEqual(brief.title, "Compacted")
        XCTAssertEqual(brief.content, "Saved 12k tokens.")
        XCTAssertTrue(brief.isBrief)
        let long = CommandResult(input: "/status", output: "Model: a\nEffort: high")
        XCTAssertEqual(long.title, "/status")
        XCTAssertFalse(long.isBrief)
        XCTAssertEqual(CommandResult(input: "/x", output: "  ").content, "")
    }

    func testAutoApproveAllowsToolsButNeverAnswersQuestions() {
        let tool = PermissionRequest(rpcId: .number(3), params: [
            "sessionId": "s1",
            "toolCall": ["toolCallId": "c1", "title": "Run cargo test"],
            "options": [
                ["optionId": "reject", "name": "Reject", "kind": "reject_once"],
                ["optionId": "always", "name": "Always", "kind": "allow_always"],
                ["optionId": "once", "name": "Allow", "kind": "allow_once"],
            ],
        ])
        XCTAssertEqual(tool?.autoApproveOption?.id, "once")
        let question = PermissionRequest(rpcId: .number(4), params: [
            "sessionId": "s1",
            "toolCall": ["toolCallId": "question:q", "title": "Choose", "rawInput": ["question": "Choose"]],
            "options": [["optionId": "question_option_0", "name": "A", "kind": "allow_once"]],
            "_meta": ["rustcode": ["kind": "question", "version": 2]],
        ])
        XCTAssertNil(question?.autoApproveOption)
    }

    func testToolUpdatesCarryEngineDetailAndLiveOutput() {
        let started = SessionUpdate(notificationParams: ["update": [
            "sessionUpdate": "tool_call", "toolCallId": "c1", "title": "run_command", "status": "in_progress",
            "_meta": ["rustcode": ["detail": "cargo test", "summary": "Running the tests"]],
        ]])
        guard case .toolCall(_, _, _, _, _, let detail, let summary) = started else { return XCTFail("expected a tool call") }
        XCTAssertEqual(detail, "cargo test")
        XCTAssertEqual(summary, "Running the tests")
        let output = SessionUpdate(notificationParams: ["update": [
            "sessionUpdate": "tool_call_update", "toolCallId": "c1",
            "_meta": ["rustcode": ["outputDelta": "running 3 tests\n"]],
        ]])
        guard case .toolCallUpdate(_, _, _, let full, _, let delta) = output else { return XCTFail("expected an update") }
        XCTAssertNil(full)
        XCTAssertEqual(delta, "running 3 tests\n")
    }

    func testQuestionAnswersCarrySeveralOptionsOrTextInRustcodeMeta() {
        let a = PermissionOption(id: "question_option_0", name: "A", kind: .allowOnce)
        let b = PermissionOption(id: "question_option_1", name: "B", kind: .allowOnce)
        let several = ACPRequests.questionOptions([a, b])
        XCTAssertEqual(several["outcome"]?["optionId"]?.stringValue, "question_option_0")
        XCTAssertEqual(several["_meta"]?["rustcode"]?["answer"]?["options"]?.arrayValue?.compactMap(\.stringValue), ["A", "B"])
        let text = ACPRequests.questionText("Neither", carrier: a)
        XCTAssertEqual(text["outcome"]?["outcome"]?.stringValue, "selected")
        XCTAssertEqual(text["_meta"]?["rustcode"]?["answer"]?["text"]?.stringValue, "Neither")
    }

    func testSessionListCoversEveryProjectAndPage() {
        XCTAssertEqual(ACPRequests.listSessions(cwd: nil), [:])
        XCTAssertEqual(ACPRequests.listSessions(cwd: URL(fileURLWithPath: "/tmp/p"), cursor: "v1:abc"),
                       ["cwd": "/tmp/p", "cursor": "v1:abc"])
        let page: JSONValue = ["sessions": [["sessionId": "s1", "cwd": "/Users/me/code/app"]], "nextCursor": "v1:s1"]
        XCTAssertEqual(SessionSummary.nextCursor(from: page), "v1:s1")
        XCTAssertEqual(SessionSummary.list(from: page).first?.projectName, "app")
    }

    func testMergedSessionListHasEachChatOnce() {
        let listed = [SessionSummary(id: "a", cwd: "/p"), SessionSummary(id: "b", cwd: "/p"), SessionSummary(id: "a", cwd: "/p")]
        let started = [SessionSummary.started(id: "new", cwd: "/p", prompt: "hi", model: nil),
                       SessionSummary.started(id: "b", cwd: "/p", prompt: "b", model: nil)]
        XCTAssertEqual(SessionSummary.merging(listed, started: started).map(\.id), ["new", "a", "b"])
    }

    /// #2254: a chat used since it started moves above newer, idle ones.
    func testSessionListPutsLatestActivityFirst() {
        let now = Date()
        let listed = [SessionSummary(id: "new", cwd: "/p", updatedAt: now.addingTimeInterval(-60)),
                      SessionSummary(id: "undated", cwd: "/p"),
                      SessionSummary(id: "old", cwd: "/p", updatedAt: now),
                      SessionSummary(id: "older", cwd: "/p", updatedAt: now.addingTimeInterval(-60))]
        XCTAssertEqual(SessionSummary.merging(listed, started: []).map(\.id), ["old", "new", "older", "undated"])
    }

    /// #2418: a chat whose turn started here stays on top though the engine's
    /// list, read mid-turn, still dates it earlier; a later listing wins.
    func testLocalActivityOutranksAStaleListing() {
        let now = Date()
        let listed = [SessionSummary(id: "idle", cwd: "/p", updatedAt: now),
                      SessionSummary(id: "running", cwd: "/p", updatedAt: now.addingTimeInterval(-600))]
        let used = ["running": now.addingTimeInterval(10)]
        XCTAssertEqual(SessionSummary.touched(listed, used).map(\.id), ["running", "idle"])
        XCTAssertEqual(SessionSummary.touched(listed, [:]).map(\.id), ["idle", "running"])
        let newer = [SessionSummary(id: "idle", cwd: "/p", updatedAt: now.addingTimeInterval(20)),
                     SessionSummary(id: "running", cwd: "/p", updatedAt: now.addingTimeInterval(-600))]
        XCTAssertEqual(SessionSummary.touched(newer, used).map(\.id), ["idle", "running"])
    }

    func testAvailableCommandsCarryActionAndArguments() {
        let update = SessionUpdate(notificationParams: ["sessionId": "s", "update": [
            "sessionUpdate": "available_commands_update",
            "availableCommands": [
                ["name": "status", "description": "Session status"],
                ["name": "model", "description": "Choose a model", "input": ["hint": "arguments"],
                 "_meta": ["rustcode": ["action": "model_picker"]]],
            ],
        ]])
        guard case .availableCommands(let commands) = update else { return XCTFail("expected commands") }
        XCTAssertEqual(commands.map(\.name), ["/status", "/model"])
        XCTAssertEqual(commands[1].action, .modelPicker)
        XCTAssertEqual(commands[1].insertion, "/model ")
        XCTAssertEqual(SlashCommand.suggestions(for: "/st", in: commands).map(\.name), ["/status"])
        XCTAssertEqual(SlashCommand.suggestions(for: "/status now", in: commands), [])
        XCTAssertEqual(SlashCommand.invoked(by: "/model gpt", in: commands)?.action, .modelPicker)
        XCTAssertNil(SlashCommand.invoked(by: "explain /model", in: commands))
    }

    func testTerminalIsAnAppCommandListedWithTheEngines() {
        let engine = [SlashCommand(name: "/status", description: "Session status")]
        let listed = SlashCommand.withAppCommands(engine)
        XCTAssertEqual(listed.map(\.name), ["/status", "/terminal", "/remote", "/visualize"])
        XCTAssertEqual(SlashCommand.suggestions(for: "/term", in: listed).map(\.action), [.terminal])
        XCTAssertEqual(SlashCommand.withAppCommands([]).map(\.name), ["/terminal", "/remote", "/visualize"],
                       "listed with no engine")
        // An engine command of the same name is not shadowed; the other app
        // commands are still listed beside it.
        let clash = [SlashCommand(name: "/terminal", description: "engine")]
        let withClash = SlashCommand.withAppCommands(clash)
        XCTAssertEqual(withClash.map(\.name), ["/terminal", "/remote", "/visualize"], "not shadowed")
        XCTAssertEqual(withClash[0].description, "engine")

        XCTAssertEqual(SlashCommand.appCommand(chip: nil, draft: " /terminal ")?.action, .terminal)
        XCTAssertEqual(SlashCommand.appCommand(chip: listed[1], draft: "")?.action, .terminal)
        XCTAssertEqual(SlashCommand.appCommand(chip: listed[2], draft: "")?.action, .remote)
        XCTAssertEqual(SlashCommand.appCommand(chip: nil, draft: "/remote")?.action, .remote)
        XCTAssertNil(SlashCommand.appCommand(chip: engine[0], draft: ""))
        XCTAssertNil(SlashCommand.appCommand(chip: nil, draft: "open /terminal"))
    }

    func testVisualizeOpensTheGuideAndIsSuggestedByChartWords() {
        let listed = SlashCommand.withAppCommands([])
        XCTAssertEqual(SlashCommand.appCommand(chip: nil, draft: "/visualize")?.action, .visualizations)
        XCTAssertEqual(SlashCommand.suggestions(for: "/vis", in: listed).map(\.name), ["/visualize"])
        XCTAssertEqual(SlashCommand.suggestions(for: "/viz", in: listed).map(\.name), [])
        XCTAssertEqual(SlashCommand.dispatch("/visualize", in: listed, plain: true), .app(listed[2]))
    }

    func testConfigOptionsCarryModelAndEffort() {
        let options = ModelOptions(configOptions: [
            ["id": "model", "currentValue": "a", "options": [["value": "a", "name": "A"]]],
            ["id": "effort", "currentValue": "default", "options": [["value": "default", "name": "Off"], ["value": "high", "name": "High"]]],
        ])
        XCTAssertEqual(options.current, "a")
        XCTAssertEqual(options.effort, "default")
        XCTAssertEqual(options.efforts.map(\.name), ["Off", "High"])
        XCTAssertEqual(ACPRequests.setConfig(sessionId: "s", configId: "effort", value: "high")["configId"], "effort")
    }

    func testEngineLocatorPrefersBundledEngineBeforeCLI() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let app = root.appendingPathComponent("RustCode.app")
        let bundled = app.appendingPathComponent("Contents/Helpers/rustcode")
        try FileManager.default.createDirectory(at: bundled.deletingLastPathComponent(), withIntermediateDirectories: true)
        try Data().write(to: bundled)
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: bundled.path)
        let locator = EngineLocator(userSetting: nil, environment: [:], bundleURL: app,
                                    homeDirectory: root, isExecutable: EngineLocator.isExecutableFile)
        XCTAssertEqual(try locator.locate().source, .bundled)
    }
}

final class ACPConnectionOrderTests: XCTestCase {
    /// A response resolves at once while the updates sent before it may
    /// still be queued, so the connection counts what it delivered.
    func testInboundCountCoversUpdatesSentBeforeAResponse() async throws {
        let connection = ACPConnection(writer: { _ in })
        let pending = Task { try await connection.request("session/load", params: [:]) }
        while await connection.inboundCount == 0, !(await connection.hasPending) { await Task.yield() }
        for index in 0..<3 {
            await connection.receive(line: Data(#"{"jsonrpc":"2.0","method":"session/update","params":{"n":\#(index)}}"#.utf8))
        }
        await connection.receive(line: Data(#"{"jsonrpc":"2.0","id":1,"result":{}}"#.utf8))
        _ = try await pending.value
        let delivered = await connection.inboundCount
        XCTAssertEqual(delivered, 3)
    }
}
