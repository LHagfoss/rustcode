import Foundation
import Testing
@testable import RustCodeRemote

/// The contract in this repository's `docs/remote-protocol/v1`, read in place so
/// the app is always tested against the host it ships with.
enum Contract {
    static let root = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent()  // RustCodeRemoteTests
        .deletingLastPathComponent()  // Tests
        .deletingLastPathComponent()  // RustCodeRemote
        .deletingLastPathComponent()  // ios
        .deletingLastPathComponent()  // repository root
        .appendingPathComponent("docs/remote-protocol/v1")

    static func goldenFiles(_ directory: String) throws -> [URL] {
        try FileManager.default
            .contentsOfDirectory(at: root.appendingPathComponent("golden/\(directory)"), includingPropertiesForKeys: nil)
            .filter { $0.pathExtension == "json" }
            .sorted { $0.lastPathComponent < $1.lastPathComponent }
    }

    static func json(_ data: Data) throws -> NSObject {
        try #require(try JSONSerialization.jsonObject(with: data, options: [.fragmentsAllowed]) as? NSObject)
    }

    static func schema(_ name: String) throws -> [String: Any] {
        let data = try Data(contentsOf: root.appendingPathComponent("\(name).schema.json"))
        return try #require(try JSONSerialization.jsonObject(with: data) as? [String: Any])
    }

    /// Every string a schema definition allows: plain `enum` lists and `const` alternatives,
    /// optionally the `const` of one property of each `oneOf` alternative (a tag).
    static func strings(in definition: Any?, tag: String? = nil) -> Set<String> {
        guard let definition = definition as? [String: Any] else { return [] }
        var found = Set<String>()
        if tag == nil {
            if let values = definition["enum"] as? [String] { found.formUnion(values) }
            if let value = definition["const"] as? String { found.insert(value) }
        } else if let tag, let properties = definition["properties"] as? [String: Any],
                  let property = properties[tag] as? [String: Any], let value = property["const"] as? String {
            found.insert(value)
        }
        for alternative in definition["oneOf"] as? [Any] ?? [] {
            found.formUnion(strings(in: alternative, tag: tag))
        }
        return found
    }

    static func definition(_ name: String, in schema: [String: Any]) -> Any? {
        (schema["$defs"] as? [String: Any])?[name]
    }
}

/// The same round trip the Rust side runs: decode every golden file, encode it again and
/// compare as JSON.
@Suite struct GoldenFrameTests {
    @Test func everyGoldenRequestRoundTrips() throws {
        let files = try Contract.goldenFiles("requests")
        #expect(files.count == 25)
        var operations = Set<String>()
        for file in files {
            let data = try Data(contentsOf: file)
            let request = try JSONDecoder().decode(WireRequest.self, from: data)
            let encoded = try JSONEncoder().encode(request)
            #expect(try Contract.json(encoded) == Contract.json(data), "\(file.lastPathComponent) does not round-trip")
            if case .unknown(let type, _) = request.operation {
                Issue.record("\(file.lastPathComponent): operation \(type) is not modelled")
            }
            operations.insert(request.operation.type)
        }
        // Every operation this build knows has a golden request, and the other way round.
        #expect(operations == WireOperation.knownTypes)
    }

    @Test func everyGoldenFrameRoundTrips() throws {
        let files = try Contract.goldenFiles("frames")
        #expect(files.count == 58)
        var kinds = Set<String>()
        var results = Set<String>()
        var events = Set<String>()
        var errorCodes = Set<String>()
        for file in files {
            let data = try Data(contentsOf: file)
            let frame = try JSONDecoder().decode(WireFrame.self, from: data)
            let encoded = try JSONEncoder().encode(frame)
            #expect(try Contract.json(encoded) == Contract.json(data), "\(file.lastPathComponent) does not round-trip")

            switch frame {
            case .response(let response):
                kinds.insert("response")
                results.insert(response.result.type)
                if case .unknown(let type, _) = response.result {
                    Issue.record("\(file.lastPathComponent): result \(type) is not modelled")
                }
                if case .error(let error) = response.result {
                    #expect(error.code.known != nil, "\(file.lastPathComponent): unknown error code")
                    errorCodes.insert(error.code.rawValue)
                }
                if case .requestStatus(let status) = response.result, let nested = status.result {
                    results.insert(nested.type)
                }
            case .event(let event):
                kinds.insert("event")
                events.insert(event.event.type)
                if case .unknown(let type, _) = event.event {
                    Issue.record("\(file.lastPathComponent): event \(type) is not modelled")
                }
            case .sessions:
                kinds.insert("sessions")
            case .unknown(let kind, _):
                Issue.record("\(file.lastPathComponent): frame kind \(kind) is not modelled")
            }
        }
        #expect(kinds == WireFrame.knownKinds)
        #expect(results == WireResult.knownTypes)
        #expect(events == WireEvent.knownTypes)
        // The golden set does not exercise every error code; the schema test covers the rest.
        #expect(errorCodes.isSubset(of: Set(WireErrorCode.allCases.map(\.rawValue))))
    }

    @Test func everyGoldenHandshakeFrameRoundTrips() throws {
        let requests = try Contract.goldenFiles("handshake/requests")
        #expect(requests.count == 4)
        var methods = Set<String>()
        var requestTypes = Set<String>()
        for file in requests {
            let data = try Data(contentsOf: file)
            let request = try JSONDecoder().decode(WireHandshakeRequest.self, from: data)
            #expect(try Contract.json(JSONEncoder().encode(request)) == Contract.json(data), "\(file.lastPathComponent) does not round-trip")
            switch request {
            case .pair(let pair):
                requestTypes.insert("pair")
                #expect(pair.method.known != nil)
                methods.insert(pair.method.rawValue)
            case .authenticate:
                requestTypes.insert("authenticate")
            }
        }
        #expect(requestTypes == ["pair", "authenticate"])
        #expect(methods == Set(WirePairingMethod.allCases.map(\.rawValue)))

        let responses = try Contract.goldenFiles("handshake/responses")
        #expect(responses.count == 17)
        var responseTypes = Set<String>()
        var codes = Set<String>()
        for file in responses {
            let data = try Data(contentsOf: file)
            let response = try JSONDecoder().decode(WireGatewayFrame.self, from: data)
            #expect(try Contract.json(JSONEncoder().encode(response)) == Contract.json(data), "\(file.lastPathComponent) does not round-trip")
            switch response {
            case .paired: responseTypes.insert("paired")
            case .authenticated: responseTypes.insert("authenticated")
            case .error(let failure):
                responseTypes.insert("error")
                #expect(failure.code.known != nil, "\(file.lastPathComponent): unknown error code")
                codes.insert(failure.code.rawValue)
            case .unknown(let type, _):
                Issue.record("\(file.lastPathComponent): handshake response \(type) is not modelled")
            }
            // And each maps to something the client acts on.
            if case .unknown = try WireCodec().decode(String(decoding: data, as: UTF8.self)) {
                Issue.record("\(file.lastPathComponent) decoded to unknown")
            }
        }
        #expect(responseTypes == ["paired", "authenticated", "error"])
        // One golden error frame per code the gateway can send.
        #expect(codes == Set(WireGatewayErrorCode.allCases.map(\.rawValue)))

        let qr = try Data(contentsOf: Contract.root.appendingPathComponent("golden/handshake/pairing_qr.json"))
        let payload = try JSONDecoder().decode(WirePairingPayload.self, from: qr)
        #expect(try Contract.json(JSONEncoder().encode(payload)) == Contract.json(qr))
        #expect(payload.hostName == "studio")
    }

    @Test func theHandshakeDTOsKnowExactlyWhatTheSchemasAllow() throws {
        let request = try Contract.schema("handshake-request")
        let response = try Contract.schema("handshake-response")
        #expect(Contract.strings(in: request, tag: "type") == ["pair", "authenticate"])
        #expect(Contract.strings(in: Contract.definition("PairingMethod", in: request)) == Set(WirePairingMethod.allCases.map(\.rawValue)))
        #expect(Contract.strings(in: response, tag: "type") == ["paired", "authenticated", "error"])
        #expect(Contract.strings(in: Contract.definition("ErrorCode", in: response)) == Set(WireGatewayErrorCode.allCases.map(\.rawValue)))

        // The QR payload has exactly the fields the DTO reads.
        let qr = try Contract.schema("pairing-qr")
        let properties = try #require(qr["properties"] as? [String: Any])
        #expect(Set(properties.keys) == ["protocol_version", "address", "gateway_id", "credential", "host_name"])
    }

    @Test func theDTOsKnowExactlyWhatTheSchemasAllow() throws {
        let request = try Contract.schema("request")
        let frame = try Contract.schema("frame")
        func values(_ name: String, in schema: [String: Any], tag: String? = nil) -> Set<String> {
            Contract.strings(in: Contract.definition(name, in: schema), tag: tag)
        }
        func all<T: CaseIterable & RawRepresentable>(_: T.Type) -> Set<String> where T.RawValue == String {
            Set(T.allCases.map(\.rawValue))
        }

        #expect(values("RemoteOperation", in: request, tag: "type") == WireOperation.knownTypes)
        #expect(values("RemoteAnswer", in: request, tag: "type") == WireAnswer.knownTypes)
        #expect(values("ApprovalChoice", in: request) == all(WireApprovalChoice.self))

        #expect(Contract.strings(in: frame, tag: "kind") == WireFrame.knownKinds)
        #expect(values("RemoteResult", in: frame, tag: "type") == WireResult.knownTypes)
        #expect(values("RemoteEvent", in: frame, tag: "type") == WireEvent.knownTypes)
        #expect(values("RemoteErrorCode", in: frame) == all(WireErrorCode.self))
        #expect(values("ReceiptState", in: frame) == all(WireReceiptState.self))
        #expect(values("PromptDisposition", in: frame) == all(WirePromptDisposition.self))
        #expect(values("ResyncReason", in: frame) == all(WireResyncReason.self))
        #expect(values("SessionCloseReason", in: frame) == all(WireSessionCloseReason.self))
        #expect(values("SessionActivity", in: frame) == all(WireSessionActivity.self))
        #expect(values("OwnerHealth", in: frame) == all(WireOwnerHealth.self))
        #expect(values("RemoteToolState", in: frame) == all(WireToolState.self))
        #expect(values("RemoteSubagentStatus", in: frame) == all(WireSubagentStatus.self))
        #expect(values("RemotePendingPromptKind", in: frame) == all(WirePendingPromptKind.self))
    }

    @Test func everyGoldenFrameMapsToADomainMessage() throws {
        let codec = WireCodec()
        for file in try Contract.goldenFiles("frames") {
            let text = try String(contentsOf: file, encoding: .utf8)
            let message = try codec.decode(text)
            if case .unknown(let what) = message {
                Issue.record("\(file.lastPathComponent) decoded to unknown(\(what))")
            }
            if case .reply(_, .unknown(let what)) = message {
                Issue.record("\(file.lastPathComponent) decoded to an unknown reply (\(what))")
            }
            if case .sessionUpdate(let update) = message {
                #expect(!update.changes.isEmpty, "\(file.lastPathComponent) changes nothing")
            }
        }
    }

    @Test func nullIsAcceptedWhereTheHostOmits() throws {
        let text = """
            {"kind":"response","protocol_version":1,"request_id":"r","receipt":null,
             "result":{"type":"request_status","target_request_id":"t","receipt":"unknown","result":null}}
            """
        let frame = try JSONDecoder().decode(WireFrame.self, from: Data(text.utf8))
        guard case .response(let response) = frame, case .requestStatus(let status) = response.result else {
            Issue.record("unexpected frame")
            return
        }
        #expect(response.receipt == nil)
        #expect(status.result == nil)
    }
}

/// A newer host must not break an older app: values this build does not know decode to an
/// explicit unknown, survive a round trip, and are ignored or reported, never thrown.
@Suite struct ForwardCompatibilityTests {
    private func roundTrip<T: Codable>(_ type: T.Type, _ text: String) throws -> T {
        let value = try JSONDecoder().decode(T.self, from: Data(text.utf8))
        #expect(try Contract.json(JSONEncoder().encode(value)) == Contract.json(Data(text.utf8)))
        return value
    }

    @Test func unknownFrameKind() throws {
        let text = #"{"kind":"presence","protocol_version":1,"devices":[{"id":"a","idle_secs":3.5}],"note":null}"#
        guard case .unknown("presence", _) = try roundTrip(WireFrame.self, text) else {
            Issue.record("expected an unknown frame")
            return
        }
        #expect(try WireCodec().decode(text) == .unknown("presence"))
    }

    @Test func unknownEventStillOccupiesItsSequenceNumber() throws {
        let text = #"{"kind":"event","protocol_version":1,"session_id":"s","registration_epoch":2,"sequence":9,"generation":1,"event":{"type":"plan_updated","steps":["a","b"]}}"#
        guard case .event(let frame) = try roundTrip(WireFrame.self, text), case .unknown("plan_updated", _) = frame.event else {
            Issue.record("expected an unknown event")
            return
        }
        let ref = SessionRef(sessionID: "s", registrationEpoch: 2)
        #expect(try WireCodec().decode(text) == .sessionUpdate(SessionUpdate(session: ref, sequence: 9, changes: [])))

        var snapshot = SessionSnapshot(session: ref, sequence: 8)
        #expect(snapshot.apply(SessionUpdate(session: ref, sequence: 9, changes: [])) == .applied)
        #expect(snapshot.apply(update(ref, 10, .responseDelta("next"))) == .applied)
    }

    @Test func unknownResultAndErrorCode() throws {
        let result = #"{"kind":"response","protocol_version":1,"request_id":"r1","result":{"type":"usage","tokens":12}}"#
        _ = try roundTrip(WireFrame.self, result)
        #expect(try WireCodec().decode(result) == .reply(requestID: RequestID("r1"), .unknown("usage")))

        let error = #"{"kind":"response","protocol_version":1,"request_id":"r2","receipt":"rejected","result":{"type":"error","code":"quota_exceeded","message":"No budget left"}}"#
        _ = try roundTrip(WireFrame.self, error)
        #expect(try WireCodec().decode(error) == .reply(
            requestID: RequestID("r2"),
            .failure(.rejected(.other(code: "quota_exceeded", message: "No budget left")))
        ))
    }

    @Test func unknownEnumValuesDegradeInsteadOfFailing() throws {
        let text = #"{"kind":"sessions","protocol_version":1,"gateway_id":"gw","sessions":[{"session_id":"s","registration_epoch":1,"title":"T","model":"m","activity":"compacting","attention":{"approval":false,"question":false},"health":"reconnecting"}]}"#
        _ = try roundTrip(WireFrame.self, text)
        guard case .sessionsChanged(let sessions) = try WireCodec().decode(text) else {
            Issue.record("expected a session list")
            return
        }
        #expect(sessions.first?.activity == .running)
        #expect(sessions.first?.health == .degraded)
        #expect(sessions.first?.workspace == "")
    }

    @Test func unknownOperationAndAnswerRoundTrip() throws {
        let text = #"{"protocol_version":1,"request_id":"r","operation":{"type":"rename_session","title":"New"}}"#
        guard case .unknown("rename_session", _) = try roundTrip(WireRequest.self, text).operation else {
            Issue.record("expected an unknown operation")
            return
        }
        guard case .unknown("ranked", _) = try roundTrip(WireAnswer.self, #"{"type":"ranked","order":[2,1]}"#) else {
            Issue.record("expected an unknown answer")
            return
        }
    }

    @Test func unknownGatewayFrameAndErrorCode() throws {
        #expect(try WireCodec().decode(#"{"type":"welcome","protocol_version":1}"#) == .unknown("welcome"))
        let failure = try WireCodec().decode(#"{"type":"error","code":"maintenance","message":"Back soon"}"#)
        #expect(failure == .gatewayFailure(GatewayFailure(
            error: .transport("The host closed the connection: Back soon (maintenance).")
        )))
    }

    @Test func framesThatAreNotProtocolAreRejectedWithoutQuotingThem() {
        let codec = WireCodec()
        for text in ["not json", "[1,2]", #"{"hello":"world"}"#, #"{"kind":"event","protocol_version":1,"token":"secret-value"}"#] {
            do {
                _ = try codec.decode(text)
                Issue.record("\(text) should not decode")
            } catch {
                guard case RemoteError.invalidFrame(let detail) = error else {
                    Issue.record("unexpected error \(error)")
                    continue
                }
                #expect(!detail.contains("secret-value"))
            }
        }
    }
}
