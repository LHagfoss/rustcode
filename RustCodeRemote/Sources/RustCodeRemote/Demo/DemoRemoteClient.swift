import Foundation

/// A simulated RustCode host, entirely in memory. It follows the rules the real one will:
/// sequence-numbered updates, idle-only submit, identity-checked cancel/answer/approval,
/// at-most-once application per request ID, and work that continues while the phone is away.
public actor DemoRemoteClient: RemoteClient {
    private struct Session {
        var summary: SessionSummary
        var state: SessionSnapshot
        var olderHistory: [TranscriptItem] = []
        var queued: [String] = []
        var startsLiveTurnOnAttach = false
        var turnTask: Task<Void, Never>?
        var updates: AsyncThrowingStream<SessionUpdate, any Error>.Continuation?
    }

    private let pace: Duration
    private var sessions: [String: Session] = [:]
    private var order: [String] = []
    private var listContinuation: AsyncThrowingStream<[SessionSummary], any Error>.Continuation?
    private var outcomes: [RequestID: ReceiptState] = [:]
    private var isConnected = false
    private var counter = 0

    public static let approvalSessionID = "demo-backoff"
    public static let questionSessionID = "demo-store"
    public static let idleSessionID = "demo-notes"

    /// - Parameter pace: the delay between streamed chunks. `.zero` makes scripts run as
    ///   fast as the scheduler allows, which is what tests want.
    public init(pace: Duration = .milliseconds(45)) {
        self.pace = pace
        for var session in Self.seed() {
            session.state.settings = SessionSettings(models: [
                .init(id: "demo-balanced", model: "demo-model", reasoningEfforts: ["default", "medium", "high"]),
                .init(id: "demo-fast", model: "demo-fast-model", reasoningEfforts: ["default"])
            ], selectedModel: "demo-balanced", reasoningEffort: "default", canChange: !session.state.isRunning)
            // The demo owns its complete transcript, including the older history page.
            session.summary.turnCount = UInt64((session.olderHistory + session.state.items).filter {
                if case .user = $0.content { return true }; return false
            }.count)
            sessions[session.summary.id] = session
            order.append(session.summary.id)
        }
    }

    // MARK: Pairing and connection

    public func pair(_ request: PairingRequest, deviceName: String) async throws -> StoredCredential {
        StoredCredential(
            host: PairedHost(
                gatewayID: "demo-gateway",
                name: "Demo Mac",
                address: HostAddress(host: "demo.local", port: 17879),
                device: DeviceIdentity(id: "demo-device", name: deviceName),
                pairedAt: Date()
            ),
            deviceToken: "demo"
        )
    }

    @discardableResult
    public func connect(using credential: StoredCredential) async throws -> ConnectionIdentity {
        isConnected = true
        return ConnectionIdentity(gatewayID: "demo-gateway", instanceID: "demo-instance", hostName: "Demo Mac")
    }

    public func disconnect() async {
        isConnected = false
        listContinuation?.finish()
        listContinuation = nil
        for id in order {
            sessions[id]?.updates?.finish()
            sessions[id]?.updates = nil
        }
    }

    // MARK: Sessions

    public func subscribeSessions() async throws -> AsyncThrowingStream<[SessionSummary], any Error> {
        guard isConnected else { throw RemoteError.notConnected }
        let (stream, continuation) = AsyncThrowingStream.makeStream(of: [SessionSummary].self)
        listContinuation?.finish()
        listContinuation = continuation
        continuation.yield(summaries)
        return stream
    }

    public func refreshSessions() async throws {
        guard isConnected, let listContinuation else { throw RemoteError.notConnected }
        _ = await pause(8)
        listContinuation.yield(summaries)
    }

    public func attach(_ session: SessionRef, resumeAfter: UInt64?) async throws -> SessionAttachment {
        guard isConnected else { throw RemoteError.notConnected }
        guard let current = sessions[session.sessionID], current.state.session == session else {
            throw RemoteError.rejected(.staleSession)
        }
        let (stream, continuation) = AsyncThrowingStream.makeStream(of: SessionUpdate.self)
        current.updates?.finish()
        sessions[session.sessionID]?.updates = continuation

        if current.startsLiveTurnOnAttach {
            sessions[session.sessionID]?.startsLiveTurnOnAttach = false
            startTask(in: session.sessionID) { await $0.runBackoffTurn() }
        }
        // Snapshot and subscription are one cut: both happen without suspending the actor.
        return SessionAttachment(start: .snapshot(current.state), updates: stream)
    }

    public func detach(_ session: SessionRef) async {
        sessions[session.sessionID]?.updates?.finish()
        sessions[session.sessionID]?.updates = nil
    }

    public func getHistory(_ session: SessionRef, before cursor: HistoryCursor, limit: Int) async throws -> HistoryPage {
        guard let current = sessions[session.sessionID], current.state.session == session else {
            throw RemoteError.rejected(.staleSession)
        }
        guard current.state.historyCursor == cursor else {
            return HistoryPage(items: [], nextCursor: nil)
        }
        sessions[session.sessionID]?.state.prepend(HistoryPage(items: current.olderHistory, nextCursor: nil))
        return HistoryPage(items: current.olderHistory, nextCursor: nil)
    }

    public func getContent(
        _ contentID: String,
        in session: SessionRef,
        offset: UInt64,
        maxBytes: Int
    ) async throws -> ContentChunk {
        guard isConnected else { throw RemoteError.notConnected }
        guard contentID == Self.testApprovalContentID else {
            throw RemoteError.rejected(.other(code: "not_found", message: "That content is no longer available."))
        }
        // Served in small chunks so that fetching is visibly a process.
        let bytes = Array(Self.testApprovalDetail.utf8)
        let start = min(Int(offset), bytes.count)
        let end = min(start + min(maxBytes, 48), bytes.count)
        _ = await pause(6)
        return ContentChunk(
            contentID: contentID,
            offset: UInt64(start),
            text: String(decoding: bytes[start..<end], as: UTF8.self),
            totalBytes: UInt64(bytes.count),
            nextOffset: end < bytes.count ? UInt64(end) : nil
        )
    }

    // MARK: Mutations

    public func setSessionSettings(model: String, reasoningEffort: String, in session: SessionRef, requestID: RequestID) async throws -> RequestReceipt {
        try once(requestID, in: session) { current in
            guard !current.state.isRunning else { return .rejected(.busy) }
            guard var settings = current.state.settings,
                  let choice = settings.models.first(where: { $0.id == model }),
                  choice.reasoningEfforts.contains(reasoningEffort) else { return .rejected(.unsupportedOperation) }
            settings.selectedModel = model
            settings.reasoningEffort = reasoningEffort
            var snapshot = current.state
            snapshot.settings = settings
            snapshot.sequence += 1
            emit(session.sessionID, .replaced(snapshot))
            sessions[session.sessionID]?.summary.model = choice.model
            listContinuation?.yield(summaries)
            return .applied
        }
    }

    public func submit(_ text: String, to session: SessionRef, requestID: RequestID) async throws -> RequestReceipt {
        try once(requestID, in: session) { current in
            guard !current.state.isRunning else { return .rejected(.busy) }
            guard !text.hasPrefix("/") else { return .rejected(.unsupportedOperation) }
            beginReplyTurn(to: text, in: session.sessionID)
            return .applied
        }
    }

    public func steer(_ text: String, in session: SessionRef, requestID: RequestID) async throws -> RequestReceipt {
        try once(requestID, in: session) { current in
            guard current.state.isRunning else {
                return .rejected(.other(code: "not_running", message: "There is no running turn to steer."))
            }
            emit(session.sessionID, .itemAppended(TranscriptItem(id: nextID("user"), .user(text))))
            emit(session.sessionID, .itemAppended(TranscriptItem(id: nextID("notice"), .notice("Steering the running turn"))))
            return .applied
        }
    }

    public func queue(_ text: String, in session: SessionRef, requestID: RequestID) async throws -> RequestReceipt {
        try once(requestID, in: session) { current in
            if current.state.isRunning {
                sessions[session.sessionID]?.queued.append(text)
                emit(session.sessionID, .itemAppended(TranscriptItem(id: nextID("notice"), .notice("Queued: \(text)"))))
            } else {
                beginReplyTurn(to: text, in: session.sessionID)
            }
            return .applied
        }
    }

    public func cancel(turnID: String, in session: SessionRef, requestID: RequestID) async throws -> RequestReceipt {
        try once(requestID, in: session) { current in
            guard current.state.activeTurnID == turnID else { return .rejected(.staleTurn) }
            current.turnTask?.cancel()
            sessions[session.sessionID]?.turnTask = nil
            sessions[session.sessionID]?.queued.removeAll()
            if current.state.pendingQuestion != nil { emit(session.sessionID, .questionChanged(nil)) }
            if current.state.pendingApproval != nil { emit(session.sessionID, .approvalChanged(nil)) }
            emit(session.sessionID, .itemAppended(TranscriptItem(id: nextID("notice"), .notice("Turn cancelled"))))
            emit(session.sessionID, .turnFinished(turnID: turnID, outcome: .cancelled))
            publishSummaries()
            return .applied
        }
    }

    public func answerQuestion(
        _ questionID: String,
        with answer: QuestionAnswer,
        in session: SessionRef,
        requestID: RequestID
    ) async throws -> RequestReceipt {
        try once(requestID, in: session) { current in
            guard let question = current.state.pendingQuestion, question.id == questionID else {
                return .rejected(.staleQuestion)
            }
            let known = Set(question.options.map(\.id))
            guard answer.selectedOptionIDs.allSatisfy(known.contains) else {
                return .rejected(.other(code: "invalid_answer", message: "That option is not part of the question."))
            }
            let labels = question.options.filter { answer.selectedOptionIDs.contains($0.id) }.map(\.label)
            let reply = (labels + [answer.text].compactMap { $0 }.filter { !$0.isEmpty }).joined(separator: ", ")
            emit(session.sessionID, .questionChanged(nil))
            emit(session.sessionID, .itemAppended(TranscriptItem(id: nextID("user"), .user(reply))))
            publishSummaries()
            startTask(in: session.sessionID) { await $0.runAfterAnswer(reply, in: session.sessionID) }
            return .applied
        }
    }

    public func resolveApproval(
        batchID: String,
        decision: ApprovalDecision,
        in session: SessionRef,
        requestID: RequestID
    ) async throws -> RequestReceipt {
        try once(requestID, in: session) { current in
            guard let approval = current.state.pendingApproval, approval.batchID == batchID else {
                return .rejected(.staleApproval)
            }
            emit(session.sessionID, .approvalChanged(nil))
            publishSummaries()
            startTask(in: session.sessionID) { await $0.runAfterApproval(decision, in: session.sessionID) }
            return .applied
        }
    }

    public func getRequestStatus(_ requestID: RequestID, in session: SessionRef) async throws -> RequestReceipt {
        guard isConnected else { throw RemoteError.notConnected }
        return RequestReceipt(requestID: requestID, state: outcomes[requestID] ?? .unknown)
    }

    // MARK: Engine

    private var summaries: [SessionSummary] {
        order.compactMap { sessions[$0]?.summary }
    }

    /// Applies a mutation at most once per request ID and remembers its outcome.
    private func once(
        _ requestID: RequestID,
        in session: SessionRef,
        _ body: (Session) -> ReceiptState
    ) throws -> RequestReceipt {
        guard isConnected else { throw RemoteError.notConnected }
        if let known = outcomes[requestID] {
            return RequestReceipt(requestID: requestID, state: known)
        }
        let state: ReceiptState
        if let current = sessions[session.sessionID], current.state.session == session {
            state = body(current)
        } else {
            state = .rejected(.staleSession)
        }
        outcomes[requestID] = state
        return RequestReceipt(requestID: requestID, state: state)
    }

    private func nextID(_ prefix: String) -> String {
        counter += 1
        return "\(prefix)-live-\(counter)"
    }

    private func emit(_ sessionID: String, _ event: SessionEvent) {
        guard var session = sessions[sessionID] else { return }
        let update = SessionUpdate(session: session.state.session, sequence: session.state.sequence + 1, event: event)
        session.state.apply(update)
        if case .turnStarted = event {
            if let count = sessions[sessionID]?.summary.turnCount { sessions[sessionID]?.summary.turnCount = count + 1 }
        }
        let canChange = !session.state.isRunning
        session.state.settings?.canChange = canChange
        sessions[sessionID]?.state = session.state
        session.updates?.yield(update)
    }

    /// Recomputes list metadata from session state and pushes the list if it changed.
    private func publishSummaries() {
        var changed = false
        for id in order {
            guard let session = sessions[id] else { continue }
            var summary = session.summary
            let state = session.state
            summary.attention = SessionAttention(
                pendingQuestions: state.pendingQuestion == nil ? 0 : 1,
                pendingApprovals: state.pendingApproval == nil ? 0 : 1
            )
            summary.activity = !summary.attention.isEmpty ? .waitingForUser : state.isRunning ? .running : .idle
            if summary != session.summary {
                sessions[id]?.summary = summary
                changed = true
            }
        }
        if changed {
            listContinuation?.yield(summaries)
        }
    }

    private func startTask(in sessionID: String, _ body: @escaping @Sendable (DemoRemoteClient) async -> Void) {
        sessions[sessionID]?.turnTask?.cancel()
        sessions[sessionID]?.turnTask = Task { [weak self] in
            guard let self else { return }
            await body(self)
        }
    }

    /// - Returns: false when the turn was cancelled meanwhile.
    private func pause(_ multiple: Int = 1) async -> Bool {
        if pace > .zero {
            try? await Task.sleep(for: pace * multiple)
        } else {
            await Task.yield()
        }
        return !Task.isCancelled
    }

    private func stream(_ text: String, in sessionID: String) async -> Bool {
        let itemID = nextID("assistant")
        for chunk in Self.chunks(of: text) {
            emit(sessionID, .assistantDelta(itemID: itemID, text: chunk))
            guard await pause() else { return false }
        }
        emit(sessionID, .assistantCompleted(itemID: itemID))
        return true
    }

    private func runTool(_ tool: ToolCall, output: String, in sessionID: String) async -> Bool {
        let itemID = nextID("tool")
        var tool = tool
        tool.status = .running
        emit(sessionID, .itemAppended(TranscriptItem(id: itemID, .tool(tool))))
        guard await pause(14) else { return false }
        tool.status = .succeeded
        tool.output = output
        emit(sessionID, .itemUpdated(TranscriptItem(id: itemID, .tool(tool))))
        return await pause(4)
    }

    private func finishTurn(in sessionID: String) {
        guard let turnID = sessions[sessionID]?.state.activeTurnID else { return }
        emit(sessionID, .turnFinished(turnID: turnID, outcome: .completed))
        publishSummaries()
        if let next = sessions[sessionID]?.queued.first {
            sessions[sessionID]?.queued.removeFirst()
            beginReplyTurn(to: next, in: sessionID)
        }
    }

    // MARK: Scripts

    /// The turn is running from the moment the prompt is accepted, so a second submit that
    /// arrives right behind it is rejected as busy instead of starting a second turn.
    private func beginReplyTurn(to prompt: String, in sessionID: String) {
        emit(sessionID, .itemAppended(TranscriptItem(id: nextID("user"), .user(prompt))))
        emit(sessionID, .turnStarted(turnID: nextID("turn")))
        publishSummaries()
        startTask(in: sessionID) { await $0.runReplyTurn(to: prompt, in: sessionID) }
    }

    private func runReplyTurn(to prompt: String, in sessionID: String) async {
        guard await pause(6) else { return }

        let search = ToolCall(
            name: "search",
            summary: "rg \"\(prompt.prefix(24))\"",
            input: "pattern: \(prompt.prefix(60))\npath: .",
            status: .running
        )
        guard await runTool(search, output: "3 matches in 2 files", in: sessionID) else { return }

        let reply = """
            This is the demo host, so nothing was really run. A live session would now work on:

            > \(prompt)

            What you can try from here:

            - Send another message while this one streams to see it **queued**.
            - Press stop to cancel the turn by its ID.
            - Open the other sessions for a pending question and a pending approval.
            """
        guard await stream(reply, in: sessionID) else { return }
        finishTurn(in: sessionID)
    }

    private func runBackoffTurn() async {
        let sessionID = Self.approvalSessionID
        // The seed already has this turn running; the script only plays it out.
        guard await pause(4) else { return }

        guard await stream(
            "I'll look at how the client schedules reconnects today, then replace the fixed delay with exponential backoff and jitter.",
            in: sessionID
        ) else { return }

        let read = ToolCall(
            name: "read_file",
            summary: "engine/src/remote/client.rs",
            input: "path: engine/src/remote/client.rs\nlines: 120-180",
            status: .running
        )
        let source = """
            fn schedule_reconnect(&mut self) {
                self.timer.reset(Duration::from_secs(5));
            }
            """
        guard await runTool(read, output: source, in: sessionID) else { return }

        let edit = ToolCall(
            name: "edit_file",
            summary: "engine/src/remote/client.rs (+18 −2)",
            input: "Replace the fixed 5 s delay with `Backoff::next_delay()`.",
            status: .running
        )
        guard await runTool(edit, output: "Applied 1 edit", in: sessionID) else { return }

        guard await stream(
            """
            The delay now doubles from 1 s up to a 30 s ceiling, with ±20 % jitter:

            ```rust
            let base = self.initial * 2u32.pow(self.attempt.min(5));
            let delay = base.min(self.ceiling).mul_f64(jitter);
            ```

            Next I want to run the reconnect tests to confirm nothing regressed.
            """,
            in: sessionID
        ) else { return }

        emit(sessionID, .approvalChanged(Self.testApproval))
        publishSummaries()
    }

    private func runAfterApproval(_ decision: ApprovalDecision, in sessionID: String) async {
        switch decision {
        case .approve:
            let run = ToolCall(
                name: "run_command",
                summary: "cargo test -p engine reconnect",
                input: "cargo test -p engine reconnect",
                status: .running
            )
            let output = """
                running 6 tests
                test remote::backoff::doubles_until_ceiling ... ok
                test remote::backoff::jitter_stays_in_range ... ok
                test remote::client::reconnects_after_drop ... ok
                test remote::client::stops_on_unauthorized ... ok
                test remote::client::manual_retry_resets ... ok
                test remote::client::resyncs_after_gap ... ok

                test result: ok. 6 passed; 0 failed
                """
            guard await runTool(run, output: output, in: sessionID) else { return }
            guard await stream("All six reconnect tests pass. The change is ready to review.", in: sessionID) else { return }
        case .deny:
            emit(sessionID, .itemAppended(TranscriptItem(id: nextID("notice"), .notice("Command denied"))))
            guard await stream(
                "Understood, I did not run the tests. The edit is in place but unverified.",
                in: sessionID
            ) else { return }
        }
        finishTurn(in: sessionID)
    }

    private func runAfterAnswer(_ answer: String, in sessionID: String) async {
        guard await pause(6) else { return }
        guard await stream(
            "Going with **\(answer)**. I'll keep the public API unchanged and move the storage behind it.",
            in: sessionID
        ) else { return }
        finishTurn(in: sessionID)
    }

    // MARK: Seed data

    private static let testApprovalContentID = "approval:batch-demo-1:0"
    private static let testApprovalDetail =
        "cargo test -p engine reconnect\n\nWorking directory: ~/code/rustcode\nSandbox: workspace write, no network"

    /// Arrives as a preview only, like a long action from a real host: the rest has to be
    /// fetched before the batch can be approved.
    private static let testApproval = PendingApproval(
        batchID: "batch-demo-1",
        items: [
            PendingApproval.Item(
                id: "approval-demo-1",
                toolName: "run_command",
                title: "Run a shell command",
                risk: "This action requires your approval before it can continue.",
                detail: String(testApprovalDetail.prefix(22)),
                isDetailComplete: false,
                detailContentID: testApprovalContentID,
                detailTotalBytes: UInt64(testApprovalDetail.utf8.count)
            )
        ]
    )

    static func chunks(of text: String) -> [String] {
        var chunks: [String] = []
        var current = ""
        var words = 0
        for character in text {
            current.append(character)
            if character == " " || character == "\n" {
                words += 1
                if words == 2 {
                    chunks.append(current)
                    current = ""
                    words = 0
                }
            }
        }
        if !current.isEmpty { chunks.append(current) }
        return chunks
    }

    private static func seed() -> [Session] {
        let backoffRef = SessionRef(sessionID: approvalSessionID, registrationEpoch: 1)
        let backoffItems = [
            TranscriptItem(
                id: "backoff-1",
                .user("The gateway client reconnects every 5 seconds forever. Give it proper backoff.")
            )
        ]
        var backoff = Session(
            summary: SessionSummary(
                ref: backoffRef,
                title: "Add reconnect backoff",
                workspace: "~/code/rustcode",
                model: "demo-model",
                activity: .running
            ),
            state: SessionSnapshot(
                session: backoffRef,
                items: backoffItems,
                activeTurnID: "turn-backoff-1",
                backgroundTasks: [
                    BackgroundTask(id: "task-1", command: "cargo watch -x check", elapsed: .seconds(93))
                ],
                sequence: UInt64(backoffItems.count)
            )
        )
        backoff.startsLiveTurnOnAttach = true

        let storeRef = SessionRef(sessionID: questionSessionID, registrationEpoch: 1)
        let storeItems = [
            TranscriptItem(id: "store-1", .user("Refactor the session store so history can be paged.")),
            TranscriptItem(
                id: "store-2",
                .tool(ToolCall(
                    name: "search",
                    summary: "rg \"SessionStore\"",
                    input: "pattern: SessionStore\npath: engine/src",
                    output: "14 matches in 5 files",
                    status: .succeeded
                ))
            ),
            TranscriptItem(
                id: "store-3",
                .assistant(AssistantText(
                    text: "The store keeps every message in one JSON file. There are two reasonable ways to make it pageable, and they differ in migration cost. Which one do you want?"
                ))
            ),
        ]
        let question = PendingQuestion(
            id: "question-demo-1",
            header: "Storage",
            prompt: "How should the session store page its history?",
            options: [
                PendingQuestion.Option(
                    id: "segments",
                    label: "Segment files",
                    detail: "Split history into fixed-size files. No new dependency, simple migration."
                ),
                PendingQuestion.Option(
                    id: "sqlite",
                    label: "SQLite",
                    detail: "One database per session. Fast queries, but a new dependency."
                ),
                PendingQuestion.Option(
                    id: "index",
                    label: "Keep JSON, add an index",
                    detail: "Smallest change. Still rewrites the whole file on save."
                ),
            ],
            allowsFreeText: true
        )
        let store = Session(
            summary: SessionSummary(
                ref: storeRef,
                title: "Page the session store",
                workspace: "~/code/rustcode",
                model: "demo-model",
                activity: .waitingForUser,
                attention: SessionAttention(pendingQuestions: 1)
            ),
            state: SessionSnapshot(
                session: storeRef,
                items: storeItems,
                activeTurnID: "turn-store-1",
                pendingQuestion: question,
                sequence: UInt64(storeItems.count)
            )
        )

        let notesRef = SessionRef(sessionID: idleSessionID, registrationEpoch: 1)
        let notesItems = [
            TranscriptItem(id: "notes-failed", .user("Try the earlier draft"),
                turn: TurnTiming(turnID: "demo-failed", endedAt: "2026-10-09T18:39:00+02:00",
                    elapsedWorkMilliseconds: 8000, outcome: "failed")),
            TranscriptItem(id: "notes-3", .user("Draft release notes for everything merged since the last tag.")),
            TranscriptItem(id: "notes-phase", .assistant(AssistantText(text: "I’ll inspect the merged changes first.",
                responseTimeMilliseconds: 3000,
                turn: TurnTiming(turnID: "demo-notes", elapsedWorkMilliseconds: 32000, outcome: "completed")))),
            TranscriptItem(
                id: "notes-4",
                .subagent(SubagentSummary(
                    name: "explorer",
                    task: "Collect merged pull requests since the last tag",
                    status: .completed,
                    detail: "Read 23 commits and grouped them by scope."
                ))
            ),
            TranscriptItem(
                id: "notes-5",
                .tool(ToolCall(
                    name: "run_command",
                    summary: "git log --oneline v0.62.0..HEAD",
                    input: "git log --oneline v0.62.0..HEAD",
                    output: "23 commits",
                    status: .succeeded
                ))
            ),
            TranscriptItem(
                id: "notes-6",
                .assistant(AssistantText(
                    text: """
                        <think>I’ll group the merged changes by scope, then check the release steps.</think>

                        ## Release notes

                        **Engine**

                        - Sub-agents report progress while they run.
                        - Large `write_to_file` payloads are accepted and checked once complete.

                        **Fixes**

                        1. Turn metrics separate time spent waiting on you from tool time.
                        2. Chunked writes are verified after the last chunk.

                        Run `scripts/release.sh` when you are ready to tag.

                        | Area | Status |
                        | --- | --- |
                        | Engine | Ready |
                        | Fixes | Verified |

                        - [x] Review merged changes
                        - [ ] Tag the release
                        """,
                    responseTimeMilliseconds: 12000,
                    thoughtTimeMilliseconds: 420,
                    completedAt: "2026-10-09T18:40:32+02:00",
                    thoughtTokens: 96, thoughtTokensEstimated: true,
                    turn: TurnTiming(turnID: "demo-notes", startedAt: "2026-10-09T18:40:00+02:00",
                        endedAt: "2026-10-09T18:40:32+02:00", elapsedWorkMilliseconds: 32000,
                        outcome: "completed")
                ))
            ),
        ]
        let notes = Session(
            summary: SessionSummary(
                ref: notesRef,
                title: "Draft release notes",
                workspace: "~/code/rustcode",
                model: "demo-model"
            ),
            state: SessionSnapshot(
                session: notesRef,
                items: notesItems,
                sequence: UInt64(notesItems.count),
                historyCursor: HistoryCursor("demo-notes-older")
            ),
            olderHistory: [
                TranscriptItem(id: "notes-1", .user("What changed in the sandbox since last week?")),
                TranscriptItem(
                    id: "notes-2",
                    .assistant(AssistantText(text: "Only the network policy: outbound access is now off by default."))
                ),
            ]
        )

        return [backoff, store, notes]
    }
}
