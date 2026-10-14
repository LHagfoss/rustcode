import Foundation

/// `RemoteCodec` for remote protocol v1 and the gateway's handshake frames.
///
/// Translates between the `Wire…` DTOs and the domain types. It is stateless: everything a
/// later frame depends on (which request a reply answers, which gateway a cursor came from)
/// is tracked by `LiveRemoteClient`.
public struct WireCodec: RemoteCodec {
    /// The gateway closes a connection that sends a larger frame.
    public static let maximumOutgoingFrameBytes = 1024 * 1024
    /// The handshake frame has a tighter bound.
    public static let maximumHandshakeFrameBytes = 8 * 1024
    /// Port a gateway listens on unless told otherwise.
    public static let defaultPort = 17879

    public init() {}

    // MARK: Pairing and endpoint

    public func pairingTarget(for request: PairingRequest) throws -> PairingTarget {
        switch request {
        case .manual(let address, let code):
            let code = code.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !code.isEmpty else { throw RemoteError.pairingFailed("enter the code shown in the terminal") }
            return PairingTarget(address: address, credential: .shortCode(code))
        case .scanned(let text):
            guard let payload = try? JSONDecoder().decode(WirePairingPayload.self, from: Data(text.utf8)) else {
                throw RemoteError.pairingFailed("that is not a RustCode pairing code")
            }
            guard payload.protocolVersion == remoteProtocolVersion else {
                throw RemoteError.incompatibleProtocol
            }
            guard let address = try? HostAddress.parse(payload.address) else {
                throw RemoteError.pairingFailed("the pairing code names an address this app cannot use")
            }
            return PairingTarget(
                address: address,
                credential: .oneTime(payload.credential),
                expectedGatewayID: payload.gatewayID,
                hostName: payload.hostName
            )
        }
    }

    public func endpoint(for address: HostAddress) throws -> ConnectionEndpoint {
        var components = URLComponents()
        components.scheme = "ws"
        components.host = address.host.contains(":") ? "[\(address.host)]" : address.host
        components.port = address.port
        components.path = "/"
        guard let url = components.url else {
            throw RemoteError.transport("\(address) is not a usable address.")
        }
        // No headers: the token travels in the first frame, and the gateway refuses any
        // upgrade request that carries an Origin header.
        return ConnectionEndpoint(url: url)
    }

    // MARK: Encoding

    public func encode(_ command: RemoteCommand, requestID: RequestID) throws -> String {
        let limit = command.isHandshake ? Self.maximumHandshakeFrameBytes : Self.maximumOutgoingFrameBytes
        let frame: String
        switch command {
        case .pair(let credential, let deviceName):
            let (method, secret): (WirePairingMethod, String) = switch credential {
            case .oneTime(let secret): (.credential, secret)
            case .shortCode(let secret): (.code, secret)
            }
            frame = try Self.text(WireHandshakeRequest.pair(.init(
                protocolVersion: remoteProtocolVersion,
                method: WireOpen(method),
                secret: secret,
                deviceName: deviceName
            )))
        case .authenticate(let device, let deviceToken):
            frame = try Self.text(WireHandshakeRequest.authenticate(.init(
                protocolVersion: remoteProtocolVersion,
                deviceID: device.id,
                token: deviceToken
            )))
        default:
            frame = try Self.text(Self.request(for: command, requestID: requestID))
        }
        guard frame.utf8.count <= limit else {
            throw RemoteError.rejected(.other(code: "frame_too_large", message: "That is too long to send in one message."))
        }
        return frame
    }

    static func request(for command: RemoteCommand, requestID: RequestID) -> WireRequest {
        func scoped(_ session: SessionRef, _ operation: WireOperation) -> WireRequest {
            WireRequest(
                requestID: requestID.rawValue,
                sessionID: session.sessionID,
                registrationEpoch: session.registrationEpoch,
                operation: operation
            )
        }
        switch command {
        case .pair, .authenticate:
            preconditionFailure("handshake frames are not protocol requests")
        case .listDirectories(let path):
            return WireRequest(requestID: requestID.rawValue, operation: .listDirectories(.init(path: path)))
        case .createSession(let path):
            return WireRequest(requestID: requestID.rawValue, operation: .createSession(.init(path: path)))
        case .subscribeSessions:
            return WireRequest(requestID: requestID.rawValue, operation: .subscribeSessions)
        case .attachSession(let session, let resume):
            let cursor = resume.map {
                WireResumeCursor(
                    snapshotID: $0.snapshotID,
                    gatewayID: $0.origin.gatewayID,
                    instanceID: $0.origin.instanceID,
                    lastSequence: $0.lastSequence
                )
            }
            return scoped(session, .attachSession(.init(resume: cursor)))
        case .detachSession(let session):
            return scoped(session, .detachSession)
        case .getHistory(let session, let cursor, let limit):
            return scoped(session, .getHistory(.init(cursor: cursor.rawValue, limit: UInt32(clamping: limit))))
        case .getContent(let session, let contentID, let offset, let maxBytes):
            return scoped(session, .getContent(.init(
                contentID: contentID,
                offset: offset,
                maxBytes: UInt32(clamping: maxBytes)
            )))
        case .executeCommand(let session, let command):
            return scoped(session, .executeCommand(.init(command: command)))
        case .cancelQuestion(let session, let questionID):
            return scoped(session, .cancelQuestion(.init(questionID: questionID)))
        case .setSessionSettings(let session, let model, let effort):
            return scoped(session, .setSessionSettings(.init(model: model, reasoningEffort: effort)))
        case .uploadImage(let session, let chunk):
            return scoped(session, .uploadImage(chunk))
        case .submitPrompt(let session, let text):
            return scoped(session, .submitPrompt(.init(prompt: text)))
        case .steer(let session, let text):
            return scoped(session, .steer(.init(prompt: text)))
        case .queue(let session, let text):
            return scoped(session, .queue(.init(prompt: text)))
        case .cancelTurn(let session, let turnID):
            return scoped(session, .cancelTurn(.init(turnID: turnID)))
        case .answerQuestion(let session, let questionID, let answer):
            return scoped(session, .answerQuestion(.init(questionID: questionID, answer: Self.answer(answer))))
        case .resolveApproval(let session, let batchID, let decision):
            return scoped(session, .resolveApproval(.init(
                batchID: batchID,
                choice: WireOpen(decision == .approve ? .approve : .deny)
            )))
        case .getRequestStatus(let session, let target):
            // Resolved by device and request ID; naming the session asks exactly the
            // terminal that would hold the receipt.
            return scoped(session, .getRequestStatus(.init(targetRequestID: target.rawValue)))
        case .requestShare(let sessionID):
            // The session is not shared yet, so it has no registration to scope to.
            return WireRequest(requestID: requestID.rawValue, operation: .requestShare(.init(sessionID: sessionID)))
        }
    }

    /// The wire answer is either a selection or free text, never both. Text the user typed
    /// wins, because it is the more specific statement.
    static func answer(_ answer: QuestionAnswer) -> WireAnswer {
        let text = (answer.text ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        if !text.isEmpty {
            return .custom(.init(text: text))
        }
        return .selected(.init(options: answer.selectedOptionIDs))
    }

    // MARK: Decoding

    public func decode(_ frame: String) throws -> RemoteMessage {
        let data = Data(frame.utf8)
        let tags: Tags
        do {
            tags = try JSONDecoder().decode(Tags.self, from: data)
        } catch {
            throw RemoteError.invalidFrame("not a JSON object")
        }
        do {
            if tags.kind != nil {
                return Self.message(for: try JSONDecoder().decode(WireFrame.self, from: data))
            }
            if tags.type != nil {
                return Self.message(for: try JSONDecoder().decode(WireGatewayFrame.self, from: data))
            }
        } catch {
            // Never quote the frame: it can carry a token or transcript text.
            throw RemoteError.invalidFrame("a \(tags.kind ?? tags.type ?? "?") frame did not match the protocol")
        }
        throw RemoteError.invalidFrame("frame has neither kind nor type")
    }

    private struct Tags: Decodable {
        var kind: String?
        var type: String?
    }

    static func text(_ value: some Encodable) throws -> String {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.withoutEscapingSlashes]
        return String(decoding: try encoder.encode(value), as: UTF8.self)
    }

    // MARK: Gateway frames

    /// A date the host could not have meant is dropped, not guessed at.
    private static func linkExpiry(_ text: String?) -> Date? {
        text.flatMap { try? Date($0, strategy: .iso8601) }
    }

    static func message(for frame: WireGatewayFrame) -> RemoteMessage {
        switch frame {
        case .paired(let paired):
            .handshake(.paired(PairingGrant(
                identity: ConnectionIdentity(
                    gatewayID: paired.gatewayID,
                    instanceID: paired.instanceID,
                    hostName: paired.hostName,
                    linkExpiresAt: linkExpiry(paired.expiresAt)
                ),
                device: DeviceIdentity(id: paired.deviceID, name: paired.deviceName),
                deviceToken: paired.token
            )))
        case .authenticated(let authenticated):
            .handshake(.authenticated(ConnectionIdentity(
                gatewayID: authenticated.gatewayID,
                instanceID: authenticated.instanceID,
                hostName: authenticated.hostName,
                linkExpiresAt: linkExpiry(authenticated.expiresAt)
            )))
        case .error(let failure):
            .gatewayFailure(gatewayFailure(failure))
        case .unknown(let type, _):
            .unknown(type)
        }
    }

    static func gatewayFailure(_ failure: WireGatewayFrame.Failure) -> GatewayFailure {
        let message = failure.message
        let error: RemoteError = switch failure.code.known {
        case .pairingFailed:
            .pairingFailed("the code is wrong, expired or already used")
        case .rateLimited:
            .rateLimited(retryAfter: failure.retryAfterSeconds.map { Duration.seconds(Int64(clamping: $0)) })
        case .unauthorized, .revoked:
            .unauthorized
        case .unsupportedVersion:
            .incompatibleProtocol
        case .busy:
            .transport("The host has too many connections right now.")
        case .handshakeTimeout, .slowConsumer, .idleTimeout, .shuttingDown, .invalidFrame, .frameTooLarge,
             .internal, .notImplemented:
            .transport("The host closed the connection: \(message).")
        case nil:
            // A code from a newer gateway. Whatever it means, the connection is going away.
            .transport("The host closed the connection: \(message) (\(failure.code.rawValue)).")
        }
        return GatewayFailure(error: error)
    }

    // MARK: Protocol frames

    static func message(for frame: WireFrame) -> RemoteMessage {
        switch frame {
        case .response(let response):
            .reply(requestID: RequestID(response.requestID), reply(for: response))
        case .sessions(let sessions):
            .sessionsChanged(sessions.sessions.map(summary))
        case .event(let event):
            message(for: event)
        case .unknown(let kind, _):
            .unknown(kind)
        }
    }

    static func reply(for response: WireResponse) -> RemoteReply {
        let receipt = response.receipt?.known
        switch response.result {
        case .error(let error):
            // `unknown` means the owner could not be asked. That is not a rejection.
            if receipt == .unknown || error.code.known == .ownerUnavailable {
                return .failure(.outcomeUnknown(detail: error.message))
            }
            return .failure(remoteError(error))
        case .imageUploaded(let progress):
            return receipt == .applied ? .imageUploaded(progress) : .receipt(receipt == .received ? .received : .unknown)
        case .directories(let listing): return .directories(listing)
        case .sessionCreated(let created):
            return receipt == .applied ? .sessionCreated(summary(created.session)) : .receipt(receipt == .received ? .received : .unknown)
        case .sessions(let sessions):
            return .sessions(sessions.sessions.map(summary))
        case .attached(let attached):
            // With or without `resync`, a snapshot replaces whatever the client held.
            return .attached(
                .snapshot(snapshot(attached.snapshot)),
                origin: ReplayOrigin(gatewayID: attached.gatewayID, instanceID: attached.instanceID)
            )
        case .resumed(let resumed):
            return .attached(
                .resumed,
                origin: ReplayOrigin(gatewayID: resumed.gatewayID, instanceID: resumed.instanceID)
            )
        case .detached:
            return .detached
        case .history(let page):
            return .history(HistoryPage(
                items: page.messages.map(item),
                nextCursor: page.nextCursor.map { HistoryCursor($0) }
            ))
        case .content(let chunk):
            return .content(ContentChunk(
                contentID: chunk.contentID,
                offset: chunk.offset,
                text: chunk.text,
                totalBytes: chunk.totalBytes,
                nextOffset: chunk.nextOffset
            ))
        case .commandExecuted(let result):
            return receipt == .applied ? .commandExecuted(result) : .receipt(receipt == .received ? .received : .unknown)
        case .questionCancelled, .sessionSettingsUpdated, .promptAccepted, .turnCancelled, .questionAnswered, .approvalResolved:
            return .receipt(receipt == .unknown ? .unknown : receipt == .received ? .received : .applied)
        case .shareRequestSent(let sent):
            return .shareRequestSent(pendingID: sent.pendingID)
        case .shareAccepted(let accepted):
            return .shareAccepted(summary(accepted.session))
        case .shareDenied(let denied):
            return .shareDenied(reason: denied.reason)
        case .requestStatus(let status):
            if status.receipt.known == .applied, case .commandExecuted(let result) = status.result { return .commandExecuted(result) }
            if case .shareAccepted(let accepted) = status.result { return .shareAccepted(summary(accepted.session)) }
            if case .shareDenied(let denied) = status.result { return .shareDenied(reason: denied.reason) }
            return .receipt(receiptState(status))
        case .unknown(let type, _):
            return .unknown(type)
        }
    }

    static func receiptState(_ status: WireResult.RequestStatus) -> ReceiptState {
        switch status.receipt.known {
        case .received: return .received
        case .applied: return .applied
        case .unknown, nil: return .unknown
        case .rejected:
            if case .error(let error) = status.result, case .rejected(let rejection) = remoteError(error) {
                return .rejected(rejection)
            }
            return .rejected(.other(code: "rejected", message: nil))
        }
    }

    static func remoteError(_ error: WireError) -> RemoteError {
        func other() -> RemoteError {
            .rejected(.other(code: error.code.rawValue, message: error.message))
        }
        switch error.code.known {
        case .incompatibleVersion: return .incompatibleProtocol
        case .unauthorized: return .unauthorized
        case .rateLimited: return .rateLimited(retryAfter: nil)
        case .ownerUnavailable: return .outcomeUnknown(detail: error.message)
        case .busy: return .rejected(.busy)
        case .notRunning: return .rejected(.notRunning)
        case .staleTurn: return .rejected(.staleTurn)
        case .staleQuestion: return .rejected(.staleQuestion)
        case .invalidAnswer: return .rejected(.invalidAnswer)
        case .staleApproval: return .rejected(.staleApproval)
        case .staleSession: return .rejected(.staleSession)
        case .unsupportedOperation: return .rejected(.unsupportedOperation)
        case .invalidRequest, .notFound, .staleCursor, .requestConflict, .receiptCapacity, .frameTooLarge, .internal:
            return other()
        case nil:
            return other()
        }
    }

    // MARK: Events

    static func message(for frame: WireEventFrame) -> RemoteMessage {
        let session = SessionRef(sessionID: frame.sessionID, registrationEpoch: frame.registrationEpoch)
        func update(_ changes: [SessionEvent]) -> RemoteMessage {
            .sessionUpdate(SessionUpdate(session: session, sequence: frame.sequence, changes: changes))
        }
        switch frame.event {
        case .turnStarted(let started):
            // The prompt belongs to the transcript; the event is the only place it arrives.
            var changes: [SessionEvent] = [
                .itemAppended(TranscriptItem(id: "prompt:\(frame.sequence)", .user(started.prompt.text),
                    turn: started.timing.map(timing) ?? started.turnID.map { TurnTiming(turnID: $0) }))
            ]
            if let turnID = started.turnID ?? started.timing?.turnID {
                changes.append(.turnStarted(turnID: turnID))
            }
            if let timing = started.timing {
                changes.append(.timingChanged(Self.timing(timing), thoughtTimeMilliseconds: nil))
            }
            return update(changes)
        case .textDelta(let delta):
            var changes: [SessionEvent] = delta.text.isEmpty ? [] : [.responseDelta(delta.text)]
            if let timing = delta.timing {
                changes.append(.timingChanged(Self.timing(timing), thoughtTimeMilliseconds: delta.thoughtTimeMilliseconds))
            } else if let thought = delta.thoughtTimeMilliseconds {
                changes.append(.thoughtTimeChanged(thought))
            }
            if let tokens = delta.thoughtTokens { changes.append(.thoughtTokensChanged(tokens, estimated: delta.thoughtTokensEstimated)) }
            return update(changes)
        case .toolStarted(let started):
            return update([.itemAppended(toolItem(started.tool))])
        case .toolFinished(let finished):
            return update([.toolFinished(
                itemID: toolItemID(finished.id),
                succeeded: finished.success,
                isPending: finished.pending,
                output: finished.content.text,
                isOutputTruncated: finished.content.truncated
            )])
        case .subagentUpdated(let updated):
            return update([.subagentUpdated(subagentItem(updated.subagent))])
        case .approvalRequested(let requested):
            return update([.approvalChanged(approval(requested.approval))])
        case .approvalResolved(let resolved):
            return update([.approvalResolved(batchID: resolved.batchID)])
        case .questionRequested(let requested):
            return update([.questionChanged(question(requested.question))])
        case .questionResolved(let resolved):
            return update([.questionResolved(questionID: resolved.questionID)])
        case .turnFinished(let ended), .turnCancelled(let ended):
            let fallback: TurnOutcome = frame.event.type == "turn_cancelled" ? .cancelled : .completed
            let outcome: TurnOutcome = switch ended.timing?.outcome {
            case "failed": .failed
            case "cancelled": .cancelled
            case "completed": .completed
            case .some(let value): .unknown(value)
            case nil: fallback
            }
            var changes = ended.timing.map { [SessionEvent.timingChanged(Self.timing($0), thoughtTimeMilliseconds: nil)] } ?? []
            changes.append(.turnFinished(turnID: ended.turnID ?? ended.timing?.turnID, outcome: outcome))
            return update(changes)
        case .snapshot(let replaced):
            return update([.replaced(snapshot(replaced.snapshot))])
        case .resyncRequired:
            return .resyncRequired(session)
        case .sessionClosed(let closed):
            return .sessionClosed(session, reason: closeReason(closed.reason))
        case .unknown:
            // Keeps its place in the sequence, so the events after it still apply.
            return update([])
        }
    }

    static func closeReason(_ reason: WireOpen<WireSessionCloseReason>) -> String {
        switch reason.known {
        case .sharingDisabled: "Sharing was turned off for this session."
        case .sessionChanged: "The terminal moved on to another session."
        case .ownerExited: "The terminal that owned this session was closed."
        case .deviceRevoked: "This device was revoked on the host."
        case nil: "The session is no longer shared."
        }
    }

    // MARK: Model mapping

    static func summary(_ info: WireSessionInfo) -> SessionSummary {
        let activity: SessionActivity = switch info.activity.known {
        case .idle: .idle
        case .running: .running
        case .awaitingApproval, .awaitingQuestion: .waitingForUser
        // An activity this version does not know is at least not idle.
        case nil: .running
        }
        return SessionSummary(
            ref: SessionRef(sessionID: info.sessionID, registrationEpoch: info.registrationEpoch),
            title: info.title,
            workspace: info.workspace ?? "",
            model: info.model,
            activity: activity,
            attention: SessionAttention(
                pendingQuestions: info.attention.question ? 1 : 0,
                pendingApprovals: info.attention.approval ? 1 : 0
            ),
            health: info.health.known == .live ? .healthy : .degraded,
            turnCount: info.turnCount,
            lastActiveAt: info.lastActiveAt.flatMap(TimingPresentation.date)
        )
    }

    static func snapshot(_ wire: WireSnapshot) -> SessionSnapshot {
        var items = wire.transcript.map(item)
        if let turn = wire.turn {
            // The snapshot does not say how the turn's tools and its response text interleave.
            items.append(contentsOf: turn.tools.map(toolItem))
            if !turn.liveResponse.text.isEmpty {
                items.append(TranscriptItem(
                    id: "response:\(turn.turnID)",
                    .assistant(AssistantText(text: turn.liveResponse.text, isStreaming: true,
                        thoughtTimeMilliseconds: turn.thoughtTimeMilliseconds,
                        thoughtTokens: turn.thoughtTokens, thoughtTokensEstimated: turn.thoughtTokensEstimated,
                        turn: turn.timing.map(timing) ?? TurnTiming(turnID: turn.turnID)))
                ))
            }
        }
        return SessionSnapshot(
            session: SessionRef(
                sessionID: wire.session.sessionID,
                registrationEpoch: wire.session.registrationEpoch
            ),
            items: items,
            activeTurnID: wire.turn?.turnID,
            canSteer: wire.turn?.canSteer ?? true,
            pendingQuestion: wire.pendingQuestion.map(question),
            pendingApproval: wire.pendingApproval.map(approval),
            subagents: wire.subagents.map(subagentItem),
            backgroundTasks: wire.backgroundTasks.map {
                BackgroundTask(
                    id: $0.id,
                    command: $0.command,
                    elapsed: .milliseconds(Int64(clamping: $0.elapsedMilliseconds))
                )
            },
            pendingPrompts: wire.pendingPrompts.map {
                PendingPrompt(kind: $0.kind.known == .steer ? .steer : .queue, text: $0.text.text)
            },
            omitted: OmittedCounts(
                tools: Int(wire.omitted.tools),
                subagents: Int(wire.omitted.subagents),
                pendingPrompts: Int(wire.omitted.pendingPrompts),
                backgroundTasks: Int(wire.omitted.backgroundTasks)
            ),
            sequence: wire.sequence,
            historyCursor: wire.historyCursor.map { HistoryCursor($0) },
            activeTurnTiming: wire.turn?.timing.map(timing),
            lastTurn: wire.lastTurn.map(timing),
            snapshotID: wire.snapshotID,
            settings: wire.settings.map { $0.domain },
            capabilities: Set(wire.capabilities ?? []),
            commands: wire.commands ?? []
        )
    }

    static func item(_ message: WireMessage) -> TranscriptItem {
        let text = message.content.text
        func entry(_ content: TranscriptItem.Content) -> TranscriptItem {
            TranscriptItem(id: message.messageID, content, turn: message.turn.map(timing),
                timestamp: message.timestamp, metadata: message.metadata)
        }
        if let tool = message.tool {
            return entry(.tool(ToolCall(
                name: tool.name,
                summary: tool.detail ?? "",
                output: text.isEmpty ? nil : text,
                status: tool.pending ? .running : tool.success ? .succeeded : .failed,
                isOutputTruncated: message.content.truncated,
                callID: tool.callID, isTranscriptRecord: true
            )))
        }
        switch message.role {
        case "user": return entry(.user(text))
        case "assistant": return entry(.assistant(AssistantText(
            text: text, responseTimeMilliseconds: message.responseTimeMilliseconds,
            thoughtTimeMilliseconds: message.thoughtTimeMilliseconds, timestamp: message.timestamp,
            completedAt: message.completedAt, thoughtTokens: message.thoughtTokens, thoughtTokensEstimated: message.thoughtTokensEstimated, turn: message.turn.map(timing), metadata: message.metadata
        )))
        // `role` is a free string on the wire; anything else is shown without a speaker.
        default: return entry(.notice(text))
        }
    }

    static func timing(_ wire: WireTurnTiming) -> TurnTiming {
        TurnTiming(turnID: wire.turnID, startedAt: wire.startedAt, endedAt: wire.endedAt,
                   elapsedWorkMilliseconds: wire.elapsedWorkMilliseconds, outcome: wire.outcome,
                   metadata: wire.metadata)
    }

    static func toolItemID(_ id: String) -> String { "tool:\(id)" }

    static func toolItem(_ tool: WireTool) -> TranscriptItem {
        let status: ToolCall.Status = switch tool.state.known {
        case .succeeded: .succeeded
        case .failed: .failed
        case .preparing, .running, nil: .running
        }
        return TranscriptItem(id: toolItemID(tool.id), .tool(ToolCall(
            name: tool.name,
            summary: tool.detail ?? "",
            status: status, callID: tool.id
        )))
    }

    static func subagentItem(_ subagent: WireSubagent) -> TranscriptItem {
        let status: SubagentSummary.Status = switch subagent.status.known {
        case .queued: .queued
        case .running, nil: .running
        case .interrupted: .interrupted
        case .completed: .completed
        case .failed: .failed
        case .cancelled: .cancelled
        }
        let messages = subagent.messageCount == 1 ? "1 message" : "\(subagent.messageCount) messages"
        return TranscriptItem(id: "subagent:\(subagent.id)", .subagent(SubagentSummary(
            name: subagent.name,
            task: subagent.task.text,
            status: status,
            detail: [subagent.model, messages].compactMap { $0 }.joined(separator: " · ")
        )))
    }

    static func question(_ question: WireQuestion) -> PendingQuestion {
        PendingQuestion(
            id: question.questionID,
            header: question.header,
            prompt: question.text,
            // Options have no identity on the wire other than their label, and an answer
            // names them by label.
            options: question.options.map {
                PendingQuestion.Option(id: $0.label, label: $0.label, detail: $0.description)
            },
            allowsMultipleSelection: question.multiple,
            // Every question accepts a custom answer; the wire has no flag to say otherwise.
            allowsFreeText: true,
            position: Int(question.position),
            chainLength: Int(question.chainLength)
        )
    }

    static func approval(_ batch: WireApprovalBatch) -> PendingApproval {
        PendingApproval(
            batchID: batch.batchID,
            items: batch.actions.enumerated().map { index, action in
                PendingApproval.Item(
                    id: "\(batch.batchID)#\(index)",
                    toolName: action.toolName,
                    title: action.summary,
                    risk: action.risk,
                    detail: action.details.text,
                    isDetailComplete: !action.details.truncated,
                    detailContentID: action.details.contentID,
                    detailTotalBytes: action.details.totalBytes
                )
            }
        )
    }
}
