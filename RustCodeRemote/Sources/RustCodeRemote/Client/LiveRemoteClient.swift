import Foundation

/// `RemoteClient` over a `RemoteTransport`. Owns the connection plumbing that does not depend
/// on wire shapes: one receive loop, correlation of replies to requests, timeouts, routing of
/// pushed updates to their subscribers, and failing everything at once when the socket goes.
/// The translation between frames and domain values is delegated to a `RemoteCodec`.
public actor LiveRemoteClient: RemoteClient {
    public typealias TransportFactory = @Sendable () -> any RemoteTransport

    private typealias Waiter = AsyncStream<Result<RemoteReply, RemoteError>>.Continuation
    private typealias SessionsContinuation = AsyncThrowingStream<[SessionSummary], any Error>.Continuation
    private typealias UpdatesContinuation = AsyncThrowingStream<SessionUpdate, any Error>.Continuation

    private let codec: any RemoteCodec
    private let makeTransport: TransportFactory
    private let requestTimeout: Duration

    private var transport: (any RemoteTransport)?
    private var receiveTask: Task<Void, Never>?
    /// Bumped whenever the connection changes, so a finished receive loop cannot tear down its successor.
    private var generation = 0

    private var waiters: [RequestID: Waiter] = [:]
    /// Where each attached session's sequence numbers came from; a resume cursor names it.
    private var replayOrigins: [SessionRef: ReplayOrigin] = [:]
    private var replaySnapshotIDs: [SessionRef: String] = [:]
    /// Mutations in flight; a `received` receipt for these is not the final answer.
    private var mutations: Set<RequestID> = []
    private var receivedOnly: Set<RequestID> = []
    private var sessionsContinuation: SessionsContinuation?
    private var updateContinuations: [SessionRef: UpdatesContinuation] = [:]
    /// Sessions whose events cannot be trusted until the snapshot the host announced arrives.
    private var awaitingSnapshot: Set<SessionRef> = []

    public init(
        codec: any RemoteCodec,
        requestTimeout: Duration = .seconds(20),
        transport: @escaping TransportFactory = { URLSessionWebSocketTransport() }
    ) {
        self.codec = codec
        self.requestTimeout = requestTimeout
        self.makeTransport = transport
    }

    // MARK: Pairing and connection

    public func pair(_ request: PairingRequest, deviceName: String) async throws -> StoredCredential {
        let target = try codec.pairingTarget(for: request)
        try await open(target.address)
        do {
            let reply = try await self.request(
                .pair(credential: target.credential, deviceName: deviceName),
                id: Self.handshakeID
            )
            // Pairing leaves the connection authenticated, but the app connects afresh with
            // the stored token, which also proves the token works.
            await teardown(with: .connectionClosed)
            guard case .handshake(.paired(let grant)) = reply else { throw Self.unexpected(reply) }
            if let expected = target.expectedGatewayID, expected != grant.identity.gatewayID {
                throw RemoteError.pairingFailed("the host that answered is not the one the code was made for")
            }
            return StoredCredential(
                host: PairedHost(
                    gatewayID: grant.identity.gatewayID,
                    name: grant.identity.hostName ?? target.hostName ?? target.address.host,
                    address: target.address,
                    device: grant.device,
                    pairedAt: Date()
                ),
                deviceToken: grant.deviceToken
            )
        } catch {
            await teardown(with: .connectionClosed)
            throw error
        }
    }

    @discardableResult
    public func connect(using credential: StoredCredential) async throws -> ConnectionIdentity {
        try await open(credential.host.address)
        do {
            let reply = try await request(
                .authenticate(device: credential.host.device, deviceToken: credential.deviceToken),
                id: Self.handshakeID
            )
            guard case .handshake(.authenticated(let identity)) = reply else { throw Self.unexpected(reply) }
            guard identity.gatewayID == credential.host.gatewayID else {
                throw RemoteError.hostIdentityChanged
            }
            return identity
        } catch {
            await teardown(with: .connectionClosed)
            throw error
        }
    }

    public func disconnect() async {
        await teardown(with: .connectionClosed)
    }

    // MARK: Sessions

    public func subscribeSessions() async throws -> AsyncThrowingStream<[SessionSummary], any Error> {
        // Register before sending so that nothing pushed right after the reply is missed;
        // the receive loop yields the initial list and later changes in arrival order.
        let (stream, continuation) = AsyncThrowingStream.makeStream(of: [SessionSummary].self)
        sessionsContinuation?.finish()
        sessionsContinuation = continuation
        do {
            let reply = try await request(.subscribeSessions)
            guard case .sessions = reply else { throw Self.unexpected(reply) }
            return stream
        } catch {
            continuation.finish()
            sessionsContinuation = nil
            throw error
        }
    }

    public func refreshSessions() async throws {
        guard sessionsContinuation != nil else { throw RemoteError.notConnected }
        // The reply's list is delivered through the stream already handed out.
        let reply = try await request(.subscribeSessions)
        guard case .sessions = reply else { throw Self.unexpected(reply) }
    }

    public func attach(_ session: SessionRef, resumeAfter: UInt64?) async throws -> SessionAttachment {
        let (stream, continuation) = AsyncThrowingStream.makeStream(of: SessionUpdate.self)
        updateContinuations[session]?.finish()
        updateContinuations[session] = continuation
        awaitingSnapshot.remove(session)
        // A replay needs to name where the cursor came from; without that, ask for a snapshot.
        // The gateway decides whether it can replay and says so in its reply.
        let resume = resumeAfter.flatMap { sequence in
            replayOrigins[session].map { ResumeCursor(origin: $0, lastSequence: sequence, snapshotID: replaySnapshotIDs[session]) }
        }
        do {
            let reply = try await request(.attachSession(session, resume: resume))
            guard case .attached(let start, let origin) = reply else { throw Self.unexpected(reply) }
            replayOrigins[session] = origin
            return SessionAttachment(start: start, updates: stream)
        } catch {
            continuation.finish()
            updateContinuations[session] = nil
            throw error
        }
    }

    public func detach(_ session: SessionRef) async {
        updateContinuations.removeValue(forKey: session)?.finish()
        _ = try? await request(.detachSession(session))
    }

    public func getHistory(_ session: SessionRef, before cursor: HistoryCursor, limit: Int) async throws -> HistoryPage {
        let reply = try await request(.getHistory(session, before: cursor, limit: limit))
        guard case .history(let page) = reply else { throw Self.unexpected(reply) }
        return page
    }

    public func getContent(
        _ contentID: String,
        in session: SessionRef,
        offset: UInt64,
        maxBytes: Int
    ) async throws -> ContentChunk {
        let reply = try await request(
            .getContent(session, contentID: contentID, offset: offset, maxBytes: maxBytes)
        )
        guard case .content(let chunk) = reply else { throw Self.unexpected(reply) }
        return chunk
    }

    // MARK: Mutations

    public func submit(_ text: String, to session: SessionRef, requestID: RequestID) async throws -> RequestReceipt {
        try await mutate(.submitPrompt(session, text: text), id: requestID)
    }

    public func steer(_ text: String, in session: SessionRef, requestID: RequestID) async throws -> RequestReceipt {
        try await mutate(.steer(session, text: text), id: requestID)
    }

    public func queue(_ text: String, in session: SessionRef, requestID: RequestID) async throws -> RequestReceipt {
        try await mutate(.queue(session, text: text), id: requestID)
    }

    public func cancel(turnID: String, in session: SessionRef, requestID: RequestID) async throws -> RequestReceipt {
        try await mutate(.cancelTurn(session, turnID: turnID), id: requestID)
    }

    public func answerQuestion(
        _ questionID: String,
        with answer: QuestionAnswer,
        in session: SessionRef,
        requestID: RequestID
    ) async throws -> RequestReceipt {
        try await mutate(.answerQuestion(session, questionID: questionID, answer: answer), id: requestID)
    }

    public func resolveApproval(
        batchID: String,
        decision: ApprovalDecision,
        in session: SessionRef,
        requestID: RequestID
    ) async throws -> RequestReceipt {
        try await mutate(.resolveApproval(session, batchID: batchID, decision: decision), id: requestID)
    }

    public func getRequestStatus(_ requestID: RequestID, in session: SessionRef) async throws -> RequestReceipt {
        let reply = try await request(.getRequestStatus(session, target: requestID))
        guard case .receipt(let state) = reply else { throw Self.unexpected(reply) }
        return RequestReceipt(requestID: requestID, state: state)
    }

    // MARK: Plumbing

    private func open(_ address: HostAddress) async throws {
        await teardown(with: .connectionClosed)
        let endpoint = try codec.endpoint(for: address)
        let transport = makeTransport()
        do {
            try await transport.connect(to: endpoint.url, headers: endpoint.headers)
        } catch {
            throw Self.remoteError(from: error)
        }
        generation += 1
        let generation = generation
        self.transport = transport
        receiveTask = Task { [weak self] in
            var failure: RemoteError?
            do {
                for try await frame in transport.incoming {
                    await self?.receive(frame, generation: generation)
                }
            } catch {
                failure = Self.remoteError(from: error)
            }
            await self?.connectionEnded(generation: generation, error: failure)
        }
    }

    /// Sends one command and waits for its reply. A `.failure` reply is thrown.
    private func request(_ command: RemoteCommand, id: RequestID = RequestID()) async throws -> RemoteReply {
        guard let transport else { throw RemoteError.notConnected }
        let frame = try codec.encode(command, requestID: id)

        // The waiter is registered before the frame leaves, so a fast reply cannot be missed.
        let (replies, waiter) = AsyncStream.makeStream(of: Result<RemoteReply, RemoteError>.self)
        waiters[id] = waiter
        let timer = Task { [weak self, requestTimeout] in
            try? await Task.sleep(for: requestTimeout)
            guard !Task.isCancelled else { return }
            await self?.timeOut(id)
        }
        defer {
            timer.cancel()
            waiters[id] = nil
            receivedOnly.remove(id)
        }

        do {
            try await transport.send(frame)
        } catch {
            throw Self.remoteError(from: error)
        }
        for await result in replies {
            let reply = try result.get()
            if case .failure(let error) = reply { throw error }
            return reply
        }
        throw RemoteError.connectionClosed
    }

    private func mutate(_ command: RemoteCommand, id: RequestID) async throws -> RequestReceipt {
        mutations.insert(id)
        defer { mutations.remove(id) }
        do {
            let reply = try await request(command, id: id)
            guard case .receipt(let state) = reply else { throw Self.unexpected(reply) }
            return RequestReceipt(requestID: id, state: state)
        } catch RemoteError.rejected(let rejection) {
            return RequestReceipt(requestID: id, state: .rejected(rejection))
        } catch RemoteError.outcomeUnknown {
            return RequestReceipt(requestID: id, state: .unknown)
        }
    }

    private func receive(_ frame: String, generation: Int) async {
        guard generation == self.generation else { return }
        let message: RemoteMessage
        do {
            message = try codec.decode(frame)
        } catch {
            // A frame outside the schema is rejected, not guessed at: drop the connection
            // and let reconnect resynchronise from a snapshot.
            let reason = (error as? RemoteError) ?? .invalidFrame(String(describing: error))
            await teardown(with: reason)
            return
        }

        switch message {
        case .reply(let id, let reply):
            if case .attached(.snapshot(let snapshot), _) = reply {
                replaySnapshotIDs[snapshot.session] = snapshot.snapshotID
            }
            if case .sessions(let sessions) = reply {
                sessionsContinuation?.yield(sessions)
            }
            if case .receipt(.received) = reply, mutations.contains(id) {
                receivedOnly.insert(id)
                return
            }
            resolve(id, with: .success(reply))
        case .handshake(let reply):
            resolve(Self.handshakeID, with: .success(.handshake(reply)))
        case .gatewayFailure(let failure):
            if waiters[Self.handshakeID] != nil {
                // The handshake failed; the gateway closes the connection behind this frame.
                resolve(Self.handshakeID, with: .failure(failure.error))
            } else {
                // After the handshake every reply is a correlated response. An error that
                // names no request says why the gateway is closing the connection.
                await teardown(with: failure.error)
            }
        case .sessionsChanged(let sessions):
            sessionsContinuation?.yield(sessions)
        case .sessionUpdate(let update):
            if update.isSnapshot, case .replaced(let snapshot) = update.changes[0] {
                replaySnapshotIDs[update.session] = snapshot.snapshotID
            }
            if awaitingSnapshot.contains(update.session) {
                // Nothing between a resync notice and its snapshot can be trusted.
                guard update.isSnapshot else { return }
                awaitingSnapshot.remove(update.session)
            }
            updateContinuations[update.session]?.yield(update)
        case .resyncRequired(let session):
            // The host follows this with a snapshot event on the same subscription.
            awaitingSnapshot.insert(session)
        case .sessionClosed(let session, let reason):
            replayOrigins[session] = nil
            replaySnapshotIDs[session] = nil
            updateContinuations.removeValue(forKey: session)?
                .finish(throwing: RemoteError.sessionClosed(reason))
        case .unknown:
            // A newer host may send frames this version does not know. They carry nothing
            // this client could act on.
            break
        }
    }

    private func resolve(_ id: RequestID, with result: Result<RemoteReply, RemoteError>) {
        guard let waiter = waiters.removeValue(forKey: id) else { return }
        waiter.yield(result)
        waiter.finish()
    }

    private func timeOut(_ id: RequestID) {
        if receivedOnly.contains(id) {
            // The gateway has the request but the owner has not answered: report exactly that.
            resolve(id, with: .success(.receipt(.received)))
        } else {
            resolve(id, with: .failure(.timeout))
        }
    }

    private func connectionEnded(generation: Int, error: RemoteError?) async {
        guard generation == self.generation else { return }
        await teardown(with: error ?? .connectionClosed)
    }

    private func teardown(with error: RemoteError) async {
        guard let transport else { return }
        self.transport = nil
        generation += 1
        receiveTask?.cancel()
        receiveTask = nil

        for id in Array(waiters.keys) {
            resolve(id, with: .failure(error))
        }
        sessionsContinuation?.finish(throwing: error)
        sessionsContinuation = nil
        for continuation in updateContinuations.values {
            continuation.finish(throwing: error)
        }
        updateContinuations.removeAll()
        awaitingSnapshot.removeAll()

        await transport.close()
    }

    /// The handshake frames carry no request ID; their reply is correlated under this one.
    private static let handshakeID = RequestID("handshake")

    private static func unexpected(_ reply: RemoteReply) -> RemoteError {
        .invalidFrame("unexpected reply")
    }

    private static func remoteError(from error: any Error) -> RemoteError {
        (error as? RemoteError) ?? .transport(error.localizedDescription)
    }
}
