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

    public private(set) var badges = AttentionBadges()
    public private(set) var receipts = ReceiptLedger()

    /// A message for the user about the last thing that went wrong. Views clear it once shown.
    public var notice: String?

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

    public func select(_ sessionID: String?) {
        guard sessionID != selectedSessionID else { return }
        attachTask?.cancel()
        if let previous = attached?.session {
            let client = client
            Task { await client.detach(previous) }
        }
        attached = nil
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

    // MARK: Mutations

    /// Sends text to the attached session.
    /// - Returns: false when the text certainly did not reach the session, so the caller can
    ///   give it back to the user. True does not mean applied: the outcome may be uncertain.
    @discardableResult
    public func send(_ text: String, mode: SendMode = .automatic) async -> Bool {
        let text = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, let snapshot = attached else { return false }
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

    /// Cancels the turn the user is looking at, by its ID.
    public func cancelTurn() async {
        guard let snapshot = attached, let turnID = snapshot.activeTurnID else { return }
        let session = snapshot.session
        await perform(.cancel, in: session) { client, id in
            try await client.cancel(turnID: turnID, in: session, requestID: id)
        }
    }

    @discardableResult
    public func answer(_ question: PendingQuestion, with answer: QuestionAnswer) async -> Bool {
        guard let session = attached?.session, !answer.isEmpty else { return false }
        return await perform(.answer, in: session) { client, id in
            try await client.answerQuestion(question.id, with: answer, in: session, requestID: id)
        }
    }

    /// Approving requires the full detail of every item; a bounded preview is not enough.
    @discardableResult
    public func resolve(_ approval: PendingApproval, decision: ApprovalDecision) async -> Bool {
        guard let session = attached?.session else { return false }
        guard decision == .deny || approval.canBeResolved else {
            notice = "The full details of this action have not loaded yet."
            return false
        }
        return await perform(.approval, in: session) { client, id in
            try await client.resolveApproval(batchID: approval.batchID, decision: decision, in: session, requestID: id)
        }
    }

    /// Asks the host what became of an uncertain request. Never sends the request again.
    public func checkStatus(of requestID: RequestID) async {
        guard let request = receipts.requests.first(where: { $0.id == requestID }) else { return }
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
        badges.removeAll()
        receipts.removeAll()
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
                try await client.connect(using: credential)
                try Task.checkCancellation()
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
        for request in receipts.uncertain {
            Task { await checkStatus(of: request.id) }
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
            let resumeAfter = !wantsSnapshot && attached?.session == ref ? attached?.sequence : nil
            do {
                let attachment = try await client.attach(ref, resumeAfter: resumeAfter)
                guard !Task.isCancelled, selectedSessionID == sessionID else { return }
                switch attachment.start {
                case .snapshot(let snapshot):
                    attached = snapshot
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
                }
                // The stream ending without a gap means the connection went; the connection
                // loop attaches again once it is back.
                guard lostUpdates, !Task.isCancelled else { return }
                await client.detach(ref)
            } catch RemoteError.resyncRequired {
                // Fall through and attach again for a fresh snapshot.
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
        _ operation: (any RemoteClient, RequestID) async throws -> RequestReceipt
    ) async -> Bool {
        guard connection == .connected else {
            notice = RemoteError.notConnected.localizedDescription
            return false
        }
        // One ID for the life of the request, so the host can deduplicate it and its
        // outcome can still be looked up after a disconnect.
        let id = RequestID()
        receipts.begin(TrackedRequest(id: id, session: session, kind: kind))
        do {
            return settle(try await operation(client, id))
        } catch RemoteError.notConnected {
            receipts.dismiss(id)
            notice = RemoteError.notConnected.localizedDescription
            return false
        } catch {
            // Sent, but no answer: it may or may not have been applied. Never resend.
            receipts.markUncertain(id)
            return true
        }
    }

    /// - Returns: false when the owner rejected the request.
    @discardableResult
    private func settle(_ receipt: RequestReceipt) -> Bool {
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
