import Foundation
import Testing
@testable import RustCodeRemote

@Suite struct TimingStateTests {
    let ref = SessionRef(sessionID: "s", registrationEpoch: 1)

    @Test func logicalTurnIdentityAndThoughtAggregatesSurviveToolBoundariesAndFinish() throws {
        var snapshot = SessionSnapshot(session: ref)
        let timing = TurnTiming(turnID: "turn:stable", elapsedWorkMilliseconds: 12000)
        snapshot.apply(update(ref, 1, .turnStarted(turnID: timing.turnID)))
        snapshot.apply(update(ref, 2, .timingChanged(timing, thoughtTimeMilliseconds: nil)))
        snapshot.apply(update(ref, 3, .responseDelta("<think>Plan</think>First")))
        snapshot.apply(update(ref, 4, .timingChanged(timing, thoughtTimeMilliseconds: 4000)))
        snapshot.apply(update(ref, 5, .itemAppended(TranscriptItem(id: "tool", .tool(ToolCall(name: "read", summary: "", status: .running))))))
        snapshot.apply(update(ref, 6, .responseDelta("<think>Check</think>Second")))
        snapshot.apply(update(ref, 7, .timingChanged(timing, thoughtTimeMilliseconds: 2000)))
        let final = TurnTiming(turnID: timing.turnID, endedAt: "2026-10-09T16:42:00Z", elapsedWorkMilliseconds: 32000, outcome: "failed")
        snapshot.apply(update(ref, 8, .timingChanged(final, thoughtTimeMilliseconds: nil)))
        snapshot.apply(update(ref, 9, .turnFinished(turnID: timing.turnID, outcome: .failed)))
        let assistants = snapshot.items.compactMap { item -> AssistantText? in
            if case .assistant(let assistant) = item.content { assistant } else { nil }
        }
        #expect(assistants.count == 2)
        #expect(assistants.map(\.thoughtTimeMilliseconds) == [4000, 2000])
        #expect(assistants.allSatisfy { $0.turn == final && !$0.isStreaming })
        #expect(snapshot.lastTurn == final)
        #expect(snapshot.activeTurnID == nil)
        #expect(snapshot.items[0].id == "response:turn:stable")
        #expect(snapshot.items[2].id == "response:turn:stable:6")
    }

    @Test func clockUpdatesDoNotAttributeEarlierSegmentThoughtsToANewSegment() {
        var snapshot = SessionSnapshot(session: ref, items: [
            TranscriptItem(id: "a", .assistant(AssistantText(text: "First", thoughtTimeMilliseconds: 4000))),
            TranscriptItem(id: "tool", .tool(ToolCall(name: "read", summary: "", status: .running)))
        ], activeTurnID: "t")
        snapshot.apply(update(ref, 1, .thoughtTimeChanged(9000)))
        #expect(snapshot.items.count == 2)
        snapshot.apply(update(ref, 2, .responseDelta("Second")))
        guard case .assistant(let assistant) = snapshot.items[2].content else { return }
        #expect(assistant.thoughtTimeMilliseconds == nil)
    }

    @Test func emptyFailedTurnRemainsInHistoryAfterALaterTurnCompletes() {
        let failed = TurnTiming(turnID: "failed", outcome: "failed")
        let completed = TurnTiming(turnID: "completed", outcome: "completed")
        var snapshot = SessionSnapshot(session: ref, items: [
            TranscriptItem(id: "u1", .user("First"), turn: TurnTiming(turnID: "failed"))
        ], activeTurnID: "failed")
        snapshot.apply(update(ref, 1, .timingChanged(failed, thoughtTimeMilliseconds: nil)))
        snapshot.apply(update(ref, 2, .turnFinished(turnID: "failed", outcome: .failed)))
        snapshot.apply(update(ref, 3, .itemAppended(TranscriptItem(id: "u2", .user("Second"), turn: TurnTiming(turnID: "completed")))))
        snapshot.apply(update(ref, 4, .turnStarted(turnID: "completed")))
        snapshot.apply(update(ref, 5, .timingChanged(completed, thoughtTimeMilliseconds: nil)))
        snapshot.apply(update(ref, 6, .turnFinished(turnID: "completed", outcome: .completed)))
        #expect(snapshot.items[0].turn == failed)
        #expect(snapshot.lastTurn == completed)
        #expect(TimingPresentation.footerOwners(items: snapshot.items, lastTurn: snapshot.lastTurn) == ["u1": failed, "u2": completed])
    }

    @Test func identityWithoutInitialTimingPreservesEmptyFailureAfterAnotherTurn() throws {
        var snapshot = SessionSnapshot(session: ref)
        let frames = [
            #"{"type":"turn_started","turn_id":"a","prompt":{"text":"First","truncated":false,"offset":0,"total_bytes":5}}"#,
            #"{"type":"turn_finished","turn_id":"a","timing":{"turn_id":"a","outcome":"failed"}}"#,
            #"{"type":"turn_started","turn_id":"b","prompt":{"text":"Second","truncated":false,"offset":0,"total_bytes":6}}"#,
            #"{"type":"turn_finished","turn_id":"b","timing":{"turn_id":"b","outcome":"completed"}}"#,
        ]
        for (index, raw) in frames.enumerated() {
            let event = try JSONDecoder().decode(WireEvent.self, from: Data(raw.utf8))
            let message = WireCodec.message(for: WireEventFrame(protocolVersion: 1, sessionID: ref.sessionID,
                registrationEpoch: ref.registrationEpoch, sequence: UInt64(index + 1), generation: 1, event: event))
            if case .sessionUpdate(let update) = message { snapshot.apply(update) }
        }
        let failed = TurnTiming(turnID: "a", outcome: "failed")
        let completed = TurnTiming(turnID: "b", outcome: "completed")
        #expect(TimingPresentation.footerOwners(items: snapshot.items, lastTurn: snapshot.lastTurn) ==
            ["prompt:1": failed, "prompt:3": completed])
    }

    @Test func phaseBeforeFirstTimingRetainsItsKnownTurnIdentity() {
        var snapshot = SessionSnapshot(session: ref)
        snapshot.apply(update(ref, 1, .turnStarted(turnID: "a")))
        snapshot.apply(update(ref, 2, .responseDelta("First phase")))
        snapshot.apply(update(ref, 3, .itemAppended(TranscriptItem(id: "tool", .tool(ToolCall(name: "read", summary: "", status: .running))))))
        #expect(snapshot.items[0].turnTiming == TurnTiming(turnID: "a"))
        let failed = TurnTiming(turnID: "a", elapsedWorkMilliseconds: 12000, outcome: "failed")
        snapshot.apply(update(ref, 4, .timingChanged(failed, thoughtTimeMilliseconds: nil)))
        snapshot.apply(update(ref, 5, .turnFinished(turnID: "a", outcome: .failed)))
        #expect(snapshot.items[0].turnTiming == failed)
        #expect(TimingPresentation.footerOwners(items: snapshot.items, lastTurn: snapshot.lastTurn) == ["response:a": failed])
    }

    @Test func attachedActiveTurnWithoutTimingSeedsOnlyIdentity() throws {
        let data = try Data(contentsOf: Contract.root.appendingPathComponent("golden/frames/response_attached.json"))
        let frame = try JSONDecoder().decode(WireFrame.self, from: data)
        guard case .response(let response) = frame, case .attached(let attached) = response.result else { return }
        var wire = attached.snapshot
        wire.turn?.timing = nil
        let snapshot = WireCodec.snapshot(wire)
        let identity = TurnTiming(turnID: "turn:48213:12")
        #expect(snapshot.activeTurnTiming == identity)
        #expect(snapshot.items.last?.turnTiming == identity)
        #expect(snapshot.activeTurnTiming?.elapsedWorkMilliseconds == nil)
        #expect(snapshot.activeTurnTiming?.outcome == nil)
    }

    @Test func unknownTurnMetadataAndOutcomeSurviveWireAndDomainMapping() throws {
        let raw = #"{"turn_id":"t","outcome":"future","future_clock":{"paused":true},"elapsed_work_ms":12}"#
        let wire = try JSONDecoder().decode(WireTurnTiming.self, from: Data(raw.utf8))
        #expect(try Contract.json(JSONEncoder().encode(wire)) == Contract.json(Data(raw.utf8)))
        let timing = WireCodec.timing(wire)
        #expect(timing.outcome == "future")
        #expect(timing.metadata == ["future_clock": .object(["paused": .bool(true)])])
    }

    @Test func liveClientResumesFromTheNewestSnapshotIdentity() async throws {
        let attachedData = try Data(contentsOf: Contract.root.appendingPathComponent("golden/frames/response_attached.json"))
        let attached = try JSONDecoder().decode(WireFrame.self, from: attachedData)
        guard case .response(let response) = attached, case .attached(let payload) = response.result else { return }
        let session = WireCodec.snapshot(payload.snapshot).session
        let transport = ScriptedTransport { text in
            if text.contains("authenticate") {
                return [#"{"type":"authenticated","protocol_version":1,"gateway_id":"gateway","instance_id":"inst","device_id":"device","device_name":"Phone"}"#]
            }
            guard let request = try? JSONDecoder().decode(WireRequest.self, from: Data(text.utf8)) else { return [] }
            let result = WireResponse(protocolVersion: 1, requestID: request.requestID, receipt: nil, result: .attached(payload))
            guard let encoded = try? JSONEncoder().encode(WireFrame.response(result)),
                  let frame = String(data: encoded, encoding: .utf8) else { return [] }
            return [frame]
        }
        let client = LiveRemoteClient(codec: WireCodec(), requestTimeout: .seconds(2), transport: { transport })
        _ = try await client.connect(using: makeCredential())
        let attachment = try await client.attach(session, resumeAfter: nil)
        var replacement = WireCodec.snapshot(payload.snapshot)
        replacement.snapshotID = "snapshot-new"
        replacement.sequence += 1
        let event = WireEventFrame(protocolVersion: 1, sessionID: session.sessionID,
            registrationEpoch: session.registrationEpoch, sequence: replacement.sequence, generation: 1,
            event: .snapshot(.init(snapshot: {
                var wire = payload.snapshot
                wire.snapshotID = replacement.snapshotID
                wire.sequence = replacement.sequence
                return wire
            }())))
        await transport.push(String(decoding: try JSONEncoder().encode(WireFrame.event(event)), as: UTF8.self))
        var updates = attachment.updates.makeAsyncIterator()
        let received = try await updates.next()
        #expect(received?.isSnapshot == true)
        _ = try await client.attach(session, resumeAfter: replacement.sequence)
        let last = try #require(await transport.sent.last)
        let resumeRequest = try JSONDecoder().decode(WireRequest.self, from: Data(last.utf8))
        guard case .attachSession(let operation) = resumeRequest.operation else { return }
        #expect(operation.resume?.snapshotID == "snapshot-new")
        #expect(operation.resume?.lastSequence == replacement.sequence)
        await client.disconnect()
    }
}
