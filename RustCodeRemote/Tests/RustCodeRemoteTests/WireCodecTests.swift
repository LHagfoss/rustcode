import Foundation
import Synchronization
import Testing
@testable import RustCodeRemote

@Suite struct WireCodecTests {
    let codec = WireCodec()
    let session = SessionRef(sessionID: "5f0c2d9e-7a41-4c1b-9d55-0b6c1f2a3e44", registrationEpoch: 3)

    private func golden(_ path: String) throws -> String {
        try String(contentsOf: Contract.root.appendingPathComponent("golden/\(path).json"), encoding: .utf8)
    }

    private func decodeGolden(_ name: String) throws -> RemoteMessage {
        try codec.decode(try golden("frames/\(name)"))
    }

    // MARK: Handshake

    @Test func handshakeRequestsHaveTheGatewaysExactShape() throws {
        let code = try codec.encode(.pair(credential: .shortCode("1234-5678"), deviceName: "phone"), requestID: RequestID())
        #expect(try Contract.json(Data(code.utf8)) == Contract.json(Data(
            #"{"type":"pair","protocol_version":1,"method":"code","secret":"1234-5678","device_name":"phone"}"#.utf8
        )))
        let credential = try codec.encode(.pair(credential: .oneTime("cred"), deviceName: "phone"), requestID: RequestID())
        #expect(try Contract.json(Data(credential.utf8)) == Contract.json(Data(
            #"{"type":"pair","protocol_version":1,"method":"credential","secret":"cred","device_name":"phone"}"#.utf8
        )))
        let authenticate = try codec.encode(
            .authenticate(device: DeviceIdentity(id: "0011223344556677", name: "phone"), deviceToken: "tok"),
            requestID: RequestID()
        )
        #expect(try Contract.json(Data(authenticate.utf8)) == Contract.json(Data(
            #"{"type":"authenticate","protocol_version":1,"device_id":"0011223344556677","token":"tok"}"#.utf8
        )))
        // No request ID and no device name leak into the authenticate frame.
        #expect(!authenticate.contains("request_id"))
    }

    @Test func handshakeRepliesDecode() throws {
        let identity = ConnectionIdentity(gatewayID: "gw", instanceID: "inst")
        #expect(try codec.decode(
            #"{"type":"paired","protocol_version":1,"gateway_id":"gw","instance_id":"inst","device_id":"dev","device_name":"phone","token":"tok"}"#
        ) == .handshake(.paired(PairingGrant(
            identity: identity,
            device: DeviceIdentity(id: "dev", name: "phone"),
            deviceToken: "tok"
        ))))
        #expect(try codec.decode(
            #"{"type":"authenticated","protocol_version":1,"gateway_id":"gw","instance_id":"inst","device_id":"dev","device_name":"phone"}"#
        ) == .handshake(.authenticated(identity)))
    }

    @Test func gatewayErrorsMapToWhatTheAppDoesAboutThem() throws {
        func failure(_ code: String, extra: String = "") throws -> GatewayFailure {
            guard case .gatewayFailure(let failure) = try codec.decode(#"{"type":"error","code":"\#(code)","message":"m"\#(extra)}"#) else {
                throw RemoteError.invalidFrame(code)
            }
            return failure
        }
        #expect(try failure("unauthorized").error == .unauthorized)
        #expect(try failure("revoked") == GatewayFailure(error: .unauthorized, closesConnection: true))
        #expect(try failure("rate_limited", extra: #","retry_after_secs":240"#).error == .rateLimited(retryAfter: .seconds(240)))
        #expect(try failure("rate_limited").error == .rateLimited(retryAfter: nil))
        #expect(try failure("unsupported_version").error == .incompatibleProtocol)
        #expect(try failure("not_implemented") == GatewayFailure(error: .notImplemented, closesConnection: false))
        #expect(try failure("shutting_down").closesConnection)
        #expect(try failure("idle_timeout").closesConnection)
        guard case .pairingFailed = try failure("pairing_failed").error else {
            Issue.record("pairing_failed should be a pairing failure")
            return
        }
        // Every code the gateway documents is known, so none falls through as "unknown".
        for code in WireGatewayErrorCode.allCases {
            _ = try failure(code.rawValue)
        }
        #expect(WireGatewayErrorCode.allCases.count == 14)
    }

    // MARK: Pairing input

    @Test func aScannedCodeYieldsAddressCredentialAndExpectedGateway() throws {
        let payload = #"{"protocol_version":1,"address":"192.168.1.20:17879","gateway_id":"gw-1","credential":"abc"}"#
        #expect(try codec.pairingTarget(for: .scanned(payload)) == PairingTarget(
            address: HostAddress(host: "192.168.1.20", port: 17879),
            credential: .oneTime("abc"),
            expectedGatewayID: "gw-1"
        ))
    }

    @Test func aManualCodeYieldsTheShortCode() throws {
        let address = HostAddress(host: "100.90.1.2", port: 17879)
        #expect(try codec.pairingTarget(for: .manual(address: address, code: " 1234-5678 ")) == PairingTarget(
            address: address,
            credential: .shortCode("1234-5678")
        ))
    }

    @Test func unusableScansAreRefusedBeforeAnyConnection() {
        #expect(throws: RemoteError.incompatibleProtocol) {
            try codec.pairingTarget(for: .scanned(#"{"protocol_version":2,"address":"a:1","gateway_id":"g","credential":"c"}"#))
        }
        for text in ["https://example.com", #"{"protocol_version":1,"address":"no-port","gateway_id":"g","credential":"c"}"#] {
            do {
                _ = try codec.pairingTarget(for: .scanned(text))
                Issue.record("\(text) should be refused")
            } catch {
                guard case RemoteError.pairingFailed = error else {
                    Issue.record("unexpected error \(error)")
                    continue
                }
            }
        }
    }

    @Test func theEndpointIsAPlainWebSocketWithNoHeadersAndNoSecret() throws {
        let endpoint = try codec.endpoint(for: HostAddress(host: "192.168.1.20", port: 17879))
        #expect(endpoint.url.absoluteString == "ws://192.168.1.20:17879/")
        #expect(endpoint.headers.isEmpty)
        #expect(try codec.endpoint(for: HostAddress(host: "fd00::1", port: 17879)).url.absoluteString == "ws://[fd00::1]:17879/")
    }

    // MARK: Requests against the golden files

    @Test func domainCommandsEncodeToTheGoldenRequests() throws {
        let cases: [(String, String, RemoteCommand)] = [
            ("subscribe_sessions", "req-0002", .subscribeSessions),
            ("attach_session", "req-0003", .attachSession(session, resume: nil)),
            ("attach_session_resume", "req-0004", .attachSession(session, resume: ResumeCursor(gatewayID: "gw-8e1f4b7a", lastSequence: 412))),
            ("detach_session", "req-0005", .detachSession(session)),
            ("get_history", "req-0006", .getHistory(session, before: HistoryCursor("60@h2"), limit: 25)),
            ("get_content", "req-0007", .getContent(session, contentID: "approval:controller:48213:7:0", offset: 16384, maxBytes: 65536)),
            ("submit_prompt", "", .submitPrompt(session, text: "")),
            ("steer", "", .steer(session, text: "")),
            ("queue", "", .queue(session, text: "")),
            ("cancel_turn", "", .cancelTurn(session, turnID: "")),
            ("answer_question_selected", "req-0012", .answerQuestion(session, questionID: "question:48213:4", answer: QuestionAnswer(selectedOptionIDs: ["Rebase onto main"]))),
            ("answer_question_custom", "", .answerQuestion(session, questionID: "", answer: QuestionAnswer())),
            ("resolve_approval", "req-0014", .resolveApproval(session, batchID: "controller:48213:7", decision: .approve)),
            ("get_request_status", "req-0015", .getRequestStatus(session, target: RequestID("req-0008"))),
        ]
        for (name, _, template) in cases {
            let text = try golden("requests/\(name)")
            let wire = try JSONDecoder().decode(WireRequest.self, from: Data(text.utf8))
            // Fill the free-text fields from the golden file, so each case states only its shape.
            let command: RemoteCommand = switch (template, wire.operation) {
            case (.submitPrompt, .submitPrompt(let payload)): .submitPrompt(session, text: payload.prompt)
            case (.steer, .steer(let payload)): .steer(session, text: payload.prompt)
            case (.queue, .queue(let payload)): .queue(session, text: payload.prompt)
            case (.cancelTurn, .cancelTurn(let payload)): .cancelTurn(session, turnID: payload.turnID)
            case (.answerQuestion, .answerQuestion(let payload)):
                if case .custom(let custom) = payload.answer {
                    .answerQuestion(session, questionID: payload.questionID, answer: QuestionAnswer(text: custom.text))
                } else {
                    template
                }
            default: template
            }
            let encoded = try codec.encode(command, requestID: RequestID(wire.requestID))
            #expect(try Contract.json(Data(encoded.utf8)) == Contract.json(Data(text.utf8)), "\(name) differs from the golden request")
        }
        // `list_sessions` is the one operation the app does not use: it subscribes instead.
        #expect(cases.count == 14)
    }

    @Test func typedTextWinsOverASelectionBecauseTheWireTakesOnlyOne() {
        #expect(WireCodec.answer(QuestionAnswer(selectedOptionIDs: ["A"], text: " other ")) == .custom(.init(text: "other")))
        #expect(WireCodec.answer(QuestionAnswer(selectedOptionIDs: ["A", "B"], text: " ")) == .selected(.init(options: ["A", "B"])))
    }

    @Test func anOversizedPromptIsRefusedLocally() {
        let prompt = String(repeating: "x", count: WireCodec.maximumOutgoingFrameBytes)
        #expect(throws: (any Error).self) {
            try codec.encode(.submitPrompt(session, text: prompt), requestID: RequestID())
        }
    }

    // MARK: Frames

    @Test func responsesMapToRepliesAndReceipts() throws {
        func reply(_ name: String) throws -> RemoteReply {
            guard case .reply(_, let reply) = try decodeGolden(name) else { throw RemoteError.invalidFrame(name) }
            return reply
        }
        #expect(try reply("response_prompt_accepted") == .receipt(.applied))
        #expect(try reply("response_turn_cancelled") == .receipt(.applied))
        #expect(try reply("response_question_answered") == .receipt(.applied))
        #expect(try reply("response_approval_resolved") == .receipt(.applied))
        #expect(try reply("response_detached") == .detached)
        #expect(try reply("response_resumed") == .attached(.resumed, gatewayID: "gw-8e1f4b7a"))
        #expect(try reply("response_request_status") == .receipt(.applied))
        #expect(try reply("response_request_status_received") == .receipt(.received))
        #expect(try reply("response_request_status_unknown") == .receipt(.unknown))

        #expect(try reply("response_error_busy") == .failure(.rejected(.busy)))
        #expect(try reply("response_error_not_running") == .failure(.rejected(.notRunning)))
        #expect(try reply("response_error_stale_turn") == .failure(.rejected(.staleTurn)))
        #expect(try reply("response_error_stale_question") == .failure(.rejected(.staleQuestion)))
        #expect(try reply("response_error_stale_approval") == .failure(.rejected(.staleApproval)))
        #expect(try reply("response_error_stale_session") == .failure(.rejected(.staleSession)))
        #expect(try reply("response_error_invalid_answer") == .failure(.rejected(.invalidAnswer)))
        #expect(try reply("response_error_unsupported_operation") == .failure(.rejected(.unsupportedOperation)))
        #expect(try reply("response_error_incompatible_version") == .failure(.incompatibleProtocol))
        // The owner could not be asked: that is uncertainty, never a rejection.
        #expect(try reply("response_error_owner_unavailable") == .failure(.outcomeUnknown))

        guard case .content(let chunk) = try reply("response_content") else {
            Issue.record("expected content")
            return
        }
        #expect(chunk.offset == 16384)
        #expect(chunk.nextOffset == 16420)
        #expect(chunk.totalBytes == 48211)
    }

    @Test func sessionListsMap() throws {
        guard case .reply(_, .sessions(let sessions)) = try decodeGolden("response_sessions"),
              case .sessionsChanged(let pushed) = try decodeGolden("sessions") else {
            Issue.record("expected session lists")
            return
        }
        let first = try #require(sessions.first)
        #expect(first.ref == session)
        #expect(first.title == "Fix the failing parser tests")
        #expect(first.activity == .waitingForUser)
        #expect(first.attention == SessionAttention(pendingQuestions: 0, pendingApprovals: 1))
        #expect(first.health == .healthy)
        #expect(!pushed.isEmpty)
    }

    @Test func theSnapshotCarriesEverythingTheScreenShows() throws {
        guard case .sessionUpdate(let update) = try decodeGolden("event_snapshot"),
              case .replaced(let snapshot) = try #require(update.changes.first) else {
            Issue.record("expected a snapshot event")
            return
        }
        #expect(update.sequence == 424)
        #expect(snapshot.session == session)
        #expect(snapshot.sequence == 424)
        #expect(snapshot.items.map(\.id) == ["m60", "m61"])
        #expect(snapshot.items[0].content == .user("Why does the parser test fail?"))
        guard case .tool(let tool) = snapshot.items[1].content else {
            Issue.record("expected a tool row")
            return
        }
        #expect(tool.name == "run_command")
        #expect(tool.summary == "cargo test parser")
        #expect(tool.status == .failed)
        #expect(tool.isOutputTruncated)
        #expect(snapshot.historyCursor == HistoryCursor("60@h2"))

        let question = try #require(snapshot.pendingQuestion)
        #expect(question.id == "question:48213:4")
        #expect(question.header == "Branch")
        #expect(question.options.map(\.id) == ["Rebase onto main", "Merge main"])
        #expect(question.options[0].detail == "Replays the two local commits")
        #expect((question.position, question.chainLength) == (1, 2))
        #expect(question.allowsFreeText)

        #expect(snapshot.pendingPrompts == [PendingPrompt(kind: .queue, text: "Then open a pull request")])
        #expect(snapshot.backgroundTasks == [BackgroundTask(id: "task-3", command: "cargo watch -x check", elapsed: .milliseconds(93400))])
        #expect(snapshot.subagents.map(\.id) == ["subagent:2"])
        #expect(snapshot.omitted.total == 0)
        #expect(!snapshot.isRunning)

        // A snapshot event replaces whatever the client had.
        var state = SessionSnapshot(session: session, items: [TranscriptItem(id: "old", .user("old"))], activeTurnID: "t", sequence: 423)
        #expect(state.apply(update) == .applied)
        #expect(state.items.map(\.id) == ["m60", "m61"])
        #expect(state.sequence == 424)
        #expect(state.activeTurnID == nil)
    }

    @Test func anAttachedSnapshotWithARunningTurnShowsItsToolsAndLiveText() throws {
        guard case .reply(_, .attached(.snapshot(let snapshot), let gatewayID)) = try decodeGolden("response_attached") else {
            Issue.record("expected an attached snapshot")
            return
        }
        #expect(gatewayID == "gw-8e1f4b7a")
        let wire = try JSONDecoder().decode(WireFrame.self, from: Data(try golden("frames/response_attached").utf8))
        guard case .response(let response) = wire, case .attached(let attached) = response.result else { return }
        #expect(snapshot.activeTurnID == attached.snapshot.turn?.turnID)
        #expect(snapshot.canSteer == (attached.snapshot.turn?.canSteer ?? true))
        let toolCount = attached.snapshot.turn?.tools.count ?? 0
        #expect(snapshot.items.count >= attached.snapshot.transcript.count + toolCount)
    }

    @Test func aTurnOfEventsBuildsTheTranscript() throws {
        var state = SessionSnapshot(session: session, sequence: 412)
        for name in [
            "event_turn_started", "event_text_delta", "event_tool_started", "event_tool_finished",
            "event_subagent_updated", "event_approval_requested",
        ] {
            guard case .sessionUpdate(let update) = try decodeGolden(name) else {
                Issue.record("\(name) is not an update")
                return
            }
            #expect(state.apply(update) == .applied, "\(name) did not apply in sequence")
        }
        #expect(state.sequence == 418)
        #expect(state.activeTurnID == "turn:48213:12")
        #expect(state.items.count == 3)
        #expect(state.items[0].content == .user("Run the test suite and fix what fails"))
        // The tool row ended the response text that came before it.
        #expect(state.items[1].content == .assistant(AssistantText(text: "The failure comes from ", isStreaming: false)))
        guard case .tool(let tool) = state.items[2].content else {
            Issue.record("expected a tool row")
            return
        }
        #expect(state.items[2].id == "tool:call_Jq3")
        #expect(tool.name == "view_file")
        #expect(tool.status == .succeeded)
        #expect(tool.output == "use std::collections::HashMap;\n")
        #expect(tool.isOutputTruncated)
        #expect(state.subagents.map(\.id) == ["subagent:2"])

        let approval = try #require(state.pendingApproval)
        #expect(approval.batchID == "controller:48213:7")
        #expect(approval.items.map(\.isDetailComplete) == [true, false])
        #expect(approval.items[1].detailContentID == "approval:controller:48213:7:1")
        #expect(approval.items[1].detailTotalBytes == 48211)
        #expect(!approval.canBeResolved)

        // Resolving another batch leaves this one pending; resolving it clears it.
        state.apply(SessionUpdate(session: session, sequence: 419, event: .approvalResolved(batchID: "other")))
        #expect(state.pendingApproval != nil)
        guard case .sessionUpdate(let resolved) = try decodeGolden("event_approval_resolved") else { return }
        state.sequence = resolved.sequence - 1
        state.apply(resolved)
        #expect(state.pendingApproval == nil)
    }

    @Test func turnEndingsAndQuestions() throws {
        var state = SessionSnapshot(session: session, activeTurnID: "turn:48213:12")
        func apply(_ name: String) throws {
            guard case .sessionUpdate(let update) = try decodeGolden(name) else { throw RemoteError.invalidFrame(name) }
            state.sequence = update.sequence - 1
            #expect(state.apply(update) == .applied)
        }
        try apply("event_question_requested")
        #expect(state.pendingQuestion != nil)
        try apply("event_question_resolved")
        #expect(state.pendingQuestion == nil)
        try apply("event_turn_finished")
        #expect(!state.isRunning)

        state.activeTurnID = "turn:48213:12"
        try apply("event_turn_cancelled")
        #expect(!state.isRunning)

        // A turn that ended without naming itself ends whatever was running.
        state.activeTurnID = "some-turn"
        state.apply(SessionUpdate(session: session, sequence: state.sequence + 1, event: .turnFinished(turnID: nil, outcome: .completed)))
        #expect(!state.isRunning)
    }

    @Test func resyncAndCloseAreNotTranscriptUpdates() throws {
        #expect(try decodeGolden("event_resync_required") == .resyncRequired(session))
        #expect(try decodeGolden("event_session_closed") == .sessionClosed(session, reason: "Sharing was turned off for this session."))
    }

    @Test func historyPagesMap() throws {
        guard case .reply(_, .history(let page)) = try decodeGolden("response_history") else {
            Issue.record("expected a history page")
            return
        }
        #expect(page.items.map(\.id) == ["m60", "m61"])
        #expect(page.nextCursor == HistoryCursor("35@h2"))
    }
}

/// Pairing and reconnecting, end to end through the real codec over a scripted socket.
@Suite struct WirePairingFlowTests {
    private let paired = #"{"type":"paired","protocol_version":1,"gateway_id":"gw-1","instance_id":"inst-1","device_id":"0011223344556677","device_name":"Phone","token":"device-token"}"#
    private let authenticated = #"{"type":"authenticated","protocol_version":1,"gateway_id":"gw-1","instance_id":"inst-2","device_id":"0011223344556677","device_name":"Phone"}"#

    /// A gateway reduced to its handshake: answers the first frame, then `not_implemented`.
    private func gateway(handshake reply: String) -> ScriptedTransport {
        ScriptedTransport { frame in
            frame.contains(#""type":"pair""#) || frame.contains(#""type":"authenticate""#)
                ? [reply]
                : [#"{"type":"error","code":"not_implemented","message":"this gateway shares no sessions yet"}"#]
        }
    }

    private func sentJSON(_ transport: ScriptedTransport, at index: Int = 0) async throws -> [String: Any] {
        let frame = await transport.sent[index]
        return try #require(try JSONSerialization.jsonObject(with: Data(frame.utf8)) as? [String: Any])
    }

    @Test func aScannedCodePairsWithItsCredentialAndYieldsAStorableCredential() async throws {
        let transport = gateway(handshake: paired)
        let client = LiveRemoteClient(codec: WireCodec()) { transport }
        let qr = #"{"protocol_version":1,"address":"192.168.1.20:17879","gateway_id":"gw-1","credential":"one-time-credential"}"#
        let credential = try await client.pair(.scanned(qr), deviceName: "Phone")

        #expect(await transport.connectedURL?.absoluteString == "ws://192.168.1.20:17879/")
        #expect(await transport.connectedHeaders.isEmpty)
        let sent = try await sentJSON(transport)
        #expect(sent["type"] as? String == "pair")
        #expect(sent["method"] as? String == "credential")
        #expect(sent["secret"] as? String == "one-time-credential")
        #expect(sent["device_name"] as? String == "Phone")

        #expect(credential.deviceToken == "device-token")
        #expect(credential.host.gatewayID == "gw-1")
        #expect(credential.host.device == DeviceIdentity(id: "0011223344556677", name: "Phone"))
        #expect(credential.host.address == HostAddress(host: "192.168.1.20", port: 17879))
        #expect(await transport.isClosed)

        // What goes into the keychain store is exactly that, and it survives a round trip.
        let store = InMemoryCredentialStore()
        store.save(credential)
        #expect(try JSONDecoder().decode(StoredCredential.self, from: JSONEncoder().encode(credential)) == store.load())
    }

    @Test func aManualAddressPairsWithTheCode() async throws {
        let transport = gateway(handshake: paired)
        let client = LiveRemoteClient(codec: WireCodec()) { transport }
        _ = try await client.pair(
            .manual(address: HostAddress(host: "100.90.1.2", port: 17879), code: "1234-5678"),
            deviceName: "Phone"
        )
        let sent = try await sentJSON(transport)
        #expect(sent["method"] as? String == "code")
        #expect(sent["secret"] as? String == "1234-5678")
        #expect(await transport.connectedURL?.absoluteString == "ws://100.90.1.2:17879/")
    }

    @Test func aCodeForAnotherGatewayIsRefused() async {
        let transport = gateway(handshake: paired)
        let client = LiveRemoteClient(codec: WireCodec()) { transport }
        let qr = #"{"protocol_version":1,"address":"192.168.1.20:17879","gateway_id":"gw-other","credential":"c"}"#
        do {
            _ = try await client.pair(.scanned(qr), deviceName: "Phone")
            Issue.record("pairing should have failed")
        } catch {
            guard case RemoteError.pairingFailed = error else {
                Issue.record("unexpected error \(error)")
                return
            }
        }
    }

    @Test func pairingFailuresSayWhy() async {
        for (frame, expected) in [
            (#"{"type":"error","code":"pairing_failed","message":"pairing failed"}"#, RemoteError.pairingFailed("the code is wrong, expired or already used")),
            (#"{"type":"error","code":"rate_limited","message":"locked","retry_after_secs":240}"#, RemoteError.rateLimited(retryAfter: .seconds(240))),
            (#"{"type":"error","code":"unsupported_version","message":"v1 only"}"#, RemoteError.incompatibleProtocol),
        ] {
            let transport = gateway(handshake: frame)
            let client = LiveRemoteClient(codec: WireCodec()) { transport }
            await #expect(throws: expected) {
                _ = try await client.pair(.manual(address: HostAddress(host: "10.0.0.2", port: 17879), code: "0000-0000"), deviceName: "Phone")
            }
            #expect(await transport.isClosed)
        }
        #expect(RemoteError.rateLimited(retryAfter: .seconds(240)).localizedDescription == "Too many failed attempts. Try again in 4 minutes.")
    }

    private func storedCredential() -> StoredCredential {
        StoredCredential(
            host: PairedHost(
                gatewayID: "gw-1",
                name: "192.168.1.20",
                address: HostAddress(host: "192.168.1.20", port: 17879),
                device: DeviceIdentity(id: "0011223344556677", name: "Phone"),
                pairedAt: Date(timeIntervalSince1970: 0)
            ),
            deviceToken: "device-token"
        )
    }

    @Test func reconnectingAuthenticatesWithTheStoredDeviceAndToken() async throws {
        let transport = gateway(handshake: authenticated)
        let client = LiveRemoteClient(codec: WireCodec()) { transport }
        let identity = try await client.connect(using: storedCredential())

        let sent = try await sentJSON(transport)
        #expect(sent["type"] as? String == "authenticate")
        #expect(sent["device_id"] as? String == "0011223344556677")
        #expect(sent["token"] as? String == "device-token")
        #expect(sent["protocol_version"] as? Int == 1)
        #expect(await transport.connectedURL?.absoluteString.contains("device-token") == false)
        #expect(identity == ConnectionIdentity(gatewayID: "gw-1", instanceID: "inst-2"))
    }

    @Test func unauthorizedAndRevokedBothEndInUnauthorized() async {
        for code in ["unauthorized", "revoked"] {
            let transport = gateway(handshake: #"{"type":"error","code":"\#(code)","message":"no"}"#)
            let client = LiveRemoteClient(codec: WireCodec()) { transport }
            await #expect(throws: RemoteError.unauthorized) {
                try await client.connect(using: storedCredential())
            }
        }
    }

    @Test func aFoundationGatewayIsConnectedWithNothingShared() async throws {
        let transport = gateway(handshake: authenticated)
        let client = LiveRemoteClient(codec: WireCodec()) { transport }
        try await client.connect(using: storedCredential())
        let stream = try await client.subscribeSessions()
        var iterator = stream.makeAsyncIterator()
        #expect(try await iterator.next() == [])

        let request = try await sentJSON(transport, at: 1)
        #expect((request["operation"] as? [String: Any])?["type"] as? String == "subscribe_sessions")
        #expect(request["session_id"] == nil)

        // Revocation while connected ends the stream as unauthorized.
        await transport.push(#"{"type":"error","code":"revoked","message":"this device was revoked"}"#)
        await #expect(throws: RemoteError.unauthorized) {
            while try await iterator.next() != nil {}
        }
        #expect(await transport.isClosed)
    }

    @MainActor
    @Test func theStoreGoesFromPairingToConnectedAndBackToPairingWhenRevoked() async throws {
        let transports = TransportQueue([gateway(handshake: paired), gateway(handshake: authenticated)])
        let credentials = InMemoryCredentialStore()
        let store = RemoteStore(
            client: LiveRemoteClient(codec: WireCodec()) { transports.next() },
            credentials: credentials,
            sleep: { _ in },
            randomUnit: { 0.5 }
        )
        let qr = #"{"protocol_version":1,"address":"192.168.1.20:17879","gateway_id":"gw-1","credential":"c"}"#
        await store.pair(.scanned(qr), deviceName: "Phone")

        #expect(store.pairing == .idle)
        #expect(credentials.load()?.deviceToken == "device-token")
        #expect(credentials.load()?.host.device.id == "0011223344556677")
        #expect(credentials.load()?.host.gatewayID == "gw-1")
        #expect(await waitUntil { store.connection == .connected })
        #expect(store.sessions.isEmpty)

        await transports.last.push(#"{"type":"error","code":"revoked","message":"this device was revoked"}"#)
        #expect(await waitUntil { store.connection == .unauthorized })
        #expect(credentials.load() == nil)
        #expect(store.host == nil)
        if case .failed = store.pairing {} else { Issue.record("the pairing screen should say why") }
    }
}

/// Hands out prepared transports one connection at a time.
final class TransportQueue: Sendable {
    private let transports: [ScriptedTransport]
    private let index = Mutex(0)

    init(_ transports: [ScriptedTransport]) {
        self.transports = transports
    }

    var last: ScriptedTransport { transports[transports.count - 1] }

    func next() -> ScriptedTransport {
        let position = index.withLock { value in
            defer { value += 1 }
            return value
        }
        return transports[min(position, transports.count - 1)]
    }
}
