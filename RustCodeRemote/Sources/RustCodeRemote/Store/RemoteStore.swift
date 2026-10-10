import Foundation
import Observation

public enum ConnectionState: Sendable, Equatable {
    case unpaired
    case connecting
    case connected
    /// The last attempt failed; another one starts after `delay`.
    case waitingToRetry(attempt: Int, delay: Duration, reason: String)
    /// Automatic retries are used up, or retrying cannot help. Waits for a manual retry.
    case offline(reason: String)
    /// The app is in the background; the socket is closed on purpose.
    case suspended
    /// The host revoked this device. Cleared only by pairing again.
    case unauthorized
}

public enum PairingStatus: Sendable, Equatable {
    case idle
    case pairing
    case failed(String)
}

public enum SendMode: Sendable, Equatable {
    /// Submit when idle, queue while a turn is running.
    case automatic
    case steer
    case queue
}

/// The app's single source of truth for everything remote. Views read it and call its
/// methods; it never exposes the client, the transport or anything wire-shaped.
@MainActor
@Observable
public final class RemoteStore {
    public private(set) var connection: ConnectionState = .unpaired
    public private(set) var pairing: PairingStatus = .idle
    public private(set) var host: PairedHost?
    public private(set) var isDemo = false

    public private(set) var sessions: [SessionSummary] = []
    public private(set) var selectedSessionID: String?
    /// Snapshot of the selected session, kept current by applying its updates.
    public private(set) var attached: SessionSnapshot?
    public private(set) var isLoadingHistory = false
    /// True while the full detail of the pending approval is being fetched.
    public private(set) var isLoadingApprovalDetails = false

    public private(set) var badges = AttentionBadges()
    public private(set) var receipts = ReceiptLedger()

    /// A message for the user about the last thing that went wrong. Views clear it once shown.
    public private(set) var commandResult: SessionCommandResult?
    public func dismissCommandResult() { commandResult = nil }
    public func isApprovalPending(_ batchID: String) -> Bool {
        guard let session = attached?.session else { return false }
        return isPending("approval:\(batchID)", in: session)
    }
    public var notice: String?
    private struct PendingMutation {
        var session: SessionRef
        var key: String
        var blocksDuplicate = true
        var desiredModel: String?
        var desiredEffort: String?
    }
    private var pendingMutations: [RequestID: PendingMutation] = [:]
    @ObservationIgnored private var performing: Set<RequestID> = []
    @ObservationIgnored private var finalReceipts: [RequestID: RequestReceipt] = [:]
    @ObservationIgnored private var receiptTask: Task<Void, Never>?

    public var hasPendingSettings: Bool {
        guard let session = attached?.session else { return false }
        return isPending("settings", in: session)
    }

    public var hasPendingCancellation: Bool {
        guard let snapshot = attached, let turn = snapshot.activeTurnID else { return false }
        return isPending("cancel:\(turn)", in: snapshot.session)
    }

    public func isQuestionPending(_ questionID: String) -> Bool {
        guard let session = attached?.session else { return false }
        return isPending("question:\(questionID)", in: session) || isQuestionCancellationPending(questionID)
    }

    public func isQuestionCancellationPending(_ questionID: String) -> Bool {
        guard let session = attached?.session else { return false }
        return isPending("cancel-question:\(questionID)", in: session)
            || (attached?.pendingQuestion?.id == questionID && hasPendingCancellation)
    }

    private func isPending(_ key: String, in session: SessionRef) -> Bool {
        pendingMutations.values.contains { $0.session == session && $0.key == key && $0.blocksDuplicate }
    }

    @ObservationIgnored private let liveClient: any RemoteClient
    @ObservationIgnored private let makeDemoClient: @Sendable () -> any RemoteClient
    @ObservationIgnored private let credentials: any CredentialStore
    @ObservationIgnored private let backoff: BackoffPolicy
    @ObservationIgnored private let sleep: @Sendable (Duration) async throws -> Void
    @ObservationIgnored private let randomUnit: @Sendable () -> Double

    @ObservationIgnored private var client: any RemoteClient
    @ObservationIgnored private var credential: StoredCredential?
    @ObservationIgnored private var connectionTask: Task<Void, Never>?
    @ObservationIgnored private var attachTask: Task<Void, Never>?
    /// The gateway instance the attached snapshot came from. Cursors do not survive a restart.
    @ObservationIgnored private var snapshotInstanceID: String?
    @ObservationIgnored private var currentInstanceID: String?

    /// Above this size the detail of an approval is not pulled onto the phone; the action
    /// has to be reviewed in the terminal.
    public static let maximumApprovalDetailBytes: UInt64 = 2 * 1024 * 1024
    /// The host serves content in chunks of at most 64 KiB.
    private static let contentChunkBytes = 64 * 1024

    public init(
        client: any RemoteClient,
        demoClient: @escaping @Sendable () -> any RemoteClient = { DemoRemoteClient() },
        credentials: any CredentialStore,
        backoff: BackoffPolicy = .standard,
        sleep: @escaping @Sendable (Duration) async throws -> Void = { try await Task.sleep(for: $0) },
        randomUnit: @escaping @Sendable () -> Double = { Double.random(in: 0...1) }
    ) {
        self.liveClient = client
        self.client = client
        self.makeDemoClient = demoClient
        self.credentials = credentials
        self.backoff = backoff
        self.sleep = sleep
        self.randomUnit = randomUnit
    }

    // MARK: Derived state

    public var selectedSession: SessionSummary? {
        sessions.first { $0.id == selectedSessionID }
    }

    /// Requests for the attached session that the user still has to know about.
    public var visibleRequests: [TrackedRequest] {
        guard let session = attached?.session else { return [] }
        return receipts.requests(for: session)
    }

    public var canRetry: Bool {
        switch connection {
        case .offline, .waitingToRetry: true
        default: false
        }
    }

    // MARK: Pairing

    /// Loads the stored pairing, if any, and connects.
    public func bootstrap() {
        guard host == nil else { return }
        do {
            guard let stored = try credentials.load() else { return }
            adopt(stored, client: liveClient, demo: false)
        } catch {
            notice = error.localizedDescription
        }
    }

    public func pair(_ request: PairingRequest, deviceName: String) async {
        guard pairing != .pairing else { return }
        pairing = .pairing
        do {
            let stored = try await liveClient.pair(request, deviceName: deviceName)
            try credentials.save(stored)
            pairing = .idle
            adopt(stored, client: liveClient, demo: false)
        } catch {
            pairing = .failed(error.localizedDescription)
        }
    }

    /// Connects to a simulated host. Nothing is written to the credential store.
    public func startDemo(deviceName: String) async {
        let demo = makeDemoClient()
        do {
            let stored = try await demo.pair(.scanned("demo"), deviceName: deviceName)
            pairing = .idle
            adopt(stored, client: demo, demo: true)
        } catch {
            pairing = .failed(error.localizedDescription)
        }
    }

    public func clearPairingError() {
        if case .failed = pairing { pairing = .idle }
    }

    /// Forgets the host: closes the connection and removes the token and all cached host data.
    public func unpair() async {
        connectionTask?.cancel()
        attachTask?.cancel()
        await client.disconnect()
        if !isDemo {
            do {
                try credentials.delete()
            } catch {
                notice = error.localizedDescription
            }
        }
        resetHostState()
        connection = .unpaired
        pairing = .idle
    }

    // MARK: Connection

    /// Tries again now, restarting the backoff. Does nothing while unauthorized.
    public func retryNow() {
        guard canRetry else { return }
        startConnection()
    }

    /// Call when the app goes to the background. No background execution is promised.
    public func suspend() async {
        switch connection {
        case .unpaired, .unauthorized, .suspended: return
        default: break
        }
        connectionTask?.cancel()
        attachTask?.cancel()
        receipts.connectionLost()
        connection = .suspended
        await client.disconnect()
    }

    /// Call when the app returns to the foreground: reconnects and resyncs the list and the
    /// selected session. Uncertain requests are queried, never replayed.
    public func resume() {
        guard connection == .suspended else { return }
        startConnection()
    }

    // MARK: Sessions

    /// Asks the host for the session list again. The connection stays as it is.
    public func refreshSessions() async {
        guard connection == .connected else {
            retryNow()
            return
        }
        do {
            try await client.refreshSessions()
        } catch {
            // A dead connection is noticed and retried by the connection loop.
        }
    }

    /// Stops following the attached session. The session itself is not affected.
    public func detach() {
        select(nil)
    }

    public func select(_ sessionID: String?) {
        guard sessionID != selectedSessionID else { return }
        attachTask?.cancel()
        if let previous = attached?.session {
            let client = client
            Task { await client.detach(previous) }
        }
        attached = nil
        commandResult = nil
        selectedSessionID = sessionID
        if let sessionID {
            badges.markSeen(sessionID)
            startAttachment(sessionID)
        }
    }

    public func loadEarlierHistory() async {
        guard let snapshot = attached, let cursor = snapshot.historyCursor, !isLoadingHistory else { return }
        isLoadingHistory = true
        defer { isLoadingHistory = false }
        do {
            let page = try await client.getHistory(snapshot.session, before: cursor, limit: 50)
            if attached?.session == snapshot.session, attached?.historyCursor == cursor {
                attached?.prepend(page)
            }
        } catch {
            notice = error.localizedDescription
        }
    }

    /// Fetches the full detail of every action in the pending approval that arrived only as
    /// a preview. Approving stays locked until this has succeeded: a preview is not the action.
    public func loadApprovalDetails() async {
        guard !isLoadingApprovalDetails, let snapshot = attached, let approval = snapshot.pendingApproval,
              !approval.canBeResolved else { return }
        isLoadingApprovalDetails = true
        defer { isLoadingApprovalDetails = false }
        let session = snapshot.session

        for item in approval.items where !item.isDetailComplete {
            guard let contentID = item.detailContentID else { continue }
            if let total = item.detailTotalBytes, total > Self.maximumApprovalDetailBytes { continue }
            do {
                var text = ""
                var offset: UInt64 = 0
                while true {
                    let chunk = try await client.getContent(
                        contentID,
                        in: session,
                        offset: offset,
                        maxBytes: Self.contentChunkBytes
                    )
                    text += chunk.text
                    guard UInt64(text.utf8.count) <= Self.maximumApprovalDetailBytes else { return }
                    // A host that does not advance would loop forever; treat it as incomplete.
                    guard let next = chunk.nextOffset else { break }
                    guard next > offset else { return }
                    offset = next
                }
                // Only if the same batch is still the one waiting.
                guard attached?.session == session, attached?.pendingApproval?.batchID == approval.batchID,
                      let index = attached?.pendingApproval?.items.firstIndex(where: { $0.id == item.id })
                else { return }
                attached?.pendingApproval?.items[index].detail = text
                attached?.pendingApproval?.items[index].isDetailComplete = true
            } catch {
                notice = error.localizedDescription
                return
            }
        }
    }

    // MARK: Mutations

    /// Sends text to the attached session.
    /// - Returns: false when the text certainly did not reach the session, so the caller can
    ///   give it back to the user. True does not mean applied: the outcome may be uncertain.
    @discardableResult
    public func send(_ text: String, mode: SendMode = .automatic) async -> Bool {
        let text = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, let snapshot = attached else { return false }
        if text.hasPrefix("/") { return await executeCommand(text) }
        let session = snapshot.session
        switch mode {
        case .automatic where !snapshot.isRunning:
            return await perform(.submit(text), in: session) { client, id in
                try await client.submit(text, to: session, requestID: id)
            }
        case .automatic, .queue:
            return await perform(.queue(text), in: session) { client, id in
                try await client.queue(text, in: session, requestID: id)
            }
        case .steer:
            return await perform(.steer(text), in: session) { client, id in
                try await client.steer(text, in: session, requestID: id)
            }
        }
    }

    @discardableResult
    public func executeCommand(_ command: String) async -> Bool {
        guard let snapshot = attached else { return false }
        if command.trimmingCharacters(in: .whitespacesAndNewlines) == "/cancel" {
            if let question = snapshot.pendingQuestion { return await cancelQuestion(question) }
            guard snapshot.activeTurnID != nil, !hasPendingCancellation else { return false }
            return await cancelTurn()
        }
        guard snapshot.capabilities.contains("execute_command") else {
            notice = "This connection does not support remote slash commands. Update RustCode and restart its remote gateway on your Mac, then reconnect."
            return false
        }
        return await perform(.command(command), in: snapshot.session, key: "command:\(command)") { client, id in
            try await client.executeCommand(command, in: snapshot.session, requestID: id)
        }
    }

    @discardableResult
    public func cancelQuestion(_ question: PendingQuestion) async -> Bool {
        guard let snapshot = attached, snapshot.pendingQuestion?.id == question.id else { return false }
        guard snapshot.capabilities.contains("cancel_question") else {
            // Older hosts expose only turn cancellation. The captured turn ID gives the
            // same Escape behavior without risking cancellation of a newer turn.
            guard let turnID = snapshot.activeTurnID else {
                notice = "The host did not provide a turn ID. Cancel this question in the host terminal."
                return false
            }
            return await perform(.cancelQuestion, in: snapshot.session, key: "cancel:\(turnID)") { client, id in
                try await client.cancel(turnID: turnID, in: snapshot.session, requestID: id)
            }
        }
        let key = snapshot.activeTurnID.map { "cancel:\($0)" } ?? "cancel-question:\(question.id)"
        return await perform(.cancelQuestion, in: snapshot.session, key: key) { client, id in
            try await client.cancelQuestion(question.id, in: snapshot.session, requestID: id)
        }
    }

    @discardableResult
    public func setSessionSettings(model: String, reasoningEffort: String) async -> Bool {
        guard let snapshot = attached, !snapshot.isRunning,
              let settings = snapshot.settings, settings.canChange,
              let choice = settings.models.first(where: { $0.id == model }),
              choice.reasoningEfforts.contains(reasoningEffort) else { return false }
        return await perform(.settings, in: snapshot.session, key: "settings", desiredModel: model, desiredEffort: reasoningEffort) { client, id in
            try await client.setSessionSettings(model: model, reasoningEffort: reasoningEffort, in: snapshot.session, requestID: id)
        }
    }

    /// Cancels the turn the user is looking at, by its ID.
    @discardableResult
    public func cancelTurn() async -> Bool {
        guard let snapshot = attached, let turnID = snapshot.activeTurnID else { return false }
        let session = snapshot.session
        return await perform(.cancel, in: session, key: "cancel:\(turnID)") { client, id in
            try await client.cancel(turnID: turnID, in: session, requestID: id)
        }
    }

    @discardableResult
    public func answer(_ question: PendingQuestion, with answer: QuestionAnswer) async -> Bool {
        guard let session = attached?.session, !answer.isEmpty, !isQuestionCancellationPending(question.id) else { return false }
        return await perform(.answer, in: session, key: "question:\(question.id)") { client, id in
            try await client.answerQuestion(question.id, with: answer, in: session, requestID: id)
        }
    }

    /// Approving requires the full detail of every item; a bounded preview is not enough.
    @discardableResult
    public func resolve(_ approval: PendingApproval, decision: ApprovalDecision) async -> Bool {
        guard let session = attached?.session else { return false }
        // What counts is the store's copy: it is the one whose details were fetched.
        let current = attached?.pendingApproval?.batchID == approval.batchID ? attached?.pendingApproval : nil
        guard decision == .deny || (current ?? approval).canBeResolved else {
            notice = "The full details of this action have not loaded yet."
            return false
        }
        return await perform(.approval, in: session, key: "approval:\(approval.batchID)") { client, id in
            try await client.resolveApproval(batchID: approval.batchID, decision: decision, in: session, requestID: id)
        }
    }

    /// Asks the host what became of an uncertain request. Never sends the request again.
    public func checkStatus(of requestID: RequestID) async {
        guard let request = pendingMutations[requestID] else { return }
        do {
            let receipt = try await client.getRequestStatus(requestID, in: request.session)
            settle(receipt)
        } catch {
            // Still unknown; the entry stays visible.
        }
    }

    public func dismissRequest(_ requestID: RequestID) {
        receipts.dismiss(requestID)
    }

    // MARK: Internals

    private func adopt(_ stored: StoredCredential, client: any RemoteClient, demo: Bool) {
        resetHostState()
        self.client = client
        credential = stored
        host = stored.host
        isDemo = demo
        startConnection()
    }

    private func resetHostState() {
        client = liveClient
        credential = nil
        host = nil
        isDemo = false
        sessions = []
        selectedSessionID = nil
        attached = nil
        commandResult = nil
        snapshotInstanceID = nil
        currentInstanceID = nil
        badges.removeAll()
        receipts.removeAll()
        pendingMutations.removeAll()
        finalReceipts.removeAll()
        receiptTask?.cancel()
    }

    private func startConnection() {
        guard let credential else { return }
        connectionTask?.cancel()
        attachTask?.cancel()
        connection = .connecting
        let client = client
        connectionTask = Task { [weak self] in
            await self?.runConnection(credential, client: client)
        }
    }

    private func runConnection(_ credential: StoredCredential, client: any RemoteClient) async {
        var reconnect = ReconnectState(policy: backoff)
        while !Task.isCancelled {
            do {
                let identity = try await client.connect(using: credential)
                try Task.checkCancellation()
                currentInstanceID = identity.instanceID
                adoptHostName(identity.hostName, for: credential)
                let receiptStream = await client.receiptUpdates()
                receiptTask?.cancel()
                receiptTask = Task { [weak self] in
                    for await receipt in receiptStream {
                        guard !Task.isCancelled else { return }
                        self?.settle(receipt)
                    }
                }
                let lists = try await client.subscribeSessions()
                try Task.checkCancellation()
                connection = .connected
                reconnect.registerSuccess()

                var isFirst = true
                for try await list in lists {
                    try Task.checkCancellation()
                    applySessionList(list)
                    if isFirst {
                        isFirst = false
                        resyncAfterConnect()
                    }
                }
                try Task.checkCancellation()
                throw RemoteError.connectionClosed
            } catch {
                if Task.isCancelled || error is CancellationError { return }
                attachTask?.cancel()
                receipts.connectionLost()

                let failure = (error as? RemoteError) ?? .transport(error.localizedDescription)
                let reason = failure.localizedDescription
                guard failure.isWorthRetrying || failure.isUnauthorized else {
                    connection = .offline(reason: reason)
                    return
                }
                switch reconnect.registerFailure(unauthorized: failure.isUnauthorized, randomUnit: randomUnit()) {
                case .stop:
                    handleUnauthorized()
                    return
                case .giveUp:
                    connection = .offline(reason: reason)
                    return
                case .retry(let attempt, let delay):
                    // A locked-out host says when it will listen again; knocking earlier is useless.
                    var delay = delay
                    if case .rateLimited(let retryAfter?) = failure, retryAfter > delay {
                        delay = retryAfter
                    }
                    connection = .waitingToRetry(attempt: attempt, delay: delay, reason: reason)
                    do {
                        try await sleep(delay)
                    } catch {
                        return
                    }
                    if Task.isCancelled { return }
                    connection = .connecting
                }
            }
        }
    }

    /// The host can be renamed, and an older pairing only knows its address. Keep the name
    /// it reports now, in memory and in the credential store.
    private func adoptHostName(_ name: String?, for stored: StoredCredential) {
        guard let name, !name.isEmpty, host?.gatewayID == stored.host.gatewayID, host?.name != name else { return }
        host?.name = name
        guard !isDemo, var updated = credential else { return }
        updated.host.name = name
        credential = updated
        try? credentials.save(updated)
    }

    /// Revoked or unauthorized: stop reconnecting and drop the token, which is now useless.
    private func handleUnauthorized() {
        if !isDemo {
            try? credentials.delete()
        }
        resetHostState()
        connection = .unauthorized
        pairing = .failed(RemoteError.unauthorized.localizedDescription)
    }

    private func applySessionList(_ list: [SessionSummary]) {
        badges.observe(previous: sessions, current: list, selected: selectedSessionID)
        sessions = list

        guard let selectedSessionID else { return }
        guard let current = list.first(where: { $0.id == selectedSessionID }) else {
            // No longer shared (`/remote off`, terminal exit or a new session identity).
            attachTask?.cancel()
            attached = nil
        commandResult = nil
            self.selectedSessionID = nil
            notice = "That session is no longer shared."
            return
        }
        if let attached, attached.session != current.ref {
            // Registered again under a new epoch: the old snapshot is another identity's.
            self.attached = nil
            startAttachment(selectedSessionID)
        }
    }

    private func resyncAfterConnect() {
        if let selectedSessionID {
            startAttachment(selectedSessionID)
        }
        for (id, _) in pendingMutations {
            Task { await checkStatus(of: id) }
        }
    }

    private func startAttachment(_ sessionID: String) {
        attachTask?.cancel()
        attachTask = Task { [weak self] in
            await self?.runAttachment(sessionID)
        }
    }

    private func runAttachment(_ sessionID: String) async {
        var wantsSnapshot = false
        var resyncs = 0
        while !Task.isCancelled, connection == .connected, selectedSessionID == sessionID, resyncs < 5,
              let ref = sessions.first(where: { $0.id == sessionID })?.ref {
            // A replay is only worth asking for from the gateway instance that issued the
            // sequence numbers; after a restart they mean nothing.
            let canResume = !wantsSnapshot && attached?.session == ref && snapshotInstanceID == currentInstanceID
            let resumeAfter = canResume ? attached?.sequence : nil
            do {
                let attachment = try await client.attach(ref, resumeAfter: resumeAfter)
                guard !Task.isCancelled, selectedSessionID == sessionID else { return }
                switch attachment.start {
                case .snapshot(let snapshot):
                    attached = snapshot
                    retireObservedMutations()
                    snapshotInstanceID = currentInstanceID
                case .resumed:
                    guard attached?.session == ref else {
                        wantsSnapshot = true
                        continue
                    }
                }

                var lostUpdates = false
                for try await update in attachment.updates {
                    guard !Task.isCancelled else { return }
                    if case .gap = attached?.apply(update) {
                        lostUpdates = true
                        break
                    }
                    retireObservedMutations()
                }
                // The stream ending without a gap means the connection went; the connection
                // loop attaches again once it is back.
                guard lostUpdates, !Task.isCancelled else { return }
                await client.detach(ref)
            } catch RemoteError.resyncRequired {
                // Fall through and attach again for a fresh snapshot.
            } catch RemoteError.sessionClosed(let reason) {
                guard !Task.isCancelled, selectedSessionID == sessionID else { return }
                attached = nil
                commandResult = nil
                selectedSessionID = nil
                notice = reason
                return
            } catch {
                return
            }
            wantsSnapshot = true
            resyncs += 1
        }
    }

    @discardableResult
    private func perform(
        _ kind: TrackedRequest.Kind,
        in session: SessionRef,
        key: String? = nil,
        desiredModel: String? = nil,
        desiredEffort: String? = nil,
        _ operation: (any RemoteClient, RequestID) async throws -> RequestReceipt
    ) async -> Bool {
        guard connection == .connected else {
            notice = RemoteError.notConnected.localizedDescription
            return false
        }
        let key = key ?? "\(kind)"
        guard !isPending(key, in: session) else { return false }
        // One ID for the life of the request, so the host can deduplicate it and its
        // outcome can still be looked up after a disconnect.
        let id = RequestID()
        pendingMutations[id] = PendingMutation(session: session, key: key, desiredModel: desiredModel, desiredEffort: desiredEffort)
        performing.insert(id)
        defer { finalReceipts[id] = nil; performing.remove(id) }
        receipts.begin(TrackedRequest(id: id, session: session, kind: kind))
        do {
            let result = try await operation(client, id)
            let accepted = settle(finalReceipts[id] ?? result)
            if pendingMutations[id] != nil { reconcileLater(id) }
            return accepted
        } catch RemoteError.notConnected {
            pendingMutations[id] = nil
            receipts.dismiss(id)
            notice = RemoteError.notConnected.localizedDescription
            return false
        } catch {
            // Sent, but no answer: it may or may not have been applied. Never resend.
            if let final = finalReceipts[id] { return settle(final) }
            receipts.markUncertain(id)
            reconcileLater(id)
            return true
        }
    }

    /// An authoritative snapshot can show that a target no longer exists or that settings
    /// match. This releases the duplicate guard without claiming to know the request receipt.
    private func retireObservedMutations() {
        guard let snapshot = attached else { return }
        for (id, pending) in pendingMutations where pending.session == snapshot.session {
            let key = pending.key
            let retired = (key.hasPrefix("cancel:") && key != "cancel:\(snapshot.activeTurnID ?? "")")
                || (key.hasPrefix("question:") && key != "question:\(snapshot.pendingQuestion?.id ?? "")")
                || (key.hasPrefix("cancel-question:") && key != "cancel-question:\(snapshot.pendingQuestion?.id ?? "")")
                || (key.hasPrefix("approval:") && key != "approval:\(snapshot.pendingApproval?.batchID ?? "")")
                || (key == "settings" && snapshot.settings?.selectedModel == pending.desiredModel
                    && snapshot.settings?.reasoningEffort == pending.desiredEffort)
            if retired { pendingMutations[id]?.blocksDuplicate = false }
        }
    }

    /// Bounded read-only probes, never a replay of the operation. Late receipts also settle it.
    private func reconcileLater(_ id: RequestID) {
        Task { [weak self] in
            for delay in [1, 3, 10] {
                try? await Task.sleep(for: .seconds(delay))
                guard let self, self.pendingMutations[id] != nil, self.connection == .connected else { return }
                await self.checkStatus(of: id)
            }
        }
    }

    /// - Returns: false when the owner rejected the request.
    @discardableResult
    private func settle(_ receipt: RequestReceipt) -> Bool {
        if let result = receipt.commandResult,
           pendingMutations[receipt.requestID]?.session == attached?.session { commandResult = result }
        if receipt.state.isFinal {
            if performing.contains(receipt.requestID) { finalReceipts[receipt.requestID] = receipt }
            pendingMutations[receipt.requestID] = nil
        }
        if case .rejected(let rejection) = receipt.state {
            receipts.dismiss(receipt.requestID)
            notice = rejection.message
            return false
        }
        receipts.record(receipt)
        return true
    }
}

extension RemoteError {
    /// Failures that another attempt cannot fix are not retried automatically.
    var isWorthRetrying: Bool {
        switch self {
        case .codecUnavailable, .incompatibleProtocol, .unauthorized: false
        default: true
        }
    }
}
