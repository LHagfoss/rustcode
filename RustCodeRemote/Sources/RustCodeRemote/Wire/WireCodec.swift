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
                expectedGatewayID: payload.gatewayID
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
        case .subscribeSessions:
            return WireRequest(requestID: requestID.rawValue, operation: .subscribeSessions)
        case .attachSession(let session, let resume):
            let cursor = resume.map { WireResumeCursor(gatewayID: $0.gatewayID, lastSequence: $0.lastSequence) }
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
        case .getRequestStatus(_, let target):
            // Host-scoped on the wire: the protocol looks the receipt up by request ID alone.
            return WireRequest(
                requestID: requestID.rawValue,
                operation: .getRequestStatus(.init(targetRequestID: target.rawValue))
            )
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

    static func message(for frame: WireGatewayFrame) -> RemoteMessage {
        switch frame {
        case .paired(let paired):
            .handshake(.paired(PairingGrant(
                identity: ConnectionIdentity(gatewayID: paired.gatewayID, instanceID: paired.instanceID),
                device: DeviceIdentity(id: paired.deviceID, name: paired.deviceName),
                deviceToken: paired.token
            )))
        case .authenticated(let authenticated):
            .handshake(.authenticated(ConnectionIdentity(
                gatewayID: authenticated.gatewayID,
                instanceID: authenticated.instanceID
            )))
        case .error(let failure):
            .gatewayFailure(gatewayFailure(failure))
        case .unknown(let type, _):
            .unknown(type)
        }
    }

    static func gatewayFailure(_ failure: WireGatewayFrame.Failure) -> GatewayFailure {
        let message = failure.message
        switch failure.code.known {
        case .pairingFailed:
            return GatewayFailure(
                error: .pairingFailed("the code is wrong, expired or already used"),
                closesConnection: true
            )
        case .rateLimited:
            let retryAfter = failure.retryAfterSeconds.map { Duration.seconds(Int64(clamping: $0)) }
            return GatewayFailure(error: .rateLimited(retryAfter: retryAfter), closesConnection: true)
        case .unauthorized, .revoked:
            return GatewayFailure(error: .unauthorized, closesConnection: true)
        case .unsupportedVersion:
            return GatewayFailure(error: .incompatibleProtocol, closesConnection: true)
        case .notImplemented:
            return GatewayFailure(error: .notImplemented, closesConnection: false)
        case .busy:
            return GatewayFailure(error: .transport("The host has too many connections right now."), closesConnection: true)
        case .handshakeTimeout, .slowConsumer, .idleTimeout, .shuttingDown:
            return GatewayFailure(error: .transport("The host closed the connection: \(message)."), closesConnection: true)
        case .invalidFrame, .frameTooLarge, .internal:
            return GatewayFailure(
                error: .rejected(.other(code: failure.code.rawValue, message: message)),
                closesConnection: false
            )
        case nil:
            // A code from a newer gateway: report it, and let the socket say whether it closed.
            return GatewayFailure(
                error: .rejected(.other(code: failure.code.rawValue, message: message)),
                closesConnection: false
            )
        }
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
                return .failure(.outcomeUnknown)
            }
            return .failure(remoteError(error))
        case .sessions(let sessions):
            return .sessions(sessions.sessions.map(summary))
        case .attached(let attached):
            return .attached(.snapshot(snapshot(attached.snapshot)), gatewayID: attached.gatewayID)
        case .resumed(let resumed):
            return .attached(.resumed, gatewayID: resumed.gatewayID)
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
        case .promptAccepted, .turnCancelled, .questionAnswered, .approvalResolved:
            return .receipt(receipt == .received ? .received : .applied)
        case .requestStatus(let status):
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
        case .ownerUnavailable: return .outcomeUnknown
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
                .itemAppended(TranscriptItem(id: "prompt:\(frame.sequence)", .user(started.prompt.text)))
            ]
            if let turnID = started.turnID {
                changes.append(.turnStarted(turnID: turnID))
            }
            return update(changes)
        case .textDelta(let delta):
            return update([.responseDelta(delta.text)])
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
        case .turnFinished(let ended):
            return update([.turnFinished(turnID: ended.turnID, outcome: .completed)])
        case .turnCancelled(let ended):
            return update([.turnFinished(turnID: ended.turnID, outcome: .cancelled)])
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
            health: info.health.known == .live ? .healthy : .degraded
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
                    .assistant(AssistantText(text: turn.liveResponse.text, isStreaming: true))
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
            historyCursor: wire.historyCursor.map { HistoryCursor($0) }
        )
    }

    static func item(_ message: WireMessage) -> TranscriptItem {
        let text = message.content.text
        if let tool = message.tool {
            return TranscriptItem(id: message.messageID, .tool(ToolCall(
                name: tool.name,
                summary: tool.detail ?? "",
                output: text.isEmpty ? nil : text,
                status: tool.pending ? .running : tool.success ? .succeeded : .failed,
                isOutputTruncated: message.content.truncated
            )))
        }
        switch message.role {
        case "user": return TranscriptItem(id: message.messageID, .user(text))
        case "assistant": return TranscriptItem(id: message.messageID, .assistant(AssistantText(text: text)))
        // `role` is a free string on the wire; anything else is shown without a speaker.
        default: return TranscriptItem(id: message.messageID, .notice(text))
        }
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
            status: status
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
