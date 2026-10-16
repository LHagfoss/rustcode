import Foundation
import Testing
@testable import RustCodeRemote

@Suite struct SavedSessionWireTests {
    private func golden(_ name: String) throws -> RemoteMessage {
        let url = Contract.root.appendingPathComponent("golden/frames/\(name).json")
        return try WireCodec().decode(String(contentsOf: url, encoding: .utf8))
    }

    @Test func savedSessionPagesMapStatesDatesAndCursor() throws {
        guard case .reply(_, .savedSessions(let page)) = try golden("response_saved_sessions") else {
            Issue.record("expected a saved-session page")
            return
        }
        #expect(page.sessions.map(\.state) == [.saved, .shared, .open])
        let first = try #require(page.sessions.first)
        #expect(first.title == "Fix the parser")
        #expect(first.workspace == "/Users/example/projects/parser")
        #expect(first.messageCount == 42)
        #expect(first.updatedAt == TimingPresentation.date("2026-07-14T09:40:12Z"))
        #expect(first.canOpen)
        #expect(!page.sessions[1].canOpen)
        #expect(!page.sessions[2].canOpen)
        #expect(page.sessions[2].workspace == nil)
        #expect(page.nextCursor == page.sessions.last?.id)
    }

    @Test func resumedSessionsNeedAnAppliedReceipt() throws {
        guard case .reply(_, .sessionResumed(let session)) = try golden("response_session_resumed") else {
            Issue.record("expected a resumed session")
            return
        }
        #expect(session.id == "5f0c2d9e-7a41-4c1b-9d55-0b6c1f2a3e44")
        let wire = #"{"session_id":"s","registration_epoch":2,"title":"","model":"m","activity":"idle","attention":{"approval":false,"question":false},"health":"live"}"#
        let unconfirmed = #"{"kind":"response","protocol_version":1,"request_id":"r","receipt":"unknown","result":{"type":"session_resumed","session":\#(wire)}}"#
        #expect(try WireCodec().decode(unconfirmed) == .reply(requestID: RequestID("r"), .receipt(.unknown)))
    }

    @Test func savedSessionOperationsAreHostWide() throws {
        for command in [
            RemoteCommand.listSavedSessions(cursor: "c", limit: 30, query: "parser"),
            .resumeSession(sessionID: "saved-1"),
        ] {
            let raw = try WireCodec().encode(command, requestID: RequestID("r"))
            let request = try JSONDecoder().decode(WireRequest.self, from: Data(raw.utf8))
            #expect(request.sessionID == nil)
            #expect(request.registrationEpoch == nil)
            switch request.operation {
            case .listSavedSessions(let list):
                #expect(list.cursor == "c")
                #expect(list.limit == 30)
                #expect(list.query == "parser")
            case .resumeSession(let resume):
                #expect(resume.sessionID == "saved-1")
            default:
                Issue.record("unexpected operation \(request.operation.type)")
            }
        }
    }

    @Test func aStateFromANewerHostIsNotOpenable() throws {
        let text = #"{"kind":"response","protocol_version":1,"request_id":"r","result":{"type":"saved_sessions","sessions":[{"session_id":"s","title":"T","workspace":"/w","message_count":1,"state":"archived"}]}}"#
        guard case .reply(_, .savedSessions(let page)) = try WireCodec().decode(text) else {
            Issue.record("expected a saved-session page")
            return
        }
        #expect(page.sessions.first?.state == .unknown)
        #expect(page.sessions.first?.canOpen == false)
        #expect(page.nextCursor == nil)
    }

    @Test func anOlderHostAdvertisesNothing() throws {
        let text = #"{"kind":"response","protocol_version":1,"request_id":"r","result":{"type":"sessions","gateway_id":"gw","sessions":[],"subscribed":true}}"#
        #expect(try WireCodec().decode(text) == .reply(requestID: RequestID("r"), .sessions([], capabilities: [])))
    }

    @Test func theLiveClientKeepsTheCapabilitiesOfTheLatestList() async throws {
        let transport = ScriptedTransport(responder: scriptedResponder([
            "authenticate": ["authenticated"],
            "subscribe": ["sessions-saved|1"],
        ]))
        let client = LiveRemoteClient(codec: LineCodec()) { transport }
        #expect(await client.hostCapabilities().isEmpty)
        try await client.connect(using: makeCredential())
        _ = try await client.subscribeSessions()
        #expect(await client.hostCapabilities() == [HostCapability.savedSessions])
        await client.disconnect()
        #expect(await client.hostCapabilities().isEmpty)
    }
}

@MainActor
@Suite struct SavedSessionStoreTests {
    private func demoStore() async -> RemoteStore {
        let store = RemoteStore(
            client: DemoRemoteClient(pace: .zero),
            demoClient: { DemoRemoteClient(pace: .zero) },
            credentials: InMemoryCredentialStore()
        )
        await store.startDemo(deviceName: "Test")
        #expect(await waitUntil { store.connection == .connected })
        return store
    }

    @Test func recentListsOnlySessionsThatAreNotLive() async {
        let store = await demoStore()
        #expect(store.supportsSavedSessions)
        await store.loadSavedSessions()
        let live = Set(store.sessions.map(\.id))
        #expect(!store.recentSessions.isEmpty)
        #expect(store.recentSessions.allSatisfy { !live.contains($0.id) && $0.state != .shared })
        #expect(store.savedSessions.contains { live.contains($0.id) })

        await store.loadSavedSessions(query: "pairing")
        #expect(store.recentSessions.map(\.id) == ["demo-saved-docs"])
        await store.unpair()
        #expect(!store.supportsSavedSessions)
        #expect(store.savedSessions.isEmpty)
    }

    @Test func openingASavedSessionSharesAndSelectsIt() async {
        let store = await demoStore()
        await store.loadSavedSessions()
        #expect(await store.openSavedSession("demo-saved-parser"))
        #expect(store.selectedSessionID == "demo-saved-parser")
        #expect(store.sessions.contains { $0.id == "demo-saved-parser" })
        #expect(!store.recentSessions.contains { $0.id == "demo-saved-parser" })
        #expect(await waitUntil { store.attached?.session.sessionID == "demo-saved-parser" })
        await store.unpair()
    }

    @Test func aSessionHeldByATerminalIsNotOpenedTwice() async {
        let store = await demoStore()
        await store.loadSavedSessions()
        let held = store.recentSessions.first { $0.id == "demo-saved-terminal" }
        #expect(held?.state == .open)
        #expect(held?.canOpen == false)
        #expect(await store.openSavedSession("demo-saved-terminal") == false)
        #expect(store.selectedSessionID == nil)
        #expect(store.notice?.contains("open in a terminal") == true)
        await store.unpair()
    }
}
