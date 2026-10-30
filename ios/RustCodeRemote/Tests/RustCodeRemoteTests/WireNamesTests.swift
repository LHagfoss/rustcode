import Foundation
import Testing
@testable import RustCodeRemote

/// Thread names on the wire: the translation, and the client using it only once the gateway
/// confirms it.
@Suite struct WireNamesTests {
    private func golden(_ path: String) throws -> String {
        try String(contentsOf: Contract.root.appendingPathComponent("golden/\(path).json"), encoding: .utf8)
    }

    private func object(_ frame: String) throws -> [String: Any] {
        try #require(JSONSerialization.jsonObject(with: Data(frame.utf8)) as? [String: Any])
    }

    @Test func everyGoldenFrameSurvivesTheRoundTrip() throws {
        let directory = Contract.root.appendingPathComponent("golden/frames")
        let names = try FileManager.default.contentsOfDirectory(atPath: directory.path).filter { $0.hasSuffix(".json") }
        #expect(!names.isEmpty)
        let codec = WireCodec()
        for name in names {
            let frame = try String(contentsOf: directory.appendingPathComponent(name), encoding: .utf8)
            let thread = WireNames.toThreadNames(frame)
            #expect(try codec.decode(WireNames.toSessionNames(thread)) == codec.decode(frame), "\(name)")
        }
    }

    @Test func aSessionListReadsInThreadNames() throws {
        let thread = try object(WireNames.toThreadNames(try golden("frames/response_sessions")))
        let result = try #require(thread["result"] as? [String: Any])
        #expect(result["type"] as? String == "threads")
        let threads = try #require(result["threads"] as? [[String: Any]])
        #expect(threads.first?["thread_id"] != nil)
        #expect(threads.first?["session_id"] == nil)
        #expect(result["sessions"] == nil)
    }

    @Test func textIsNeverTranslated() throws {
        let frame = #"{"type":"submit_prompt","text":"list_sessions","title":"sessions"}"#
        #expect(try object(WireNames.toThreadNames(frame))["text"] as? String == "list_sessions")
        #expect(try object(WireNames.toThreadNames(frame))["title"] as? String == "sessions")
        #expect(WireNames.toSessionNames("not json") == "not json")
    }

    @Test func onlyAConfirmingHandshakeTurnsThreadNamesOn() throws {
        #expect(WireNames.confirmed(in: try golden("handshake/responses/authenticated_thread_names")))
        #expect(!WireNames.confirmed(in: try golden("handshake/responses/authenticated")))
        #expect(!WireNames.confirmed(in: #"{"kind":"response","wire_names":"thread"}"#))
    }

    /// A gateway that confirms gets thread names after the handshake; the app still sees
    /// the same session list.
    @Test func theClientSpeaksThreadNamesAfterTheGatewayConfirms() async throws {
        let welcome = try golden("handshake/responses/authenticated_thread_names")
            .replacingOccurrences(of: "gw-8e1f4b7a", with: "gateway")
        let list = WireNames.toThreadNames(try golden("frames/response_sessions"))
        let transport = ScriptedTransport { sent in
            guard let frame = try? JSONSerialization.jsonObject(with: Data(sent.utf8)) as? [String: Any] else { return [] }
            if frame["type"] as? String == "authenticate" { return [welcome] }
            guard let id = frame["request_id"] as? String else { return [] }
            return [list.replacingOccurrences(of: "req-0001", with: id)]
        }
        let client = LiveRemoteClient(codec: WireCodec()) { transport }
        try await client.connect(using: makeCredential())
        let stream = try await client.subscribeSessions()
        var iterator = stream.makeAsyncIterator()
        let first = try #require(try await iterator.next())
        #expect(first.count == 1)

        let sent = await transport.sent
        #expect(sent.count == 2)
        #expect(sent[0].contains(#""wire_names":"thread""#))
        #expect(sent[1].contains("subscribe_threads"))
        #expect(!sent[1].contains("session"))
        await client.disconnect()
    }
}
