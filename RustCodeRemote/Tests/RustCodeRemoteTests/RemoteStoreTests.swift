import Foundation
import Synchronization
import Testing
@testable import RustCodeRemote

/// A client whose behaviour each test scripts directly.
private actor StubClient: RemoteClient {
    var creationDelay: Duration = .zero
    func set(creationDelay: Duration) { self.creationDelay = creationDelay }
    private(set) var creationCalls: [RequestID] = []
    var creationResults: [Result<SessionSummary, RemoteError>] = []
    func set(creationResults: [Result<SessionSummary, RemoteError>]) { self.creationResults = creationResults }
    func createSession(directory: String?, requestID: RequestID) async throws -> SessionSummary {
        creationCalls.append(requestID)
        let result = creationResults.removeFirst()
        try await Task.sleep(for: creationDelay)
        return try result.get()
    }
    var connectResults: [Result<Void, RemoteError>] = []
    var submitResult: Result<ReceiptState, RemoteError> = .success(.applied)
    var answerResult: Result<ReceiptState, RemoteError> = .success(.applied)
    var statusResult: Result<ReceiptState, RemoteError> = .success(.unknown)
    var sessions: [SessionSummary] = [makeSession("s1")]
    /// Sequences of the updates each successive attach delivers after its snapshot.
    var attachScripts: [[UInt64]] = []
    /// Instance IDs reported by successive connects; the last one repeats.
    var instanceIDs: [String] = ["instance-1"]
    var snapshot: SessionSnapshot?
    var replayOnResume: [SessionUpdate] = []
    func set(replayOnResume: [SessionUpdate]) { self.replayOnResume = replayOnResume }
    var contentChunks: [String] = []
    private(set) var contentCalls: [UInt64] = []

    private(set) var connectCalls = 0
    private var receiptContinuation: AsyncStream<RequestReceipt>.Continuation?
    var immediateFinal: ReceiptState?
    func set(immediateFinal: ReceiptState) { self.immediateFinal = immediateFinal }
    func receiptUpdates() async -> AsyncStream<RequestReceipt> {
        let (stream, continuation) = AsyncStream.makeStream(of: RequestReceipt.self)
        receiptContinuation = continuation
        return stream
    }
    func emit(_ receipt: RequestReceipt) { receiptContinuation?.yield(receipt) }
    func emit(_ update: SessionUpdate) { updates?.yield(update) }
    private(set) var questionCancelCalls = 0
    func cancelQuestion(_ questionID: String, in session: SessionRef, requestID: RequestID) async throws -> RequestReceipt {
        questionCancelCalls += 1
        return RequestReceipt(requestID: requestID, state: .unknown)
    }
    private(set) var settingsCalls = 0
    func setSessionSettings(model: String, reasoningEffort: String, in session: SessionRef, requestID: RequestID) async throws -> RequestReceipt {
        settingsCalls += 1
        return RequestReceipt(requestID: requestID, state: .unknown)
    }
    func publishSnapshot(_ snapshot: SessionSnapshot) {
        updates?.yield(SessionUpdate(session: snapshot.session, sequence: snapshot.sequence, event: .replaced(snapshot)))
    }
    private(set) var queueTexts: [String] = []
    private(set) var cancelledTurnIDs: [String] = []
    private(set) var cancelCalls: [RequestID] = []
    private(set) var submitCalls: [RequestID] = []
    private(set) var submittedTexts: [String] = []
    private(set) var uploadCalls: [(ImageUploadChunk, RequestID)] = []
    var uploadFailure: RemoteError?
    var uploadDelay: Duration = .zero
    func set(uploadFailure: RemoteError?) { self.uploadFailure = uploadFailure }
    func set(uploadDelay: Duration) { self.uploadDelay = uploadDelay }
    func uploadImage(_ chunk: ImageUploadChunk, in session: SessionRef, requestID: RequestID) async throws -> ImageUploadProgress {
        uploadCalls.append((chunk, requestID))
        try await Task.sleep(for: uploadDelay)
        if let uploadFailure { throw uploadFailure }
        let next = chunk.offset + UInt64(Data(base64Encoded: chunk.dataBase64)!.count)
        return ImageUploadProgress(attachmentID: chunk.attachmentID, nextOffset: next, totalBytes: chunk.totalBytes,
            complete: next == chunk.totalBytes,
            promptReference: next == chunk.totalBytes ? "![image](file:///private/upload/\(chunk.attachmentID).png)" : nil)
    }
    private(set) var statusCalls: [RequestID] = []
    private(set) var attachCalls: [UInt64?] = []
    private(set) var disconnectCalls = 0
    private var listContinuation: AsyncThrowingStream<[SessionSummary], any Error>.Continuation?
    private var updates: AsyncThrowingStream<SessionUpdate, any Error>.Continuation?

    func set(connectResults: [Result<Void, RemoteError>]) { self.connectResults = connectResults }
    func set(submitResult: Result<ReceiptState, RemoteError>) { self.submitResult = submitResult }
    func set(answerResult: Result<ReceiptState, RemoteError>) { self.answerResult = answerResult }
    func set(statusResult: Result<ReceiptState, RemoteError>) { self.statusResult = statusResult }
    func set(attachScripts: [[UInt64]]) { self.attachScripts = attachScripts }
    func set(instanceIDs: [String]) { self.instanceIDs = instanceIDs }
    func set(snapshot: SessionSnapshot) { self.snapshot = snapshot }
    func set(contentChunks: [String]) { self.contentChunks = contentChunks }

    func closeSession(_ reason: String) {
        updates?.finish(throwing: RemoteError.sessionClosed(reason))
    }

    func publish(_ sessions: [SessionSummary]) {
        self.sessions = sessions
        listContinuation?.yield(sessions)
    }

    func dropConnection(_ error: RemoteError) {
        listContinuation?.finish(throwing: error)
        listContinuation = nil
    }

    func pair(_ request: PairingRequest, deviceName: String) async throws -> StoredCredential {
        if case .scanned(let text) = request {
            throw text == "locked" ? RemoteError.rateLimited(retryAfter: .seconds(240)) : RemoteError.pairingFailed("expired")
        }
        return makeCredential()
    }

    @discardableResult
    func connect(using credential: StoredCredential) async throws -> ConnectionIdentity {
        connectCalls += 1
        dialed.append(credential.host.address)
        if !connectResults.isEmpty {
            try connectResults.removeFirst().get()
        }
        let instance = instanceIDs.count > 1 ? instanceIDs.removeFirst() : instanceIDs[0]
        return ConnectionIdentity(gatewayID: "gateway", instanceID: instance, hostName: hostName, linkExpiresAt: linkExpiresAt)
    }

    func getContent(_ contentID: String, in session: SessionRef, offset: UInt64, maxBytes: Int) async throws -> ContentChunk {
        contentCalls.append(offset)
        guard !contentChunks.isEmpty else { throw RemoteError.timeout }
        let text = contentChunks.removeFirst()
        let next = contentChunks.isEmpty ? nil : offset + UInt64(text.utf8.count)
        return ContentChunk(contentID: contentID, offset: offset, text: text, totalBytes: 0, nextOffset: next)
    }

    func disconnect() async {
        disconnectCalls += 1
        listContinuation?.finish()
        listContinuation = nil
    }

    func subscribeSessions() async throws -> AsyncThrowingStream<[SessionSummary], any Error> {
        let (stream, continuation) = AsyncThrowingStream.makeStream(of: [SessionSummary].self)
        listContinuation = continuation
        continuation.yield(sessions)
        return stream
    }

    private(set) var refreshCalls = 0
    var hostName: String?
    func set(hostName: String) { self.hostName = hostName }
    private(set) var dialed: [HostAddress] = []
    var linkExpiresAt: Date?
    func set(linkExpiresAt: Date) { self.linkExpiresAt = linkExpiresAt }

    func refreshSessions() async throws {
        refreshCalls += 1
        listContinuation?.yield(sessions)
    }

    func attach(_ session: SessionRef, resumeAfter: UInt64?) async throws -> SessionAttachment {
        attachCalls.append(resumeAfter)
        let (stream, continuation) = AsyncThrowingStream.makeStream(of: SessionUpdate.self)
        let script = attachScripts.isEmpty ? [] : attachScripts.removeFirst()
        for sequence in script {
            continuation.yield(update(session, sequence, .assistantDelta(itemID: "a", text: "\(sequence) ")))
        }
        updates = continuation
        if resumeAfter != nil, !replayOnResume.isEmpty {
            for update in replayOnResume { continuation.yield(update) }
            return SessionAttachment(start: .resumed, updates: stream)
        }
        return SessionAttachment(
            start: .snapshot(snapshot ?? SessionSnapshot(session: session, sequence: 10)),
            updates: stream
        )
    }

    func detach(_ session: SessionRef) async {}

    func getHistory(_ session: SessionRef, before cursor: HistoryCursor, limit: Int) async throws -> HistoryPage {
        HistoryPage(items: [], nextCursor: nil)
    }

    func submit(_ text: String, to session: SessionRef, requestID: RequestID) async throws -> RequestReceipt {
        submitCalls.append(requestID)
        submittedTexts.append(text)
        if let immediateFinal {
            receiptContinuation?.yield(RequestReceipt(requestID: requestID, state: immediateFinal))
            try await Task.sleep(for: .milliseconds(15))
            throw RemoteError.timeout
        }
        return RequestReceipt(requestID: requestID, state: try submitResult.get())
    }

    func steer(_ text: String, in session: SessionRef, requestID: RequestID) async throws -> RequestReceipt {
        RequestReceipt(requestID: requestID, state: .applied)
    }

    func queue(_ text: String, in session: SessionRef, requestID: RequestID) async throws -> RequestReceipt {
        queueTexts.append(text)
        return RequestReceipt(requestID: requestID, state: .unknown)
    }

    func cancel(turnID: String, in session: SessionRef, requestID: RequestID) async throws -> RequestReceipt {
        cancelCalls.append(requestID)
        cancelledTurnIDs.append(turnID)
        return RequestReceipt(requestID: requestID, state: .unknown)
    }

    func answerQuestion(_ questionID: String, with answer: QuestionAnswer, in session: SessionRef, requestID: RequestID) async throws -> RequestReceipt {
        RequestReceipt(requestID: requestID, state: try answerResult.get())
    }

    func resolveApproval(batchID: String, decision: ApprovalDecision, in session: SessionRef, requestID: RequestID) async throws -> RequestReceipt {
        RequestReceipt(requestID: requestID, state: .applied)
    }

    func getRequestStatus(_ requestID: RequestID, in session: SessionRef) async throws -> RequestReceipt {
        statusCalls.append(requestID)
        return RequestReceipt(requestID: requestID, state: try statusResult.get())
    }
}

/// Records the delays the store asked to sleep for, instead of sleeping.
private final class DelayLog: Sendable {
    private let storage = Mutex<[Duration]>([])

    var values: [Duration] { storage.withLock { $0 } }

    func append(_ delay: Duration) {
        storage.withLock { $0.append(delay) }
    }
}

@MainActor
@Suite struct RemoteStoreTests {
    private let delays = DelayLog()

    @Test func liveFeedbackIsEmittedOnceWithoutSnapshotOrTextFeedback() async {
        let client = StubClient()
        let store = makeStore(client)
        store.bootstrap()
        #expect(await waitUntil { store.connection == .connected && !store.sessions.isEmpty })
        store.select("s1")
        #expect(await waitUntil { store.attached?.sequence == 10 })
        #expect(store.chatFeedback == nil)
        guard let ref = store.attached?.session else { Issue.record("Session did not attach"); return }
        await client.emit(SessionUpdate(session: ref, sequence: 11, event: .turnStarted(turnID: "t")))
        #expect(await waitUntil { store.chatFeedback?.kind == .started })
        let firstRevision = store.chatFeedback?.revision
        await client.emit(SessionUpdate(session: ref, sequence: 12, event: .responseDelta("Hello")))
        #expect(await waitUntil { store.attached?.sequence == 12 })
        #expect(store.chatFeedback?.revision == firstRevision)
        let finish = SessionUpdate(session: ref, sequence: 13, event: .turnFinished(turnID: "t", outcome: .completed))
        await client.emit(finish)
        #expect(await waitUntil { store.chatFeedback?.kind == .completed })
        let completedFeedback = store.chatFeedback
        await client.emit(finish)
        await client.publishSnapshot(SessionSnapshot(session: ref, sequence: 20))
        #expect(await waitUntil { store.attached?.sequence == 20 })
        #expect(store.chatFeedback == completedFeedback)
        await store.unpair()
    }

    @Test func reconnectSkipsHistoricalFeedbackAndStillReportsNewLiveEvents() async {
        let client = StubClient()
        let ref = makeSession("s1").ref
        await client.set(snapshot: SessionSnapshot(session: ref, activeTurnID: "old", sequence: 10))
        let store = makeStore(client)
        store.bootstrap()
        #expect(await waitUntil { store.connection == .connected && !store.sessions.isEmpty })
        store.select("s1")
        #expect(await waitUntil { store.attached?.sequence == 10 })
        await store.suspend()
        await client.set(snapshot: SessionSnapshot(session: ref, sequence: 11,
            lastTurn: TurnTiming(turnID: "old", outcome: "completed")))
        await client.set(replayOnResume: [SessionUpdate(session: ref, sequence: 11,
            event: .turnFinished(turnID: "old", outcome: .completed))])
        store.resume()
        #expect(await waitUntil { store.attached?.sequence == 11 })
        #expect(store.chatFeedback == nil)
        await client.emit(SessionUpdate(session: ref, sequence: 12, event: .turnStarted(turnID: "new")))
        #expect(await waitUntil { store.chatFeedback?.kind == .started })
        await client.emit(SessionUpdate(session: ref, sequence: 13, event: .turnFinished(turnID: "new", outcome: .completed)))
        #expect(await waitUntil { store.chatFeedback?.kind == .completed })
        await store.unpair()
    }

    private func makeStore(
        _ client: StubClient,
        credentials: InMemoryCredentialStore = InMemoryCredentialStore(makeCredential()),
        backoff: BackoffPolicy = BackoffPolicy(maximumAttempts: 3),
        demoPace: Duration = .zero,
        locateHost: @escaping @Sendable (String) async -> HostAddress? = { _ in nil },
        now: @escaping @Sendable () -> Date = { Date() }
    ) -> RemoteStore {
        RemoteStore(
            client: client,
            demoClient: { DemoRemoteClient(pace: demoPace) },
            credentials: credentials,
            locateHost: locateHost,
            now: now,
            backoff: backoff,
            sleep: { [delays] delay in delays.append(delay) },
            randomUnit: { 0.5 }
        )
    }

    @Test func oldCreationCannotSelectSessionAfterRepairingSameGateway() async {
        let client = StubClient()
        await client.set(creationDelay: .milliseconds(100))
        await client.set(creationResults: [.success(makeSession("old-created"))])
        let store = makeStore(client)
        store.bootstrap()
        #expect(await waitUntil { store.connection == .connected })
        let pending = Task { await store.createSession(directory: "/work") }
        #expect(await waitUntil { await client.creationCalls.count == 1 })
        await store.unpair()
        await store.pair(.manual(address: HostAddress(host: "localhost", port: 7878), code: "123456"), deviceName: "Test")
        #expect(await waitUntil { store.connection == .connected })
        #expect(await pending.value == false)
        #expect(store.selectedSessionID == nil)
        #expect(!store.sessions.contains { $0.id == "old-created" })
        #expect(!store.isCreatingSession)
        await store.unpair()
    }

    @Test func uncertainCreationRetriesSameIdentityAndSelectsConfirmedSession() async {
        let client = StubClient()
        let hostMessage = "session is starting; retry with the same request identifier to check it"
        await client.set(creationResults: [.failure(.outcomeUnknown(detail: hostMessage)), .success(makeSession("new"))])
        let store = makeStore(client)
        store.bootstrap()
        #expect(await waitUntil { store.connection == .connected })
        #expect(await store.createSession(directory: "/work") == false)
        // The request stays visible with the host's own words, and nothing is in flight.
        let pending = store.pendingCreation
        #expect(pending?.directory == "/work")
        #expect(pending?.hostMessage == hostMessage)
        #expect(store.notice?.contains(hostMessage) == true)
        #expect(!store.isCreatingSession)
        #expect(await store.createSession(directory: "/work"))
        let requests = await client.creationCalls
        #expect(requests.count == 2)
        #expect(requests[0] == requests[1])
        #expect(requests[0] == pending?.requestID)
        #expect(store.pendingCreation == nil)
        #expect(store.selectedSessionID == "new")
        #expect(await waitUntil { store.attached?.session.sessionID == "new" })
        await store.unpair()
    }

    @Test func unconfirmedCreationDoesNotBlockAnotherFolder() async {
        let client = StubClient()
        await client.set(creationResults: [.failure(.timeout), .success(makeSession("other"))])
        let store = makeStore(client)
        store.bootstrap()
        #expect(await waitUntil { store.connection == .connected })
        #expect(await store.createSession(directory: "/work") == false)
        #expect(store.pendingCreation?.hostMessage == RemoteError.timeout.errorDescription)
        // A different folder abandons the unconfirmed request and gets its own identity.
        #expect(await store.createSession(directory: "/different"))
        let requests = await client.creationCalls
        #expect(requests.count == 2)
        #expect(requests[0] != requests[1])
        #expect(store.pendingCreation == nil)
        #expect(store.selectedSessionID == "other")
        await store.unpair()
    }

    @Test func cancellingAnUnconfirmedCreationStartsOverWithANewIdentity() async {
        let client = StubClient()
        await client.set(creationResults: [.failure(.timeout), .success(makeSession("again"))])
        let store = makeStore(client)
        store.bootstrap()
        #expect(await waitUntil { store.connection == .connected })
        #expect(await store.createSession(directory: "/work") == false)
        store.cancelPendingCreation()
        #expect(store.pendingCreation == nil)
        #expect(await store.createSession(directory: "/work"))
        let requests = await client.creationCalls
        #expect(requests[0] != requests[1])
        await store.unpair()
    }

    @Test func anotherFolderCanStartWhileACreationIsStillInFlight() async {
        let client = StubClient()
        await client.set(creationDelay: .milliseconds(150))
        await client.set(creationResults: [.success(makeSession("slow")), .success(makeSession("chosen"))])
        let store = makeStore(client)
        store.bootstrap()
        #expect(await waitUntil { store.connection == .connected })
        let first = Task { await store.createSession(directory: "/slow") }
        #expect(await waitUntil { await client.creationCalls.count == 1 })
        #expect(store.isCreatingSession)
        // The same folder again is only debounced; it sends nothing.
        #expect(await store.createSession(directory: "/slow") == false)
        #expect(await client.creationCalls.count == 1)
        #expect(await store.createSession(directory: "/chosen"))
        // The abandoned request's late answer neither selects its session nor clears state.
        #expect(await first.value == false)
        #expect(store.selectedSessionID == "chosen")
        #expect(!store.isCreatingSession)
        #expect(store.pendingCreation == nil)
        await store.unpair()
    }

    @Test func cancellingAnInFlightCreationIgnoresItsLateAnswer() async {
        let client = StubClient()
        await client.set(creationDelay: .milliseconds(100))
        await client.set(creationResults: [.success(makeSession("late"))])
        let store = makeStore(client)
        store.bootstrap()
        #expect(await waitUntil { store.connection == .connected })
        let selected = store.selectedSessionID
        let pending = Task { await store.createSession(directory: "/work") }
        #expect(await waitUntil { await client.creationCalls.count == 1 })
        store.cancelPendingCreation()
        #expect(!store.isCreatingSession)
        #expect(await pending.value == false)
        #expect(store.selectedSessionID == selected)
        await store.unpair()
    }

    @Test func rejectedCreationAllowsChoosingAnotherFolder() async {
        let client = StubClient()
        await client.set(creationResults: [.failure(.rejected(.unsupportedOperation)), .success(makeSession("new"))])
        let store = makeStore(client)
        store.bootstrap()
        #expect(await waitUntil { store.connection == .connected })
        #expect(await store.createSession(directory: "/missing") == false)
        #expect(await store.createSession(directory: "/exists"))
        let requests = await client.creationCalls
        #expect(requests[0] != requests[1])
        await store.unpair()
    }

    // MARK: Pairing and connection

    @Test func bootstrapWithoutACredentialStaysUnpaired() {
        let store = makeStore(StubClient(), credentials: InMemoryCredentialStore())
        store.bootstrap()
        #expect(store.connection == .unpaired)
        #expect(store.host == nil)
    }

    @Test func bootstrapConnectsWithTheStoredCredential() async {
        let store = makeStore(StubClient())
        store.bootstrap()
        #expect(store.host?.name == "Test Mac")
        #expect(await waitUntil { store.connection == .connected && store.sessions.count == 1 })
    }

    @Test func pairingStoresTheCredentialAndConnects() async {
        let credentials = InMemoryCredentialStore()
        let store = makeStore(StubClient(), credentials: credentials)
        await store.pair(.manual(address: HostAddress(host: "192.168.1.20", port: 7878), code: "123456"), deviceName: "Phone")

        #expect(store.pairing == .idle)
        #expect(credentials.load() == makeCredential())
        #expect(await waitUntil { store.connection == .connected })
    }

    @Test func aFailedPairingReportsWhyAndStoresNothing() async {
        let credentials = InMemoryCredentialStore()
        let store = makeStore(StubClient(), credentials: credentials)
        await store.pair(.scanned("stale"), deviceName: "Phone")

        #expect(store.pairing == .failed("Pairing failed: expired"))
        #expect(credentials.load() == nil)
        #expect(store.connection == .unpaired)
        store.clearPairingError()
        #expect(store.pairing == .idle)
    }

    @Test func transientFailuresBackOffThenConnect() async {
        let client = StubClient()
        await client.set(connectResults: [.failure(.timeout), .failure(.connectionClosed), .success(())])
        let store = makeStore(client)
        store.bootstrap()

        #expect(await waitUntil { store.connection == .connected })
        #expect(delays.values == [.seconds(1), .seconds(2)])
        #expect(await client.connectCalls == 3)
    }

    @Test func givesUpAfterTheAttemptLimitAndManualRetryStartsOver() async {
        let client = StubClient()
        await client.set(connectResults: Array(repeating: .failure(.timeout), count: 4))
        let store = makeStore(client)
        store.bootstrap()

        #expect(await waitUntil { store.connection == .offline(reason: RemoteError.timeout.localizedDescription) })
        #expect(await client.connectCalls == 4)
        #expect(delays.values == [.seconds(1), .seconds(2), .seconds(4)])
        #expect(store.canRetry)

        store.retryNow()
        #expect(await waitUntil { store.connection == .connected })
        #expect(await client.connectCalls == 5)
    }

    @Test func unauthorizedStopsReconnectingAndRemovesTheToken() async {
        let client = StubClient()
        await client.set(connectResults: [.failure(.unauthorized)])
        let credentials = InMemoryCredentialStore(makeCredential())
        let store = makeStore(client, credentials: credentials)
        store.bootstrap()

        #expect(await waitUntil { store.connection == .unauthorized })
        #expect(credentials.load() == nil)
        #expect(store.host == nil)
        #expect(!store.canRetry)
        store.retryNow()
        try? await Task.sleep(for: .milliseconds(30))
        #expect(await client.connectCalls == 1)
        #expect(delays.values.isEmpty)
    }

    @Test func aHostThatMovedIsDialedAtItsNewAddressWithoutPairingAgain() async {
        let client = StubClient()
        await client.set(connectResults: [.failure(.transport("unreachable")), .success(())])
        let credentials = InMemoryCredentialStore(makeCredential())
        let moved = HostAddress(host: "192.168.1.77", port: 7878)
        let store = makeStore(client, credentials: credentials, locateHost: { $0 == "gateway" ? moved : nil })
        store.bootstrap()

        #expect(await waitUntil { store.connection == .connected })
        #expect(await client.dialed == [makeCredential().host.address, moved])
        #expect(store.host?.address == moved)
        #expect(credentials.load()?.host.address == moved)
        #expect(credentials.load()?.deviceToken == "token")
        // Found at once, so there was nothing to wait for.
        #expect(delays.values.isEmpty)
    }

    @Test func aHostFoundAtTheSameAddressBacksOffAsBefore() async {
        let client = StubClient()
        await client.set(connectResults: [.failure(.timeout), .success(())])
        let store = makeStore(client, locateHost: { _ in makeCredential().host.address })
        store.bootstrap()

        #expect(await waitUntil { store.connection == .connected })
        #expect(delays.values == [.seconds(1)])
        #expect(await client.connectCalls == 2)
    }

    @Test func theLinkExpiryTheHostReportsIsStoredAndNamedWhenItRunsOut() async {
        let client = StubClient()
        let expiry = Date(timeIntervalSince1970: 1_000)
        await client.set(linkExpiresAt: expiry)
        let credentials = InMemoryCredentialStore(makeCredential())
        let store = makeStore(client, credentials: credentials, now: { expiry.addingTimeInterval(1) })
        store.bootstrap()
        #expect(await waitUntil { store.connection == .connected })
        #expect(store.host?.expiresAt == expiry)
        #expect(credentials.load()?.host.expiresAt == expiry)

        await client.dropConnection(.unauthorized)
        #expect(await waitUntil { store.connection == .unauthorized })
        #expect(store.pairing == .failed(RemoteError.linkExpired.localizedDescription))
        #expect(credentials.load() == nil)
    }

    @Test func revocationWhileConnectedAlsoStops() async {
        let client = StubClient()
        let credentials = InMemoryCredentialStore(makeCredential())
        let store = makeStore(client, credentials: credentials)
        store.bootstrap()
        #expect(await waitUntil { store.connection == .connected })

        await client.dropConnection(.unauthorized)
        #expect(await waitUntil { store.connection == .unauthorized })
        #expect(credentials.load() == nil)
        #expect(store.sessions.isEmpty)
    }

    @Test func aMissingCodecIsNotRetried() async {
        let client = StubClient()
        await client.set(connectResults: [.failure(.codecUnavailable)])
        let store = makeStore(client)
        store.bootstrap()

        #expect(await waitUntil { store.connection == .offline(reason: RemoteError.codecUnavailable.localizedDescription) })
        #expect(await client.connectCalls == 1)
    }

    @Test func suspendClosesTheSocketAndResumeResyncsTheSelectedSession() async {
        let client = StubClient()
        let store = makeStore(client)
        store.bootstrap()
        #expect(await waitUntil { store.connection == .connected })
        store.select("s1")
        #expect(await waitUntil { store.attached != nil })

        await store.suspend()
        #expect(store.connection == .suspended)
        #expect(await client.disconnectCalls == 1)

        store.resume()
        #expect(await waitUntil { store.connection == .connected })
        #expect(await waitUntil { await client.attachCalls.count == 2 })
        // A fresh baseline separates historical activity from new live feedback.
        #expect(await client.attachCalls == [nil, nil])
    }

    @Test func unpairingRemovesTheTokenAndCachedHostData() async {
        let credentials = InMemoryCredentialStore(makeCredential())
        let store = makeStore(StubClient(), credentials: credentials)
        store.bootstrap()
        #expect(await waitUntil { store.connection == .connected })
        store.select("s1")
        #expect(await waitUntil { store.attached != nil })

        await store.unpair()
        #expect(credentials.load() == nil)
        #expect(store.host == nil)
        #expect(store.sessions.isEmpty)
        #expect(store.attached == nil)
        #expect(store.connection == .unpaired)
    }

    // MARK: Sessions

    @Test func aSequenceGapForcesAFreshSnapshot() async {
        let client = StubClient()
        await client.set(attachScripts: [[11, 13], [11]])
        let store = makeStore(client)
        store.bootstrap()
        #expect(await waitUntil { store.connection == .connected })
        store.select("s1")

        #expect(await waitUntil { await client.attachCalls.count == 2 && store.attached?.sequence == 11 })
        // The second attach asks for a snapshot, not a replay from a cursor it cannot trust.
        #expect(await client.attachCalls == [nil, nil])
        #expect(store.attached?.items.first?.content == .assistant(AssistantText(text: "11 ", isStreaming: true)))
    }

    @Test func aSessionThatStopsBeingSharedIsDeselected() async {
        let client = StubClient()
        let store = makeStore(client)
        store.bootstrap()
        #expect(await waitUntil { store.connection == .connected })
        store.select("s1")
        #expect(await waitUntil { store.attached != nil })

        await client.publish([makeSession("s2")])
        #expect(await waitUntil { store.selectedSessionID == nil })
        #expect(store.attached == nil)
        #expect(store.notice != nil)
    }

    @Test func aNewRegistrationEpochIsAttachedAfresh() async {
        let client = StubClient()
        let store = makeStore(client)
        store.bootstrap()
        #expect(await waitUntil { store.connection == .connected })
        store.select("s1")
        #expect(await waitUntil { store.attached?.session.registrationEpoch == 1 })

        await client.publish([makeSession("s1", epoch: 2)])
        #expect(await waitUntil { store.attached?.session.registrationEpoch == 2 })
        #expect(await client.attachCalls == [nil, nil])
    }

    @Test func completionBadgesAppearForSessionsInTheBackground() async {
        let client = StubClient()
        let store = makeStore(client)
        store.bootstrap()
        #expect(await waitUntil { store.connection == .connected })

        await client.publish([makeSession("s1", activity: .running)])
        await client.publish([makeSession("s1")])
        #expect(await waitUntil { store.badges.unseenCompletions == ["s1"] })
        store.select("s1")
        #expect(store.badges.unseenCompletions.isEmpty)
    }

    @Test func aGatewayRestartAsksForASnapshotInsteadOfAReplay() async {
        let client = StubClient()
        await client.set(instanceIDs: ["instance-1", "instance-2"])
        let store = makeStore(client)
        store.bootstrap()
        #expect(await waitUntil { store.connection == .connected })
        store.select("s1")
        #expect(await waitUntil { store.attached != nil })

        await client.dropConnection(.connectionClosed)
        #expect(await waitUntil { await client.attachCalls.count == 2 })
        // Same session and epoch, but the sequence numbers came from the previous instance.
        #expect(await client.attachCalls == [nil, nil])
    }

    @Test func aClosedSessionIsDeselectedWithItsReason() async {
        let client = StubClient()
        let store = makeStore(client)
        store.bootstrap()
        #expect(await waitUntil { store.connection == .connected })
        store.select("s1")
        #expect(await waitUntil { store.attached != nil })

        await client.closeSession("Sharing was turned off for this session.")
        #expect(await waitUntil { store.selectedSessionID == nil })
        #expect(store.attached == nil)
        #expect(store.notice == "Sharing was turned off for this session.")
    }

    @Test func refreshingAsksForTheListAgainWithoutReconnecting() async {
        let client = StubClient()
        let store = makeStore(client)
        store.bootstrap()
        #expect(await waitUntil { store.connection == .connected })

        await store.refreshSessions()
        #expect(await client.refreshCalls == 1)
        #expect(await client.connectCalls == 1)
        #expect(store.connection == .connected)
    }

    @Test func theNewestSessionIsListedFirst() async {
        let client = StubClient()
        let store = makeStore(client)
        store.bootstrap()
        #expect(await waitUntil { store.connection == .connected })

        // The host lists sessions oldest first.
        await client.publish([makeSession("s1"), makeSession("s2"), makeSession("s3")])
        #expect(await waitUntil { store.sessions.map(\.id) == ["s3", "s2", "s1"] })
    }

    @Test func detachingStopsFollowingTheSession() async {
        let client = StubClient()
        let store = makeStore(client)
        store.bootstrap()
        #expect(await waitUntil { store.connection == .connected })
        store.select("s1")
        #expect(await waitUntil { store.attached != nil })

        store.detach()
        #expect(store.selectedSessionID == nil)
        #expect(store.attached == nil)
        #expect(store.sessions.count == 1)
    }

    @Test func theHostsReportedNameReplacesTheAddressAndIsStored() async {
        let client = StubClient()
        await client.set(hostName: "studio")
        let credentials = InMemoryCredentialStore(makeCredential())
        let store = makeStore(client, credentials: credentials)
        store.bootstrap()
        #expect(store.host?.name == "Test Mac")
        #expect(await waitUntil { store.host?.name == "studio" })
        #expect(credentials.load()?.host.name == "studio")
        #expect(credentials.load()?.deviceToken == "token")
    }

    // MARK: Approval details

    private func truncatedApproval(contentID: String? = "content-1", totalBytes: UInt64 = 20) -> PendingApproval {
        PendingApproval(batchID: "b1", items: [
            .init(id: "b1#0", toolName: "run", title: "Run", detail: "complete"),
            .init(
                id: "b1#1",
                toolName: "write",
                title: "Write",
                detail: "first",
                isDetailComplete: false,
                detailContentID: contentID,
                detailTotalBytes: totalBytes
            ),
        ])
    }

    private func storeAttached(to approval: PendingApproval, client: StubClient) async -> RemoteStore {
        let ref = SessionRef(sessionID: "s1", registrationEpoch: 1)
        await client.set(snapshot: SessionSnapshot(session: ref, pendingApproval: approval, sequence: 10))
        let store = makeStore(client)
        store.bootstrap()
        _ = await waitUntil { store.connection == .connected }
        store.select("s1")
        _ = await waitUntil { store.attached != nil }
        return store
    }

    @Test func approvingUnlocksOnlyAfterTheFullDetailIsFetched() async {
        let client = StubClient()
        await client.set(contentChunks: ["first ", "second ", "third"])
        let approval = truncatedApproval()
        let store = await storeAttached(to: approval, client: client)

        #expect(await store.resolve(approval, decision: .approve) == false)
        #expect(store.notice != nil)

        await store.loadApprovalDetails()
        let item = store.attached?.pendingApproval?.items[1]
        #expect(item?.detail == "first second third")
        #expect(item?.isDetailComplete == true)
        // Fetched from the start, following the host's offsets.
        #expect(await client.contentCalls == [0, 6, 13])
        #expect(await store.resolve(approval, decision: .approve))
    }

    @Test func aFailedFetchKeepsApprovingLockedButDenyingWorks() async {
        let client = StubClient()
        let approval = truncatedApproval()
        let store = await storeAttached(to: approval, client: client)

        await store.loadApprovalDetails()
        #expect(store.attached?.pendingApproval?.canBeResolved == false)
        #expect(store.isLoadingApprovalDetails == false)
        #expect(await store.resolve(approval, decision: .approve) == false)
        #expect(await store.resolve(approval, decision: .deny))
    }

    @Test func detailThatCannotBeFetchedOrIsTooLargeStaysLocked() async {
        for approval in [
            truncatedApproval(contentID: nil),
            truncatedApproval(totalBytes: RemoteStore.maximumApprovalDetailBytes + 1),
        ] {
            let client = StubClient()
            await client.set(contentChunks: ["everything"])
            let store = await storeAttached(to: approval, client: client)
            await store.loadApprovalDetails()
            #expect(store.attached?.pendingApproval?.canBeResolved == false)
            #expect(await client.contentCalls.isEmpty)
        }
    }

    @Test func aLockedOutHostReportsWhenPairingCanBeTriedAgain() async {
        let store = makeStore(StubClient(), credentials: InMemoryCredentialStore())
        await store.pair(.scanned("locked"), deviceName: "Phone")
        #expect(store.pairing == .failed("Too many failed attempts. Try again in 4 minutes."))
    }

    @Test func aRateLimitedReconnectWaitsForTheHostsDeadline() async {
        let client = StubClient()
        await client.set(connectResults: [.failure(.rateLimited(retryAfter: .seconds(240))), .success(())])
        let store = makeStore(client)
        store.bootstrap()
        #expect(await waitUntil { store.connection == .connected })
        #expect(delays.values == [.seconds(240)])
    }

    // MARK: Receipts

    @Test func aLostReplyLeavesTheRequestUncertainAndIsNeverResent() async {
        let client = StubClient()
        await client.set(submitResult: .failure(.timeout))
        let store = makeStore(client)
        store.bootstrap()
        #expect(await waitUntil { store.connection == .connected })
        store.select("s1")
        #expect(await waitUntil { store.attached != nil })

        #expect(await store.send("deploy it"))
        #expect(store.visibleRequests.count == 1)
        #expect(!store.hasPendingSettings)
        #expect(store.visibleRequests.first?.phase == .settled(.unknown))
        let id = store.visibleRequests[0].id

        // Still unknown to the host: stays visible.
        await store.checkStatus(of: id)
        #expect(store.visibleRequests.count == 1)

        await client.set(statusResult: .success(.applied))
        await store.checkStatus(of: id)
        #expect(store.visibleRequests.isEmpty)
        #expect(await client.submitCalls == [id])
        #expect(await client.statusCalls == [id, id])
    }

    @Test func uncertainRequestsAreQueriedAfterReconnect() async {
        let client = StubClient()
        await client.set(submitResult: .failure(.connectionClosed))
        let store = makeStore(client)
        store.bootstrap()
        #expect(await waitUntil { store.connection == .connected })
        store.select("s1")
        #expect(await waitUntil { store.attached != nil })
        await store.send("deploy it")
        let id = store.visibleRequests[0].id

        await client.set(statusResult: .success(.applied))
        await client.dropConnection(.connectionClosed)
        #expect(await waitUntil { store.receipts.requests.isEmpty })
        #expect(await client.submitCalls == [id])
        #expect(await client.statusCalls == [id])
    }

    @Test func aRejectionIsReportedAndTheTextIsHandedBack() async {
        let client = StubClient()
        await client.set(submitResult: .success(.rejected(.busy)))
        let store = makeStore(client)
        store.bootstrap()
        #expect(await waitUntil { store.connection == .connected })
        store.select("s1")
        #expect(await waitUntil { store.attached != nil })

        #expect(await store.send("hello") == false)
        #expect(store.notice == RemoteRejection.busy.message)
        #expect(store.visibleRequests.isEmpty)
    }

    @Test func nothingIsSentWithoutAnAttachedSessionOrText() async {
        let client = StubClient()
        let store = makeStore(client)
        store.bootstrap()
        #expect(await waitUntil { store.connection == .connected })
        #expect(await store.send("hello") == false)
        store.select("s1")
        #expect(await waitUntil { store.attached != nil })
        #expect(await store.send("   ") == false)
        #expect(await client.submitCalls.isEmpty)
    }

    @Test func controlsRequireHostMetadataAndUseAuthoritativeSnapshotUpdates() async {
        let legacy = makeStore(StubClient())
        legacy.bootstrap()
        #expect(await waitUntil { legacy.connection == .connected })
        legacy.select("s1")
        #expect(await waitUntil { legacy.attached != nil })
        #expect(await legacy.setSessionSettings(model: "invented", reasoningEffort: "high") == false)

        let store = makeStore(StubClient())
        await store.startDemo(deviceName: "Phone")
        #expect(await waitUntil { store.sessions.count == 3 })
        store.select(DemoRemoteClient.idleSessionID)
        #expect(await waitUntil { store.attached?.settings != nil })
        let total = store.selectedSession?.turnCount
        #expect(total != nil)
        await store.loadEarlierHistory()
        #expect(store.selectedSession?.turnCount == total)
        #expect(await store.setSessionSettings(model: "demo-fast", reasoningEffort: "high") == false)
        #expect(await store.setSessionSettings(model: "demo-fast", reasoningEffort: "default"))
        #expect(await waitUntil { store.attached?.settings?.selectedModel == "demo-fast" })
        #expect(store.attached?.settings?.reasoningEffort == "default")
        #expect(store.visibleRequests.isEmpty)
    }

    @Test func answeringDistinguishesDefiniteFailureFromUncertainDelivery() async {
        let question = PendingQuestion(id: "question", prompt: "Choose", options: [.init(id: "A", label: "A")])
        let client = StubClient()
        await client.set(snapshot: SessionSnapshot(session: LineCodec.session, pendingQuestion: question))
        let store = makeStore(client)
        store.bootstrap()
        #expect(await waitUntil { store.connection == .connected })
        store.select("s1")
        #expect(await waitUntil { store.attached?.pendingQuestion != nil })
        let answer = QuestionAnswer(selectedOptionIDs: ["A"])

        await client.set(answerResult: .success(.rejected(.invalidAnswer)))
        #expect(await store.answer(question, with: answer) == false)
        #expect(store.attached?.pendingQuestion == question)
        #expect(store.visibleRequests.isEmpty)
        #expect(store.notice == RemoteRejection.invalidAnswer.message)

        await client.set(answerResult: .failure(.notConnected))
        #expect(await store.answer(question, with: answer) == false)
        #expect(store.attached?.pendingQuestion == question)

        await client.set(answerResult: .failure(.timeout))
        #expect(await store.answer(question, with: answer))
        #expect(store.attached?.pendingQuestion == question)
        #expect(store.visibleRequests.last?.isUncertain == true)
    }

    // MARK: Demo host

    @Test func demoListsItsSessionsWithoutTouchingTheCredentialStore() async {
        let credentials = InMemoryCredentialStore()
        let store = makeStore(StubClient(), credentials: credentials)
        await store.startDemo(deviceName: "Phone")

        #expect(store.isDemo)
        #expect(await waitUntil { store.connection == .connected && store.sessions.count == 3 })
        #expect(credentials.load() == nil)
        #expect(store.badges.badge(for: store.sessions[1]) == .question)
    }

    @Test func demoStreamsATurnThenAsksForApprovalAndRunsItWhenApproved() async {
        let store = makeStore(StubClient())
        await store.startDemo(deviceName: "Phone")
        #expect(await waitUntil { store.sessions.count == 3 })
        store.select(DemoRemoteClient.approvalSessionID)

        #expect(await waitUntil { store.attached?.pendingApproval != nil })
        let approval = store.attached!.pendingApproval!
        #expect(store.attached?.isRunning == true)
        #expect(await waitUntil { store.selectedSession?.attention.pendingApprovals == 1 })
        let tools = store.attached!.items.compactMap { item -> ToolCall? in
            if case .tool(let tool) = item.content { tool } else { nil }
        }
        #expect(tools.map(\.status) == [.succeeded, .succeeded])

        // The demo host sends the action as a preview: approving is locked until it is whole.
        #expect(!approval.canBeResolved)
        #expect(await store.resolve(approval, decision: .approve) == false)
        #expect(store.attached?.pendingApproval != nil)
        await store.loadApprovalDetails()
        #expect(store.attached?.pendingApproval?.canBeResolved == true)
        #expect(store.attached?.pendingApproval?.items.first?.detail.hasSuffix("no network") == true)

        let reviewed = store.attached!.pendingApproval!
        #expect(await store.resolve(reviewed, decision: .approve))
        #expect(await waitUntil { store.attached?.isRunning == false })
        #expect(store.attached?.pendingApproval == nil)
        #expect(store.attached?.items.contains { item in
            if case .tool(let tool) = item.content { tool.name == "run_command" && tool.status == .succeeded } else { false }
        } == true)
        #expect(store.receipts.requests.isEmpty)

        // The batch is gone: resolving it again is stale.
        #expect(await store.resolve(reviewed, decision: .approve) == false)
        #expect(store.notice == RemoteRejection.staleApproval.message)
    }

    @Test func demoAnswersAQuestionAndFinishesTheTurn() async {
        let store = makeStore(StubClient())
        await store.startDemo(deviceName: "Phone")
        #expect(await waitUntil { store.sessions.count == 3 })
        store.select(DemoRemoteClient.questionSessionID)
        #expect(await waitUntil { store.attached?.pendingQuestion != nil })
        let question = store.attached!.pendingQuestion!

        #expect(await store.answer(question, with: QuestionAnswer()) == false)
        #expect(await store.answer(question, with: QuestionAnswer(selectedOptionIDs: ["segments"])))
        #expect(await waitUntil { store.attached?.isRunning == false })
        #expect(store.attached?.pendingQuestion == nil)
        #expect(store.attached?.items.contains(TranscriptItem(id: "user-live-1", .user("Segment files"))) == true)
        #expect(await waitUntil { store.selectedSession?.activity == .idle })
    }

    @Test func demoSubmitsWhenIdleQueuesWhileRunningAndPagesHistory() async {
        // Paced, so the running turn stays observable long enough to queue behind it.
        let store = makeStore(StubClient(), demoPace: .milliseconds(3))
        await store.startDemo(deviceName: "Phone")
        #expect(await waitUntil { store.sessions.count == 3 })
        store.select(DemoRemoteClient.idleSessionID)
        #expect(await waitUntil { store.attached != nil })

        await store.loadEarlierHistory()
        #expect(store.attached?.items.first?.id == "notes-1")
        #expect(store.attached?.historyCursor == nil)

        #expect(await store.send("first"))
        // Until the turn's first update arrives the store still sees an idle session, and
        // the host would answer a second submit with `busy`.
        #expect(await waitUntil { store.attached?.isRunning == true })
        #expect(await store.send("second"))
        #expect(await waitUntil {
            store.attached?.isRunning == false
                && store.attached?.items.filter { $0.content == .user("second") }.count == 1
        })
        let users = store.attached!.items.compactMap { item -> String? in
            if case .user(let text) = item.content { text } else { nil }
        }
        #expect(users.suffix(2) == ["first", "second"])
        #expect(store.attached?.items.contains { $0.content == .notice("Queued: second") } == true)
    }

    @Test func oldHostQuestionCancelTargetsOnlyTheObservedTurn() async {
        let client = StubClient()
        let question = PendingQuestion(id: "q1", prompt: "First?", options: [], allowsFreeText: true)
        await client.set(snapshot: SessionSnapshot(session: makeSession("s1").ref, activeTurnID: "turn-old", pendingQuestion: question))
        let store = makeStore(client)
        store.bootstrap()
        #expect(await waitUntil { store.connection == .connected })
        store.select("s1")
        #expect(await waitUntil { store.attached != nil })
        #expect(await store.cancelQuestion(question))
        #expect(store.isQuestionCancellationPending(question.id))
        #expect(await store.cancelTurn() == false)
        await client.publishSnapshot(SessionSnapshot(session: makeSession("s1").ref, activeTurnID: "turn-new"))
        #expect(await waitUntil { store.attached?.activeTurnID == "turn-new" })
        #expect(await store.cancelQuestion(question) == false)
        #expect(await client.cancelledTurnIDs == ["turn-old"])
        #expect(await client.questionCancelCalls == 0)
    }

    @Test func uncertainAnswerCanBeCancelledWithoutDuplicatingTheCancellation() async {
        let client = StubClient()
        let question = PendingQuestion(id: "q1", prompt: "First?", options: [], allowsFreeText: true)
        await client.set(snapshot: SessionSnapshot(session: makeSession("s1").ref, activeTurnID: "turn-1", pendingQuestion: question, capabilities: ["cancel_question"]))
        await client.set(answerResult: .success(.unknown))
        let store = makeStore(client)
        store.bootstrap()
        #expect(await waitUntil { store.connection == .connected })
        store.select("s1")
        #expect(await waitUntil { store.attached != nil })
        #expect(await store.answer(question, with: QuestionAnswer(text: "one")))
        #expect(store.isQuestionPending(question.id))
        #expect(!store.isQuestionCancellationPending(question.id))
        #expect(await store.cancelQuestion(question))
        #expect(store.isQuestionCancellationPending(question.id))
        #expect(await store.cancelQuestion(question) == false)
        #expect(await store.cancelTurn() == false)
        #expect(await store.answer(question, with: QuestionAnswer(text: "two")) == false)
        #expect(await client.questionCancelCalls == 1)
    }

    @Test func confirmedSettingsSnapshotReleasesGuardWithoutInventingReceiptSuccess() async {
        let client = StubClient()
        let settings = SessionSettings(models: [.init(id: "m", model: "m", reasoningEfforts: ["low", "high"])], selectedModel: "m", reasoningEffort: "low", canChange: true)
        var snapshot = SessionSnapshot(session: makeSession("s1").ref, settings: settings)
        await client.set(snapshot: snapshot)
        let store = makeStore(client)
        store.bootstrap()
        #expect(await waitUntil { store.connection == .connected })
        store.select("s1")
        #expect(await waitUntil { store.attached != nil })
        #expect(await store.setSessionSettings(model: "m", reasoningEffort: "high"))
        #expect(store.hasPendingSettings)
        #expect(await store.setSessionSettings(model: "m", reasoningEffort: "low") == false)
        snapshot.settings?.reasoningEffort = "high"
        await client.publishSnapshot(snapshot)
        #expect(await waitUntil { store.attached?.settings?.reasoningEffort == "high" })
        #expect(!store.hasPendingSettings)
        #expect(store.visibleRequests.first?.phase == .settled(.unknown))
        #expect(await store.setSessionSettings(model: "m", reasoningEffort: "low"))
        #expect(await client.settingsCalls == 2)
    }

    @Test func finalRejectionArrivingBeforeMutationTimeoutRestoresDraft() async {
        let client = StubClient()
        await client.set(immediateFinal: .rejected(.busy))
        let store = makeStore(client)
        store.bootstrap()
        #expect(await waitUntil { store.connection == .connected })
        store.select("s1")
        #expect(await waitUntil { store.attached != nil })
        #expect(await store.send("draft") == false)
        #expect(store.visibleRequests.isEmpty)
        #expect(store.notice == RemoteRejection.busy.message)
    }

    @Test func distinctQueuedMessagesRemainAllowedWhileDuplicateIsUncertain() async {
        let client = StubClient()
        await client.set(snapshot: SessionSnapshot(session: makeSession("s1").ref, activeTurnID: "turn-1"))
        let store = makeStore(client)
        store.bootstrap()
        #expect(await waitUntil { store.connection == .connected })
        store.select("s1")
        #expect(await waitUntil { store.attached != nil })
        #expect(await store.send("one"))
        #expect(await store.send("one") == false)
        #expect(await store.send("two"))
        #expect(await client.queueTexts == ["one", "two"])
    }

    @Test func newQuestionIsNotBlockedByPreviousUnknownAnswer() async throws {
        let client = StubClient()
        let first = PendingQuestion(id: "q1", prompt: "First?", options: [], allowsFreeText: true)
        let second = PendingQuestion(id: "q2", prompt: "Second?", options: [], allowsFreeText: true)
        await client.set(snapshot: SessionSnapshot(session: makeSession("s1").ref, activeTurnID: "turn-1", pendingQuestion: first))
        await client.set(answerResult: .success(.unknown))
        let store = makeStore(client)
        store.bootstrap()
        #expect(await waitUntil { store.connection == .connected })
        store.select("s1")
        #expect(await waitUntil { store.attached != nil })
        #expect(await store.answer(first, with: QuestionAnswer(text: "one")))
        #expect(store.isQuestionPending(first.id))
        #expect(!store.isQuestionPending(second.id))
        #expect(await store.answer(second, with: QuestionAnswer(text: "two")))
    }

    @Test func uncertainCancellationCannotBeDuplicatedEvenAfterDismissal() async {
        let client = StubClient()
        await client.set(snapshot: SessionSnapshot(session: makeSession("s1").ref, activeTurnID: "turn-1"))
        let store = makeStore(client)
        store.bootstrap()
        #expect(await waitUntil { store.connection == .connected })
        store.select("s1")
        #expect(await waitUntil { store.attached != nil })
        await store.cancelTurn()
        let request = store.visibleRequests.first!
        store.dismissRequest(request.id)
        for _ in 0..<5 { await store.cancelTurn() }
        #expect(await client.cancelCalls.count == 1)
        #expect(store.hasPendingCancellation)
        await client.set(statusResult: .success(.applied))
        await store.checkStatus(of: request.id)
        #expect(!store.hasPendingCancellation)
        #expect(store.visibleRequests.isEmpty)
    }

    @Test func imagesUploadInBoundedChunksBeforeSubmittingActualReferences() async {
        let client = StubClient()
        var snapshot = SessionSnapshot(session: makeSession("s1").ref)
        snapshot.capabilities = ["upload_image"]
        await client.set(snapshot: snapshot)
        let store = makeStore(client)
        store.bootstrap()
        #expect(await waitUntil { store.connection == .connected })
        store.select("s1")
        #expect(await waitUntil { store.attached != nil })
        let image = PromptImage(data: Data(repeating: 42, count: PromptImage.chunkBytes + 7), mimeType: "image/png")
        #expect(await store.send("Describe", images: [image]))
        let calls = await client.uploadCalls
        #expect(calls.map { $0.0.offset } == [0, UInt64(PromptImage.chunkBytes)])
        #expect(calls.map { Data(base64Encoded: $0.0.dataBase64)!.count } == [PromptImage.chunkBytes, 7])
        #expect(await client.submittedTexts == ["Describe\n\n![image](file:///private/upload/\(image.id).png)"])
        #expect(!store.isUploadingImages)
    }

    @Test func failedUploadPreservesPromptAndExplicitRetryReusesChunkIdentity() async {
        let client = StubClient()
        var snapshot = SessionSnapshot(session: makeSession("s1").ref)
        snapshot.capabilities = ["upload_image"]
        await client.set(snapshot: snapshot)
        await client.set(uploadFailure: .timeout)
        let store = makeStore(client)
        store.bootstrap()
        #expect(await waitUntil { store.connection == .connected })
        store.select("s1")
        #expect(await waitUntil { store.attached != nil })
        let image = PromptImage(data: Data([1, 2, 3]), mimeType: "image/png")
        #expect(await store.send("draft", images: [image]) == false)
        #expect(await client.submitCalls.isEmpty)
        #expect(store.notice?.contains("has not been sent") == true)
        await client.set(uploadFailure: nil)
        #expect(await store.send("draft", images: [image]))
        let calls = await client.uploadCalls
        #expect(calls.count == 2)
        #expect(calls[0].1 == calls[1].1)
        #expect(await client.submitCalls.count == 1)
    }

    @Test func changingSessionDuringImageUploadCannotSubmitToAnotherChat() async {
        let client = StubClient()
        var snapshot = SessionSnapshot(session: makeSession("s1").ref)
        snapshot.capabilities = ["upload_image"]
        await client.set(snapshot: snapshot)
        await client.set(uploadDelay: .milliseconds(100))
        let store = makeStore(client)
        store.bootstrap()
        #expect(await waitUntil { store.connection == .connected })
        store.select("s1")
        #expect(await waitUntil { store.attached != nil })
        let task = Task { await store.send("draft", images: [PromptImage(data: Data([1]), mimeType: "image/png")]) }
        #expect(await waitUntil { store.isUploadingImages })
        store.select(nil)
        #expect(await task.value == false)
        #expect(await client.submitCalls.isEmpty)
    }

    @Test func rePairingSameHostDuringUploadCannotSendOldDraft() async {
        let client = StubClient()
        var snapshot = SessionSnapshot(session: makeSession("s1").ref)
        snapshot.capabilities = ["upload_image"]
        await client.set(snapshot: snapshot)
        await client.set(uploadDelay: .milliseconds(100))
        let store = makeStore(client)
        store.bootstrap()
        #expect(await waitUntil { store.connection == .connected })
        store.select("s1")
        #expect(await waitUntil { store.attached != nil })
        let pending = Task { await store.send("old draft", images: [PromptImage(data: Data([1]), mimeType: "image/png")]) }
        #expect(await waitUntil { await client.uploadCalls.count == 1 })
        await store.unpair()
        await store.pair(.manual(address: HostAddress(host: "localhost", port: 7878), code: "123456"), deviceName: "Test")
        #expect(await waitUntil { store.connection == .connected })
        store.select("s1")
        #expect(await waitUntil { store.attached != nil })
        #expect(await pending.value == false)
        #expect(await client.submitCalls.isEmpty)
        #expect(!store.isUploadingImages)
        await store.unpair()
    }

    @Test func unsupportedHostAndOversizedImagesNeverUploadOrSubmit() async {
        let client = StubClient()
        let store = makeStore(client)
        store.bootstrap()
        #expect(await waitUntil { store.connection == .connected })
        store.select("s1")
        #expect(await waitUntil { store.attached != nil })
        #expect(await store.send("draft", images: [PromptImage(data: Data([1]), mimeType: "image/png")]) == false)
        #expect(await client.uploadCalls.isEmpty)
        var snapshot = SessionSnapshot(session: makeSession("s1").ref, sequence: 11)
        snapshot.capabilities = ["upload_image"]
        await client.publishSnapshot(snapshot)
        #expect(await waitUntil { store.supportsImageUpload })
        #expect(await store.send("draft", images: [PromptImage(data: Data(repeating: 0, count: PromptImage.maximumBytes + 1), mimeType: "image/png")]) == false)
        #expect(await client.uploadCalls.isEmpty)
        #expect(await client.submitCalls.isEmpty)
    }

    @Test func demoCancelsTheObservedTurn() async {
        let store = makeStore(StubClient())
        await store.startDemo(deviceName: "Phone")
        #expect(await waitUntil { store.sessions.count == 3 })
        store.select(DemoRemoteClient.questionSessionID)
        #expect(await waitUntil { store.attached?.isRunning == true })

        await store.cancelTurn()
        #expect(await waitUntil { store.attached?.isRunning == false })
        #expect(store.attached?.pendingQuestion == nil)
    }
}
