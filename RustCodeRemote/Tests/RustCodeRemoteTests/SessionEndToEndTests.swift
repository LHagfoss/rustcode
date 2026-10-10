import Foundation
import Testing
@testable import RustCodeRemote

/// The whole path against real processes on loopback: a gateway, a terminal session shared
/// with `/remote`, a model endpoint that streams a fixed reply, and this package's client.
///
///     RUSTCODE_GATEWAY_BIN=/path/to/rustcode swift test --filter SessionEndToEndTests
@Suite(.enabled(if: ProcessInfo.processInfo.environment["RUSTCODE_GATEWAY_BIN"] != nil), .serialized)
struct SessionEndToEndTests {
    private func configFile(modelPort: UInt16) -> String {
        """
        version = 1
        tool_protocol = "json"
        sandbox_mode = "trusted"
        discord_rpc_enabled = false
        auto_recap = false
        prompt_improver = false
        vision_model = "fake"

        [default]
        big = "fake"
        small = "fake"

        [[models]]
        name = "fake"
        url = "http://127.0.0.1:\(modelPort)/v1/chat/completions"
        model = "fake"
        context_window = 128000
        supports_vision = false
        supports_reasoning_effort = true
        reasoning_efforts = ["low", "high"]

        [[models]]
        name = "fake-fast"
        url = "http://127.0.0.1:\(modelPort)/v1/chat/completions"
        model = "fake-fast"
        context_window = 128000
        supports_vision = false
        supports_reasoning_effort = false
        """
    }

    /// Waits for a session list that satisfies `condition`.
    private func list(
        _ iterator: inout AsyncThrowingStream<[SessionSummary], any Error>.Iterator,
        where condition: ([SessionSummary]) -> Bool
    ) async throws -> [SessionSummary] {
        while let sessions = try await iterator.next() {
            if condition(sessions) { return sessions }
        }
        throw RemoteError.connectionClosed
    }

    @Test(.timeLimit(.minutes(2)))
    func questionBatchAnswersAndCancellationSettleBeforeAnotherTurnStarts() async throws {
        let model = try FakeModelServer(askQuestions: true)
        let modelPort = try await model.start()
        let config = configFile(modelPort: modelPort).replacingOccurrences(of: "tool_protocol = \"json\"", with: "tool_protocol = \"apinative\"")
        let gateway = try await GatewayIntegrationTests.Gateway(configFile: config)
        let workspace = gateway.configuration.appendingPathComponent("workspace")
        try FileManager.default.createDirectory(at: workspace, withIntermediateDirectories: true)
        let terminal = try TerminalSession(binary: gateway.binary, configuration: gateway.configuration, workspace: workspace)
        defer { terminal.stop(); gateway.stop(); model.stop() }
        let client = LiveRemoteClient(codec: WireCodec(), requestTimeout: .seconds(5))
        let credential = try await client.pair(.scanned(try gateway.pairingChallenge().payload), deviceName: "Question test")
        try await client.connect(using: credential)
        var lists = try await client.subscribeSessions().makeAsyncIterator()
        _ = try await lists.next()
        try await Task.sleep(for: .seconds(3))
        terminal.type("/remote\r")
        let shared = try await list(&lists) { !$0.isEmpty }
        let session = try #require(shared.first)
        try await Task.sleep(for: .milliseconds(500))
        terminal.type("\u{1b}")
        let attachment = try await client.attach(session.ref, resumeAfter: nil)
        guard case .snapshot(var state) = attachment.start else { Issue.record("missing snapshot"); return }
        var updates = attachment.updates.makeAsyncIterator()
        #expect(try await client.submit("Ask three questions", to: session.ref, requestID: RequestID()).state == .applied)
        while state.pendingQuestion == nil, let update = try await updates.next() {
            state.apply(update)
            if update.changes.contains(where: { if case .turnFinished = $0 { true } else { false } }) { break }
        }
        let first = try #require(state.pendingQuestion)
        let answerID = RequestID()
        #expect(try await client.answerQuestion(first.id, with: QuestionAnswer(selectedOptionIDs: [first.options[0].id]), in: session.ref, requestID: answerID).state == .applied)
        while state.pendingQuestion == nil || state.pendingQuestion?.id == first.id, let update = try await updates.next() {
            state.apply(update)
            if update.changes.contains(where: { if case .turnFinished = $0 { true } else { false } }) { break }
        }
        let second = try #require(state.pendingQuestion)
        #expect(state.isRunning)
        #expect(try await client.getRequestStatus(answerID, in: session.ref).state == .applied)
        let cancelID = RequestID()
        #expect(try await client.cancelQuestion(second.id, in: session.ref, requestID: cancelID).state == .applied)
        #expect(try await client.getRequestStatus(cancelID, in: session.ref).state == .applied)
        while state.isRunning, let update = try await updates.next() { state.apply(update) }
        #expect(state.pendingQuestion == nil)
        #expect(try await client.submit("Continue after cancelling", to: session.ref, requestID: RequestID()).state == .applied)
        var sawNewTurn = false
        while let update = try await updates.next() {
            state.apply(update)
            if state.isRunning { sawNewTurn = true }
            if sawNewTurn && !state.isRunning { break }
        }
        #expect(state.items.contains { item in
            if case .assistant(let text) = item.content { text.text.contains("Hello from") } else { false }
        })
        await client.disconnect()
    }

    @Test(.timeLimit(.minutes(2)))
    func aSharedTerminalSessionIsListedAttachedDrivenAndClosed() async throws {
        let model = try FakeModelServer()
        let modelPort = try await model.start()
        let gateway = try await GatewayIntegrationTests.Gateway(configFile: configFile(modelPort: modelPort))
        let workspace = gateway.configuration.appendingPathComponent("workspace")
        try FileManager.default.createDirectory(at: workspace, withIntermediateDirectories: true)
        let terminal = try TerminalSession(binary: gateway.binary, configuration: gateway.configuration, workspace: workspace)
        let log = FrameLog()
        defer {
            log.flush()
            terminal.stop()
            gateway.stop()
            model.stop()
        }
        let client = LiveRemoteClient(codec: WireCodec(), requestTimeout: .seconds(15)) { RecordingTransport(log: log) }

        // Pair and connect before anything is shared.
        let credential = try await client.pair(.scanned(try gateway.pairingChallenge().payload), deviceName: "E2E Phone")
        let identity = try await client.connect(using: credential)
        #expect(identity.gatewayID == credential.host.gatewayID)
        var lists = try await client.subscribeSessions().makeAsyncIterator()
        #expect(try await lists.next() == [])

        // Share the terminal session.
        try await Task.sleep(for: .seconds(3))
        terminal.type("/remote\r")
        let shared = try await list(&lists) { !$0.isEmpty }
        let session = try #require(shared.first)
        #expect(shared.count == 1)
        #expect(session.model == "fake")
        #expect(session.turnCount == 0)
        #expect(session.activity == .idle)
        #expect(session.health == .healthy)
        #expect(session.workspace.hasSuffix("workspace"))
        try await Task.sleep(for: .milliseconds(500))
        terminal.type("\u{1b}")

        // Attach: a snapshot of an idle, empty session.
        let attachment = try await client.attach(session.ref, resumeAfter: nil)
        guard case .snapshot(var state) = attachment.start else {
            Issue.record("expected a snapshot")
            return
        }
        #expect(state.session == session.ref)
        #expect(!state.isRunning)
        var updates = attachment.updates.makeAsyncIterator()
        let settings = try #require(state.settings)
        #expect(settings.selectedModel == "fake")
        #expect(settings.reasoningEffort == "default")
        #expect(settings.canChange)
        #expect(settings.models.first { $0.id == "fake" }?.reasoningEfforts == ["default", "low", "high"])
        #expect(settings.models.first { $0.id == "fake-fast" }?.reasoningEfforts == ["default"])

        // Profile and effort changes arrive through authoritative replacement snapshots.
        let switchID = RequestID()
        let switched = try await client.setSessionSettings(model: "fake-fast", reasoningEffort: "default", in: session.ref, requestID: switchID)
        #expect(switched.state == .applied)
        while state.settings?.selectedModel != "fake-fast", let update = try await updates.next() {
            #expect(state.apply(update) == .applied)
        }
        #expect(state.settings?.selectedModel == "fake-fast")
        #expect(try await client.getRequestStatus(switchID, in: session.ref).state == .applied)
        let restored = try await client.setSessionSettings(model: "fake", reasoningEffort: "high", in: session.ref, requestID: RequestID())
        #expect(restored.state == .applied)
        while state.settings?.selectedModel != "fake" || state.settings?.reasoningEffort != "high",
              let update = try await updates.next() {
            #expect(state.apply(update) == .applied)
        }
        #expect(state.settings?.selectedModel == "fake")
        #expect(state.settings?.reasoningEffort == "high")

        #expect(state.capabilities.contains("cancel_question"))
        #expect(state.capabilities.contains("execute_command"))
        let commandID = RequestID()
        let status = try await client.executeCommand("/status", in: session.ref, requestID: commandID)
        #expect(status.state == .applied)
        #expect(status.commandResult?.command == "/status")
        #expect(status.commandResult?.output.isEmpty == false)
        let checked = try await client.getRequestStatus(commandID, in: session.ref)
        #expect(checked == status)
        #expect(state.items.isEmpty)
        let staleQuestion = try await client.cancelQuestion("missing", in: session.ref, requestID: RequestID())
        #expect(staleQuestion.state == .rejected(.staleQuestion))

        // A slash command from the phone is refused, and never runs.
        let slash = try await client.submit("/exit", to: session.ref, requestID: RequestID())
        #expect(slash.state == .rejected(.unsupportedOperation))

        // Run a turn from the phone and follow it event by event.
        let promptID = RequestID()
        let receipt = try await client.submit("Say hello", to: session.ref, requestID: promptID)
        #expect(receipt.state == .applied)

        var sawTurnStart = false
        var queued = false
        var finishedTurns = 0
        var queueID: RequestID?
        while finishedTurns < 2, let update = try await updates.next() {
            // Every event applies in sequence: no gap, no duplicate.
            #expect(state.apply(update) == .applied, "event \(update.sequence) did not apply at \(state.sequence)")
            for change in update.changes {
                switch change {
                case .turnStarted:
                    sawTurnStart = true
                case .responseDelta where !queued:
                    // While the turn streams, a second submit is busy and a queued prompt is accepted.
                    queued = true
                    let busy = try await client.submit("Too early", to: session.ref, requestID: RequestID())
                    #expect(busy.state == .rejected(.busy))
                    let busySettings = try await client.setSessionSettings(model: "fake-fast", reasoningEffort: "default", in: session.ref, requestID: RequestID())
                    #expect(busySettings.state == .rejected(.busy))
                    let id = RequestID()
                    queueID = id
                    let queuedReceipt = try await client.queue("And again", in: session.ref, requestID: id)
                    #expect(queuedReceipt.state == .applied)
                case .turnFinished(_, let outcome):
                    #expect(outcome == .completed)
                    finishedTurns += 1
                default:
                    break
                }
            }
        }
        #expect(sawTurnStart)
        #expect(finishedTurns == 2)
        #expect(!state.isRunning)

        let users = state.items.compactMap { item -> String? in
            if case .user(let text) = item.content { text } else { nil }
        }
        #expect(users.contains("Say hello"))
        #expect(users.contains("And again"))
        let responses = state.items.compactMap { item -> String? in
            if case .assistant(let assistant) = item.content { assistant.text } else { nil }
        }
        let expected = FakeModelServer.reply.joined()
        #expect(responses.contains { $0.contains(expected) }, "streamed text was \(responses)")

        // The outcome of a mutation can be asked for afterwards; an ID nobody saw is unknown.
        #expect(try await client.getRequestStatus(promptID, in: session.ref).state == .applied)
        if let queueID {
            #expect(try await client.getRequestStatus(queueID, in: session.ref).state == .applied)
        }
        #expect(try await client.getRequestStatus(RequestID(), in: session.ref).state == .unknown)

        // A stale cancel cannot stop anything.
        let cancel = try await client.cancel(turnID: "turn:0:0", in: session.ref, requestID: RequestID())
        #expect(cancel.state == .rejected(.staleTurn))

        // Reconnect and resume from the last applied event.
        await client.disconnect()
        try await client.connect(using: credential)
        lists = try await client.subscribeSessions().makeAsyncIterator()
        let completedSessions = try await list(&lists) { sessions in
            sessions.first?.turnCount == 2 && sessions.first?.activity == .idle
        }
        #expect(completedSessions.first?.ref == session.ref)
        #expect(completedSessions.first?.model == "fake")
        #expect(completedSessions.first?.turnCount == 2)
        let resumed = try await client.attach(session.ref, resumeAfter: state.sequence)
        switch resumed.start {
        case .resumed:
            break
        case .snapshot(let fresh):
            // Also valid: control availability changes at turn completion publish a
            // new snapshot cut, so the earlier event cursor may no longer be replayable.
            #expect(fresh.sequence >= state.sequence)
            state = fresh
        }
        updates = resumed.updates.makeAsyncIterator()

        // A fresh attach shows the finished conversation from the transcript.
        let reattached = try await client.attach(session.ref, resumeAfter: nil)
        guard case .snapshot(let after) = reattached.start else {
            Issue.record("expected a snapshot")
            return
        }
        #expect(after.settings?.selectedModel == "fake")
        #expect(after.settings?.reasoningEffort == "high")
        #expect(after.settings?.canChange == true)
        let transcript = after.items.compactMap { item -> String? in
            switch item.content {
            case .user(let text): "user: \(text)"
            case .assistant(let assistant): "assistant: \(assistant.text)"
            default: nil
            }
        }
        #expect(transcript.contains("user: Say hello"))
        #expect(transcript.contains { $0.hasPrefix("assistant: ") && $0.contains(expected) })
        updates = reattached.updates.makeAsyncIterator()

        // `/remote off` in the terminal: the session closes and leaves the list.
        terminal.type("/remote off\r")
        #expect(try await list(&lists) { $0.isEmpty } == [])
        do {
            while try await updates.next() != nil {}
            Issue.record("the update stream should end with session_closed")
        } catch {
            guard case RemoteError.sessionClosed = error else {
                Issue.record("unexpected end of the update stream: \(error)")
                return
            }
        }
        let late = try await client.submit("Anyone there?", to: session.ref, requestID: RequestID())
        if case .rejected = late.state {} else {
            Issue.record("a prompt to a closed session was not rejected: \(late.state)")
        }

        // Revoking the device closes the connection for good.
        try gateway.run("remote", "revoke", credential.host.device.id)
        await #expect(throws: RemoteError.unauthorized) {
            while try await lists.next() != nil {}
        }
        await #expect(throws: RemoteError.unauthorized) {
            try await client.connect(using: credential)
        }
    }
}
