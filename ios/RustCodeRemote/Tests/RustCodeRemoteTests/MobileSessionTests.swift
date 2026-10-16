import Foundation
import Testing
@testable import RustCodeRemote

@Suite struct MobileSessionTests {
    @Test func catalogKeepsInsertionAndFutureActionWithoutGuessingSupport() throws {
        let raw = #"{"name":"/future","description":"Host-only command","insertion":"/future ","action":"future_action"}"#
        let choice = try JSONDecoder().decode(SessionCommandChoice.self, from: Data(raw.utf8))
        #expect(choice.insertion == "/future ")
        #expect(choice.action == "future_action")
    }

    @Test func gatewayOperationsDoNotTargetAnExistingSession() throws {
        for command in [RemoteCommand.listDirectories(path: nil), .createSession(path: "/Users/me/Project with spaces")] {
            let raw = try WireCodec().encode(command, requestID: RequestID("new-session"))
            let request = try JSONDecoder().decode(WireRequest.self, from: Data(raw.utf8))
            #expect(request.sessionID == nil)
            #expect(request.registrationEpoch == nil)
            if case .createSession(let path) = request.operation { #expect(path.path == "/Users/me/Project with spaces") }
        }
    }

    @Test func creationRequiresFinalAppliedReceipt() throws {
        let session = #"{"session_id":"created","registration_epoch":1,"title":"","model":"model","activity":"idle","attention":{"approval":false,"question":false},"health":"live"}"#
        func frame(_ receipt: String) -> String {
            #"{"kind":"response","protocol_version":1,"request_id":"create","receipt":"\#(receipt)","result":{"type":"session_created","session":\#(session)}}"#
        }
        #expect(try WireCodec().decode(frame("unknown")) == .reply(requestID: RequestID("create"), .receipt(.unknown)))
        #expect(try WireCodec().decode(frame("received")) == .reply(requestID: RequestID("create"), .receipt(.received)))
        guard case .reply(_, .sessionCreated(let created)) = try WireCodec().decode(frame("applied")) else { Issue.record("Missing creation"); return }
        #expect(created.id == "created")
    }

    @Test func directoryResponsePreservesCanonicalPathAndTruncation() throws {
        let json = #"{"kind":"response","protocol_version":1,"request_id":"d","result":{"type":"directories","path":"/Users/me","parent":"/Users","directories":[{"name":"My Project","path":"/Users/me/My Project"}],"truncated":true}}"#
        let decoded = try WireCodec().decode(json)
        guard case .reply(_, .directories(let listing)) = decoded else { Issue.record("Missing directories"); return }
        #expect(listing.path == "/Users/me")
        #expect(listing.directories.first?.name == "My Project")
        #expect(listing.truncated)
    }

    @Test func demoCreationIsIdempotentAndSurvivesDisconnect() async throws {
        let client = DemoRemoteClient()
        let credential = try await client.pair(.scanned("demo"), deviceName: "test")
        try await client.connect(using: credential)
        let listing = try await client.browseDirectories(path: nil)
        let id = RequestID("create-once")
        let created = try await client.createSession(directory: listing.directories[0].path, requestID: id)
        await client.disconnect()
        try await client.connect(using: credential)
        let again = try await client.createSession(directory: listing.path, requestID: id)
        #expect(again == created)
        #expect(again.workspace == "/Users/demo/Projects")
        let attached = try await client.attach(created.ref, resumeAfter: nil)
        guard case .snapshot(let snapshot) = attached.start else { Issue.record("Missing persisted demo session"); return }
        #expect(snapshot.session == created.ref)
        #expect(created.turnCount == 0)
        await client.disconnect()
    }
}

@Suite struct MobileToolIdentityTests {
    @Test func lateEventsDoNotDuplicateOrRewriteDurableSameArgumentCalls() {
        let session = SessionRef(sessionID: "s", registrationEpoch: 1)
        let timing = TurnTiming(turnID: "current")
        func record(_ id: String, _ output: String) -> TranscriptItem {
            TranscriptItem(id: id, .tool(ToolCall(name: "run_command", summary: "pwd", output: output, status: .succeeded, callID: "local_hash", isTranscriptRecord: true)), turn: timing, timestamp: "saved-time")
        }
        let records = [record("m2", "before edit"), record("m5", "after edit")]
        var snapshot = SessionSnapshot(session: session, items: records, activeTurnID: "current")
        let start = TranscriptItem(id: "tool:local_hash", .tool(ToolCall(name: "run_command", summary: "pwd", status: .running, callID: "local_hash")))
        _ = snapshot.apply(.init(session: session, sequence: 1, event: .itemAppended(start)))
        _ = snapshot.apply(.init(session: session, sequence: 2, event: .toolFinished(itemID: start.id, succeeded: true, isPending: false, output: "late clipped preview", isOutputTruncated: true)))
        #expect(snapshot.items == records)
        // A new turn can run the same arguments; the previous turn cannot suppress it.
        _ = snapshot.apply(.init(session: session, sequence: 3, event: .turnStarted(turnID: "next")))
        _ = snapshot.apply(.init(session: session, sequence: 4, event: .itemAppended(start)))
        _ = snapshot.apply(.init(session: session, sequence: 5, event: .toolFinished(itemID: start.id, succeeded: true, isPending: false, output: "new turn output", isOutputTruncated: false)))
        #expect(snapshot.items.count == 3)
        #expect(Array(snapshot.items.prefix(2)) == records)
        guard case .tool(let latest) = snapshot.items.last?.content else { Issue.record("Missing new turn result"); return }
        #expect(latest.output == "new turn output")
    }

    @Test func durableWireToolKeepsCorrelationWithoutChangingTranscriptIdentity() throws {
        let raw = #"{"message_id":"m2","role":"tool","content":{"text":"result","truncated":false,"offset":0,"total_bytes":6},"tool":{"name":"run_command","call_id":"local_hash","success":true,"pending":false},"turn":{"turn_id":"current"}}"#
        let item = WireCodec.item(try JSONDecoder().decode(WireMessage.self, from: Data(raw.utf8)))
        #expect(item.id == "m2")
        guard case .tool(let tool) = item.content else { Issue.record("Missing tool"); return }
        #expect(tool.callID == "local_hash")
        #expect(tool.isTranscriptRecord)
    }

    @Test func repeatedStartWithoutDetailKeepsTheLiveToolTitle() throws {
        let sessionID = "5f0c2d9e-7a41-4c1b-9d55-0b6c1f2a3e44"
        let session = SessionRef(sessionID: sessionID, registrationEpoch: 1)
        var snapshot = SessionSnapshot(session: session)
        let codec = WireCodec()
        func event(_ sequence: Int, _ body: String) throws -> SessionUpdate? {
            let frame = #"{"kind":"event","protocol_version":1,"session_id":"\#(sessionID)","registration_epoch":1,"sequence":\#(sequence),"generation":1,"event":\#(body)}"#
            guard case .sessionUpdate(let update) = try codec.decode(frame) else { return nil }
            return update
        }
        // What the host sends for a view_file call during a live turn: the start with its
        // detail, then the result preceded by a repeated start that carries no detail.
        let frames = [
            #"{"type":"tool_started","tool":{"id":"call-1","name":"view_file","detail":"src/main.rs","state":"running"}}"#,
            #"{"type":"tool_started","tool":{"id":"call-1","name":"view_file","state":"running"}}"#,
            #"{"type":"tool_finished","id":"call-1","success":true,"pending":false,"content":{"text":"fn main() {}","truncated":false,"offset":0,"total_bytes":12}}"#,
        ]
        for (offset, body) in frames.enumerated() {
            let update = try #require(try event(offset + 1, body))
            #expect(snapshot.apply(update) == .applied)
        }
        #expect(snapshot.items.count == 1)
        guard case .tool(let tool) = snapshot.items.first?.content else { Issue.record("Missing tool"); return }
        #expect(tool.summary == "src/main.rs")
        #expect(tool.status == .succeeded)
        #expect(tool.output == "fn main() {}")
    }

    @Test func laterSnapshotDoesNotForgetToolDetail() {
        let session = SessionRef(sessionID: "detail", registrationEpoch: 1)
        var snapshot = SessionSnapshot(session: session, activeTurnID: "t")
        let start = TranscriptItem(id: "tool:call-1", .tool(ToolCall(name: "view_file", summary: "src/main.rs", status: .running, callID: "call-1")))
        _ = snapshot.apply(.init(session: session, sequence: 1, event: .itemAppended(start)))
        // A resync snapshot lists the running tool without its detail, and the durable
        // record of an earlier call arrives without one too.
        let bare = TranscriptItem(id: "tool:call-1", .tool(ToolCall(name: "view_file", summary: "", status: .running, callID: "call-1")))
        let replacement = SessionSnapshot(session: session, items: [bare], activeTurnID: "t", sequence: 9)
        _ = snapshot.apply(.init(session: session, sequence: 0, event: .replaced(replacement)))
        guard case .tool(let live) = snapshot.items.first?.content else { Issue.record("Missing tool"); return }
        #expect(live.summary == "src/main.rs")
        #expect(snapshot.sequence == 9)

        let record = TranscriptItem(id: "m7", .tool(ToolCall(name: "view_file", summary: "", output: "fn main", status: .succeeded, callID: "call-1", isTranscriptRecord: true)))
        let settled = SessionSnapshot(session: session, items: [record], sequence: 10)
        guard case .tool(let durable) = settled.keepingToolDetail(from: snapshot).items.first?.content else { Issue.record("Missing record"); return }
        #expect(durable.summary == "src/main.rs")
        #expect(durable.output == "fn main")
        // A different tool under a reused call ID keeps its own (empty) detail.
        let other = SessionSnapshot(session: session, items: [TranscriptItem(id: "m8", .tool(ToolCall(name: "grep", summary: "", status: .succeeded, callID: "call-1")))])
        guard case .tool(let unrelated) = other.keepingToolDetail(from: snapshot).items.first?.content else { Issue.record("Missing grep"); return }
        #expect(unrelated.summary.isEmpty)
    }

    @Test func resultWithoutStartNeverRenamesAKnownTool() {
        let session = SessionRef(sessionID: "names", registrationEpoch: 1)
        var snapshot = SessionSnapshot(session: session)
        _ = snapshot.apply(.init(session: session, sequence: 1, event: .toolFinished(itemID: "tool:x", succeeded: true, isPending: true, output: "", isOutputTruncated: false)))
        let start = TranscriptItem(id: "tool:x", .tool(ToolCall(name: "run_command", summary: "make", status: .running)))
        _ = snapshot.apply(.init(session: session, sequence: 2, event: .itemAppended(start)))
        let placeholder = TranscriptItem(id: "tool:x", .tool(ToolCall(name: "tool", summary: "", status: .succeeded)))
        _ = snapshot.apply(.init(session: session, sequence: 3, event: .itemUpdated(placeholder)))
        guard case .tool(let tool) = snapshot.items.first?.content else { Issue.record("Missing tool"); return }
        #expect(tool.name == "run_command")
        #expect(tool.summary == "make")
        #expect(tool.status == .succeeded)
    }

    @Test func lateToolStartAndResultUpsertOneNamedRowAndRetainDistinctCalls() {
        let session = SessionRef(sessionID: "tools", registrationEpoch: 1)
        var snapshot = SessionSnapshot(session: session)
        let first = TranscriptItem(id: "tool:call-1", .tool(ToolCall(name: "run_command", summary: "pwd", status: .running)))
        _ = snapshot.apply(.init(session: session, sequence: 1, event: .itemAppended(first)))
        _ = snapshot.apply(.init(session: session, sequence: 2, event: .toolFinished(itemID: first.id, succeeded: true, isPending: false, output: "same output", isOutputTruncated: false)))
        // A repeated named start repairs a missing live start after late attachment.
        let second = TranscriptItem(id: "tool:call-2", .tool(ToolCall(name: "run_command", summary: "pwd", status: .running)))
        _ = snapshot.apply(.init(session: session, sequence: 3, event: .itemAppended(second)))
        _ = snapshot.apply(.init(session: session, sequence: 4, event: .itemAppended(second)))
        _ = snapshot.apply(.init(session: session, sequence: 5, event: .toolFinished(itemID: second.id, succeeded: true, isPending: false, output: "same output", isOutputTruncated: false)))
        #expect(snapshot.items.count == 2)
        for item in snapshot.items {
            guard case .tool(let tool) = item.content else { Issue.record("Missing tool"); continue }
            #expect(tool.name == "run_command")
            #expect(tool.output == "same output")
        }
    }
}
