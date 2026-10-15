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
