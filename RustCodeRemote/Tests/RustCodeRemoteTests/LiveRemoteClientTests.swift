import Foundation
import Testing
@testable import RustCodeRemote

/// The client's plumbing over a scripted socket, using the test-only `LineCodec`.
@Suite struct LiveRemoteClientTests {
    let ref = LineCodec.session

    private func makeClient(
        _ replies: [String: [String]],
        timeout: Duration = .seconds(5)
    ) -> (LiveRemoteClient, ScriptedTransport) {
        let transport = ScriptedTransport(responder: scriptedResponder(replies))
        let client = LiveRemoteClient(codec: LineCodec(), requestTimeout: timeout) { transport }
        return (client, transport)
    }

    @Test func connectOpensTheEndpointAndAuthenticatesFirst() async throws {
        let (client, transport) = makeClient(["authenticate": ["authenticated"]])
        try await client.connect(using: makeCredential())

        #expect(await transport.connectedURL == URL(string: "ws://192.168.1.20:7878/test"))
        let sent = await transport.sent
        #expect(sent.count == 1)
        #expect(sent[0] == "handshake|authenticate")
        // The token travels in a frame the codec builds, never in the URL.
        #expect(await transport.connectedURL?.absoluteString.contains("token") == false)
    }

    @Test func aRejectedTokenThrowsUnauthorizedAndClosesTheSocket() async {
        let (client, transport) = makeClient(["authenticate": ["unauthorized"]])
        await #expect(throws: RemoteError.unauthorized) {
            try await client.connect(using: makeCredential())
        }
        #expect(await transport.isClosed)
    }

    @Test func anUnreachableHostSurfacesAsATransportError() async {
        let transport = ScriptedTransport(connectError: URLError(.cannotConnectToHost))
        let client = LiveRemoteClient(codec: LineCodec()) { transport }
        do {
            try await client.connect(using: makeCredential())
            Issue.record("connect should have failed")
        } catch {
            guard case RemoteError.transport = error else {
                Issue.record("unexpected error \(error)")
                return
            }
        }
    }

    @Test func requestsWithoutAConnectionFailImmediately() async {
        let (client, _) = makeClient([:])
        await #expect(throws: RemoteError.notConnected) {
            _ = try await client.subscribeSessions()
        }
    }

    @Test func pairingReturnsTheCredentialAndLeavesNoConnectionOpen() async throws {
        let (client, transport) = makeClient(["pair": ["paired"]])
        let address = HostAddress(host: "100.90.1.2", port: 7878)
        let credential = try await client.pair(.manual(address: address, code: "123456"), deviceName: "Phone")

        #expect(credential.deviceToken == "fresh-token")
        #expect(await transport.connectedURL == URL(string: "ws://100.90.1.2:7878/test"))
        #expect(await transport.isClosed)
    }

    @Test func sessionListDeliversTheInitialListThenPushedChanges() async throws {
        let (client, transport) = makeClient(["authenticate": ["authenticated"], "subscribe": ["sessions|2"]])
        try await client.connect(using: makeCredential())
        let stream = try await client.subscribeSessions()
        await transport.push("sessions|3")
        await transport.push("sessions|1")

        var counts: [Int] = []
        for try await list in stream {
            counts.append(list.count)
            if counts.count == 3 { break }
        }
        #expect(counts == [2, 3, 1])
    }

    @Test func attachReturnsTheSnapshotAndRoutesItsUpdates() async throws {
        let (client, transport) = makeClient([
            "authenticate": ["authenticated"],
            // The update is pushed right behind the reply, before the caller has resumed.
            "attach": ["attached", "!update|s1|11|Hel"],
        ])
        try await client.connect(using: makeCredential())
        let attachment = try await client.attach(ref, resumeAfter: nil)
        guard case .snapshot(var snapshot) = attachment.start else {
            Issue.record("expected a snapshot")
            return
        }
        #expect(snapshot.sequence == 10)

        await transport.push("update|s1|12|lo")
        await transport.push("update|other|1|ignored")
        var received = 0
        for try await update in attachment.updates {
            #expect(snapshot.apply(update) == .applied)
            received += 1
            if received == 2 { break }
        }
        #expect(snapshot.items.first?.content == .assistant(AssistantText(text: "Hello", isStreaming: true)))
    }

    @Test func afterAResyncNoticeOnlyTheSnapshotIsTrusted() async throws {
        let (client, transport) = makeClient(["authenticate": ["authenticated"], "attach": ["attached"]])
        try await client.connect(using: makeCredential())
        let attachment = try await client.attach(ref, resumeAfter: nil)
        await transport.push("update|s1|11|kept")
        await transport.push("resync|s1")
        await transport.push("update|s1|12|dropped")
        await transport.push("snapshot|s1|40")
        await transport.push("update|s1|41|after")

        var received: [SessionUpdate] = []
        for try await update in attachment.updates {
            received.append(update)
            if received.count == 3 { break }
        }
        #expect(received.map(\.sequence) == [11, 40, 41])
        #expect(received[1].isSnapshot)

        guard case .snapshot(var state) = attachment.start else { return }
        for update in received {
            #expect(state.apply(update) == .applied)
        }
        #expect(state.sequence == 41)
    }

    @Test func aMutationWaitsPastReceivedForTheOwnersAnswer() async throws {
        let (client, _) = makeClient([
            "authenticate": ["authenticated"],
            "submit": ["receipt|received", "receipt|applied"],
        ])
        try await client.connect(using: makeCredential())
        let id = RequestID("req-1")
        let receipt = try await client.submit("hello", to: ref, requestID: id)
        #expect(receipt == RequestReceipt(requestID: id, state: .applied))
    }

    @Test func aMutationKeepsItsRequestIDOnTheWire() async throws {
        let (client, transport) = makeClient(["authenticate": ["authenticated"], "queue": ["receipt|applied"]])
        try await client.connect(using: makeCredential())
        _ = try await client.queue("later", in: ref, requestID: RequestID("stable-id"))
        #expect(await transport.sent.last == "stable-id|queue")
    }

    @Test func receivedWithoutAFinalAnswerIsReportedAsReceived() async throws {
        let (client, _) = makeClient(
            ["authenticate": ["authenticated"], "steer": ["receipt|received"]],
            timeout: .milliseconds(80)
        )
        try await client.connect(using: makeCredential())
        let receipt = try await client.steer("faster", in: ref, requestID: RequestID("req-2"))
        #expect(receipt.state == .received)
    }

    @Test func noAnswerAtAllTimesOut() async throws {
        let transport = ScriptedTransport()
        let client = LiveRemoteClient(codec: LineCodec(), requestTimeout: .milliseconds(50)) { transport }
        await transport.setResponder(scriptedResponder(["authenticate": ["authenticated"]]))
        try await client.connect(using: makeCredential())
        await #expect(throws: RemoteError.timeout) {
            _ = try await client.cancel(turnID: "t1", in: ref, requestID: RequestID("req-3"))
        }
    }

    @Test func aRejectionBecomesARejectedReceipt() async throws {
        let (client, _) = makeClient(["authenticate": ["authenticated"], "submit": ["busy"]])
        try await client.connect(using: makeCredential())
        let receipt = try await client.submit("hello", to: ref, requestID: RequestID("req-4"))
        #expect(receipt.state == .rejected(.busy))
    }

    @Test func requestStatusReportsReceivedAsIs() async throws {
        let (client, transport) = makeClient(["authenticate": ["authenticated"], "status": ["receipt|received"]])
        try await client.connect(using: makeCredential())
        let receipt = try await client.getRequestStatus(RequestID("req-5"), in: ref)
        #expect(receipt == RequestReceipt(requestID: RequestID("req-5"), state: .received))
        #expect(await transport.sent.last?.hasSuffix("|status:req-5") == true)
    }

    @Test func aFrameOutsideTheSchemaDropsTheConnection() async throws {
        let (client, transport) = makeClient([
            "authenticate": ["authenticated"],
            "subscribe": ["sessions|1"],
            "submit": ["!garbage"],
        ])
        try await client.connect(using: makeCredential())
        let stream = try await client.subscribeSessions()

        await #expect(throws: RemoteError.invalidFrame("garbage")) {
            _ = try await client.submit("hello", to: ref, requestID: RequestID("req-6"))
        }
        #expect(await transport.isClosed)
        await #expect(throws: RemoteError.invalidFrame("garbage")) {
            for try await _ in stream {}
        }
        await #expect(throws: RemoteError.notConnected) {
            _ = try await client.getRequestStatus(RequestID("req-6"), in: ref)
        }
    }

    @Test func aDroppedSocketFailsPendingRequestsAndEndsStreams() async throws {
        let (client, transport) = makeClient(["authenticate": ["authenticated"], "attach": ["attached"]])
        try await client.connect(using: makeCredential())
        let attachment = try await client.attach(ref, resumeAfter: nil)

        async let pending = client.submit("hello", to: ref, requestID: RequestID("req-7"))
        while await transport.sent.count < 3 { await Task.yield() }
        await transport.drop(throwing: URLError(.networkConnectionLost))

        do {
            _ = try await pending
            Issue.record("the pending request should have failed")
        } catch {
            guard case RemoteError.transport = error else {
                Issue.record("unexpected error \(error)")
                return
            }
        }
        do {
            for try await _ in attachment.updates {}
            Issue.record("the update stream should have thrown")
        } catch {}
    }

    @Test func theHostClosingForRevocationSurfacesAsUnauthorized() async throws {
        let (client, transport) = makeClient(["authenticate": ["authenticated"], "subscribe": ["sessions|1"]])
        try await client.connect(using: makeCredential())
        let stream = try await client.subscribeSessions()
        await transport.push("closing|unauthorized")

        await #expect(throws: RemoteError.unauthorized) {
            for try await _ in stream {}
        }
        #expect(await transport.isClosed)
    }

    @Test func aHostWithAnotherIdentityIsNotTrusted() async {
        let (client, transport) = makeClient(["authenticate": ["authenticated|someone-else"]])
        await #expect(throws: RemoteError.hostIdentityChanged) {
            try await client.connect(using: makeCredential())
        }
        #expect(await transport.isClosed)
    }

    @Test func pairingRefusesAHostOtherThanTheOneTheCodeNamed() async {
        let (client, _) = makeClient(["pair": ["paired|impostor"]])
        do {
            _ = try await client.pair(.manual(address: HostAddress(host: "10.0.0.2", port: 17879), code: "QR"), deviceName: "Phone")
            Issue.record("pairing should have failed")
        } catch {
            guard case RemoteError.pairingFailed = error else {
                Issue.record("unexpected error \(error)")
                return
            }
        }
    }

    @Test func connectReportsTheGatewayInstance() async throws {
        let (client, _) = makeClient(["authenticate": ["authenticated"]])
        let identity = try await client.connect(using: makeCredential())
        #expect(identity == ConnectionIdentity(gatewayID: "gateway", instanceID: "instance-1"))
    }

    @Test func anErrorThatNamesNoRequestMeansTheConnectionIsClosing() async throws {
        let (client, transport) = makeClient(["authenticate": ["authenticated"], "history": ["!gateway-error|idle_timeout"]])
        try await client.connect(using: makeCredential())
        await #expect(throws: RemoteError.transport("idle_timeout")) {
            _ = try await client.getHistory(ref, before: HistoryCursor("c"), limit: 10)
        }
        #expect(await transport.isClosed)
    }

    @Test func refreshingDeliversTheNewListThroughTheSameStream() async throws {
        let transport = ScriptedTransport()
        let client = LiveRemoteClient(codec: LineCodec()) { transport }
        await transport.setResponder(scriptedResponder(["authenticate": ["authenticated"], "subscribe": ["sessions|1"]]))
        await #expect(throws: RemoteError.notConnected) { try await client.refreshSessions() }
        try await client.connect(using: makeCredential())
        let stream = try await client.subscribeSessions()
        var iterator = stream.makeAsyncIterator()
        #expect(try await iterator.next()?.count == 1)

        await transport.setResponder(scriptedResponder(["subscribe": ["sessions|3"]]))
        try await client.refreshSessions()
        #expect(try await iterator.next()?.count == 3)
        #expect(await transport.isClosed == false)
    }

    @Test func anErrorThatClosesTheConnectionFailsEverything() async throws {
        let (client, transport) = makeClient(["authenticate": ["authenticated"], "subscribe": ["sessions|1"]])
        try await client.connect(using: makeCredential())
        let stream = try await client.subscribeSessions()
        await transport.push("gateway-error|shutting_down")
        await #expect(throws: RemoteError.transport("shutting_down")) {
            for try await _ in stream {}
        }
        #expect(await transport.isClosed)
    }

    @Test func framesOfAnUnknownKindAreIgnored() async throws {
        let (client, transport) = makeClient([
            "authenticate": ["authenticated"],
            "subscribe": ["!future|whatever", "sessions|2"],
        ])
        try await client.connect(using: makeCredential())
        let stream = try await client.subscribeSessions()
        var iterator = stream.makeAsyncIterator()
        #expect(try await iterator.next()?.count == 2)
        #expect(await transport.isClosed == false)
    }

    @Test func anUnknownResultFailsOnlyThatRequest() async throws {
        let (client, transport) = makeClient(["authenticate": ["authenticated"], "history": ["mystery"]])
        try await client.connect(using: makeCredential())
        await #expect(throws: RemoteError.invalidFrame("unexpected reply")) {
            _ = try await client.getHistory(ref, before: HistoryCursor("c"), limit: 10)
        }
        #expect(await transport.isClosed == false)
    }

    @Test func aReplayNamesTheGatewayTheSessionWasAttachedUnder() async throws {
        let (client, transport) = makeClient([
            "authenticate": ["authenticated"],
            "attach": ["attached"],
            "attach:gw-1/inst-1@12": ["resumed"],
        ])
        try await client.connect(using: makeCredential())
        // Nothing to resume from yet: the first attach cannot name a gateway.
        _ = try await client.attach(ref, resumeAfter: 5)
        let second = try await client.attach(ref, resumeAfter: 12)
        #expect(second.start == .resumed)
        let sent = await transport.sent
        #expect(sent[1].hasSuffix("|attach"))
        #expect(sent[2].hasSuffix("|attach:gw-1/inst-1@12"))
    }

    @Test func anUnknownOutcomeIsAnUnknownReceiptNotAnError() async throws {
        let (client, _) = makeClient(["authenticate": ["authenticated"], "submit": ["receipt-unknown"]])
        try await client.connect(using: makeCredential())
        let receipt = try await client.submit("hello", to: ref, requestID: RequestID("req-9"))
        #expect(receipt.state == .unknown)
    }

    @Test func contentIsFetchedInChunks() async throws {
        let (client, transport) = makeClient(["authenticate": ["authenticated"], "content": ["content|hello"]])
        try await client.connect(using: makeCredential())
        let chunk = try await client.getContent("c", in: ref, offset: 0, maxBytes: 1024)
        #expect(chunk.text == "hello")
        #expect(await transport.sent.last?.hasSuffix("|content") == true)
    }

    @Test func thePlaceholderCodecRefusesEverything() async {
        let client = LiveRemoteClient(codec: UnavailableRemoteCodec()) { ScriptedTransport() }
        await #expect(throws: RemoteError.codecUnavailable) {
            try await client.connect(using: makeCredential())
        }
        await #expect(throws: RemoteError.codecUnavailable) {
            _ = try await client.pair(.scanned("anything"), deviceName: "Phone")
        }
    }
}

@Suite struct ScriptedTransportTests {
    @Test func recordsSentFramesAndPlaysBackResponses() async throws {
        let transport = ScriptedTransport { frame in ["echo:\(frame)"] }
        try await transport.connect(to: URL(string: "ws://127.0.0.1:1")!, headers: ["X-Test": "1"])
        try await transport.send("one")
        await transport.push("pushed")
        await transport.close()

        var frames: [String] = []
        for try await frame in transport.incoming { frames.append(frame) }
        #expect(frames == ["echo:one", "pushed"])
        #expect(await transport.sent == ["one"])
        #expect(await transport.connectedHeaders == ["X-Test": "1"])
    }

    @Test func sendingBeforeConnectingOrAfterClosingFails() async throws {
        let transport = ScriptedTransport()
        await #expect(throws: RemoteError.notConnected) { try await transport.send("early") }
        try await transport.connect(to: URL(string: "ws://127.0.0.1:1")!, headers: [:])
        await transport.close()
        await #expect(throws: RemoteError.notConnected) { try await transport.send("late") }
    }

    @Test func inMemoryCredentialStoreRoundTrips() {
        let store = InMemoryCredentialStore()
        #expect(store.load() == nil)
        store.save(makeCredential())
        #expect(store.load() == makeCredential())
        store.delete()
        #expect(store.load() == nil)
    }
}
