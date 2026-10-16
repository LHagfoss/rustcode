import Foundation
import Testing
@testable import RustCodeRemote

@Suite struct CommandReceiptTests {
    @Test func commandsAndQuestionCancellationUseTargetedWireOperations() throws {
        let ref = SessionRef(sessionID: "s", registrationEpoch: 8)
        let codec = WireCodec()
        let cancel = try JSONDecoder().decode(WireRequest.self, from: Data(codec.encode(.cancelQuestion(ref, questionID: "q"), requestID: RequestID("r")).utf8))
        #expect(cancel.operation == .cancelQuestion(.init(questionID: "q")))
        #expect(cancel.registrationEpoch == 8)
        let command = try JSONDecoder().decode(WireRequest.self, from: Data(codec.encode(.executeCommand(ref, command: "/status"), requestID: RequestID("r")).utf8))
        #expect(command.operation == .executeCommand(.init(command: "/status")))
    }

    @Test func commandOutputSurvivesStatusLookupButUnknownIsNeverSuccess() throws {
        let codec = WireCodec()
        let command = #"{"type":"command_executed","command":"/status","title":"Status","output":"Ready"}"#
        let direct = #"{"kind":"response","protocol_version":1,"request_id":"r","receipt":"applied","result":\#(command)}"#
        let status = #"{"kind":"response","protocol_version":1,"request_id":"q","receipt":null,"result":{"type":"request_status","target_request_id":"r","receipt":"applied","result":\#(command)}}"#
        let result = SessionCommandResult(command: "/status", title: "Status", output: "Ready")
        #expect(try codec.decode(direct) == .reply(requestID: RequestID("r"), .commandExecuted(result)))
        #expect(try codec.decode(status) == .reply(requestID: RequestID("q"), .commandExecuted(result)))
        #expect(try codec.decode(direct.replacingOccurrences(of: "applied", with: "unknown")) == .reply(requestID: RequestID("r"), .receipt(.unknown)))
    }

    @Test func demoQuestionCancellationStopsChainAndAllowsNewTurn() async throws {
        let client = DemoRemoteClient(pace: .zero, includeQuestionChain: true)
        let credential = try await client.pair(.scanned("demo"), deviceName: "Test")
        try await client.connect(using: credential)
        let session = SessionRef(sessionID: DemoRemoteClient.questionsSessionID, registrationEpoch: 1)
        #expect(try await client.cancelQuestion("question-demo-1", in: session, requestID: RequestID()).state == .applied)
        let attachment = try await client.attach(session, resumeAfter: nil)
        guard case .snapshot(let state) = attachment.start else { Issue.record("missing snapshot"); return }
        #expect(!state.isRunning)
        #expect(state.pendingQuestion == nil)
        #expect(try await client.submit("Continue", to: session, requestID: RequestID()).state == .applied)
        await client.disconnect()
    }

    @Test func demoThreeQuestionsAdvanceWithoutEndingTheTurnBetweenAnswers() async throws {
        let client = DemoRemoteClient(pace: .zero, includeQuestionChain: true)
        let credential = try await client.pair(.scanned("demo"), deviceName: "Test")
        try await client.connect(using: credential)
        let session = SessionRef(sessionID: DemoRemoteClient.questionsSessionID, registrationEpoch: 1)
        for number in 1...3 {
            let attachment = try await client.attach(session, resumeAfter: nil)
            guard case .snapshot(let state) = attachment.start else { Issue.record("missing snapshot"); return }
            let question = try #require(state.pendingQuestion)
            #expect(question.id == "question-demo-\(number)")
            #expect(question.position == number)
            #expect(question.chainLength == 3)
            #expect(state.isRunning)
            let receipt = try await client.answerQuestion(question.id, with: QuestionAnswer(selectedOptionIDs: ["segments"]), in: session, requestID: RequestID())
            #expect(receipt.state == .applied)
            for try await update in attachment.updates {
                if let change = update.changes.first(where: { if case .questionChanged = $0 { true } else { false } }),
                   case .questionChanged(let next) = change {
                    #expect((next == nil) == (number == 3))
                    break
                }
            }
        }
        await client.disconnect()
    }
}
