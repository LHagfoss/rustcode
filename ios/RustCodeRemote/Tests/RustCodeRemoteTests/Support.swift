import Foundation
@testable import RustCodeRemote

func makeSession(_ id: String, epoch: UInt64 = 1, activity: SessionActivity = .idle, attention: SessionAttention = .init()) -> SessionSummary {
    SessionSummary(
        ref: SessionRef(sessionID: id, registrationEpoch: epoch),
        title: "Session \(id)",
        workspace: "~/work",
        model: "test-model",
        activity: activity,
        attention: attention
    )
}

func makeCredential(token: String = "token") -> StoredCredential {
    StoredCredential(
        host: PairedHost(
            gatewayID: "gateway",
            name: "Test Mac",
            address: HostAddress(host: "192.168.1.20", port: 7878),
            device: DeviceIdentity(id: "device", name: "Test Phone"),
            pairedAt: Date(timeIntervalSince1970: 0)
        ),
        deviceToken: token
    )
}

func update(_ session: SessionRef, _ sequence: UInt64, _ event: SessionEvent) -> SessionUpdate {
    SessionUpdate(session: session, sequence: sequence, event: event)
}

/// Polls a main-actor condition; returns false if it does not hold in time.
@MainActor
func waitUntil(timeout: Duration = .seconds(5), _ condition: @MainActor () async -> Bool) async -> Bool {
    let deadline = ContinuousClock.now + timeout
    while await !condition() {
        if ContinuousClock.now > deadline { return false }
        try? await Task.sleep(for: .milliseconds(2))
    }
    return true
}

/// A codec for tests only. Its line format has nothing to do with the real protocol; it
/// exists so the client's plumbing can be exercised end to end over a fake socket.
///
/// Outgoing: `<request id>|<operation>`; the handshake uses the request ID `handshake`.
/// Incoming: `reply|<id>|<kind>[|<arg>]`, `sessions|<count>`, `update|<session>|<sequence>|<text>`,
/// `resync|<session>`, `closing|unauthorized`.
struct LineCodec: RemoteCodec {
    static let session = SessionRef(sessionID: "s1", registrationEpoch: 1)

    func pairingTarget(for request: PairingRequest) throws -> PairingTarget {
        switch request {
        case .manual(let address, let code):
            PairingTarget(address: address, credential: .shortCode(code), expectedGatewayID: code == "QR" ? "expected-gateway" : nil)
        case .scanned: throw RemoteError.pairingFailed("not a pairing code")
        }
    }

    func endpoint(for address: HostAddress) throws -> ConnectionEndpoint {
        ConnectionEndpoint(url: URL(string: "ws://\(address)/test")!)
    }

    func encode(_ command: RemoteCommand, requestID: RequestID) throws -> String {
        let operation = switch command {
        case .uploadImage: "upload_image"
        case .pair: "pair"
        case .authenticate: "authenticate"
        case .subscribeSessions: "subscribe"
        case .listDirectories: "directories"
        case .createSession: "create-session"
        case .attachSession(_, let resume):
            resume.map { "attach:\($0.origin.gatewayID)/\($0.origin.instanceID ?? "-")@\($0.lastSequence)" } ?? "attach"
        case .detachSession: "detach"
        case .getHistory: "history"
        case .getContent: "content"
        case .executeCommand: "command"
        case .cancelQuestion: "cancel-question"
        case .setSessionSettings: "settings"
        case .submitPrompt: "submit"
        case .steer: "steer"
        case .queue: "queue"
        case .cancelTurn: "cancel"
        case .answerQuestion: "answer"
        case .resolveApproval: "approve"
        case .getRequestStatus(_, let target): "status:\(target)"
        case .requestShare: "share"
        case .listSavedSessions(let cursor, _, let query): "saved:\(cursor ?? "-"):\(query ?? "-")"
        case .resumeSession(let sessionID): "resume:\(sessionID)"
        }
        return "\(requestID)|\(operation)"
    }

    func decode(_ frame: String) throws -> RemoteMessage {
        let parts = frame.split(separator: "|", omittingEmptySubsequences: false).map(String.init)
        switch parts.first {
        case "reply" where parts.count >= 3:
            return .reply(requestID: RequestID(parts[1]), try reply(Array(parts[2...])))
        case "sessions" where parts.count == 2:
            return .sessionsChanged(sessions(parts[1]))
        case "update" where parts.count == 4:
            guard let sequence = UInt64(parts[2]) else { throw RemoteError.invalidFrame(frame) }
            let ref = SessionRef(sessionID: parts[1], registrationEpoch: 1)
            return .sessionUpdate(SessionUpdate(session: ref, sequence: sequence, event: .assistantDelta(itemID: "a", text: parts[3])))
        case "snapshot" where parts.count == 3:
            guard let sequence = UInt64(parts[2]) else { throw RemoteError.invalidFrame(frame) }
            let ref = SessionRef(sessionID: parts[1], registrationEpoch: 1)
            return .sessionUpdate(SessionUpdate(
                session: ref,
                sequence: sequence,
                event: .replaced(SessionSnapshot(session: ref, sequence: sequence))
            ))
        case "resync" where parts.count == 2:
            return .resyncRequired(SessionRef(sessionID: parts[1], registrationEpoch: 1))
        case "closing":
            return .gatewayFailure(GatewayFailure(error: .unauthorized))
        case "gateway-error" where parts.count == 2:
            return .gatewayFailure(GatewayFailure(error: .transport(parts[1])))
        case "future":
            return .unknown("future")
        default:
            throw RemoteError.invalidFrame(frame)
        }
    }

    private func sessions(_ count: String) -> [SessionSummary] {
        (0..<(Int(count) ?? 0)).map { makeSession("s\($0 + 1)") }
    }

    private func reply(_ parts: [String]) throws -> RemoteReply {
        switch (parts[0], parts.count > 1 ? parts[1] : "") {
        case ("paired", let gateway): .handshake(.paired(PairingGrant(
            identity: ConnectionIdentity(gatewayID: gateway.isEmpty ? "gateway" : gateway, instanceID: "instance-1"),
            device: DeviceIdentity(id: "device", name: "Phone"),
            deviceToken: "fresh-token"
        )))
        case ("authenticated", let gateway): .handshake(.authenticated(
            ConnectionIdentity(gatewayID: gateway.isEmpty ? "gateway" : gateway, instanceID: "instance-1")
        ))
        case ("sessions", let count): .sessions(sessions(count))
        case ("sessions-saved", let count): .sessions(sessions(count), capabilities: [HostCapability.savedSessions])
        case ("attached", _): .attached(
            .snapshot(SessionSnapshot(session: Self.session, sequence: 10)),
            origin: ReplayOrigin(gatewayID: "gw-1", instanceID: "inst-1")
        )
        case ("resumed", _): .attached(.resumed, origin: ReplayOrigin(gatewayID: "gw-1", instanceID: "inst-1"))
        case ("content", let text): .content(ContentChunk(contentID: "c", offset: 0, text: text, totalBytes: 1, nextOffset: nil))
        case ("receipt-unknown", _): .failure(.outcomeUnknown())
        case ("mystery", _): .unknown("mystery")
        case ("detached", _): .detached
        case ("receipt", "received"): .receipt(.received)
        case ("receipt", "applied"): .receipt(.applied)
        case ("receipt", "unknown"): .receipt(.unknown)
        case ("busy", _): .failure(.rejected(.busy))
        case ("unauthorized", _): .failure(.unauthorized)
        default: throw RemoteError.invalidFrame(parts.joined(separator: "|"))
        }
    }
}

/// Answers each sent frame according to its operation. `replies` maps an operation to the
/// reply bodies to send back, each of which is prefixed with `reply|<id>|`.
func scriptedResponder(_ replies: [String: [String]]) -> ScriptedTransport.Responder {
    { frame in
        let parts = frame.split(separator: "|", maxSplits: 1).map(String.init)
        guard parts.count == 2 else { return [] }
        let operation = parts[1].split(separator: ":").first.map(String.init) ?? parts[1]
        return (replies[parts[1]] ?? replies[operation] ?? []).map { $0.hasPrefix("!") ? String($0.dropFirst()) : "reply|\(parts[0])|\($0)" }
    }
}
