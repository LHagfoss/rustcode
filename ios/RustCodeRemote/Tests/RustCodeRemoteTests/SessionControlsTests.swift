import Foundation
import Testing
@testable import RustCodeRemote

@Suite struct SessionControlsTests {
    @Test func hostMetadataIsOptionalAndTitlesHaveStableFallback() throws {
        let raw = #"{"session_id":"abcdef123456","registration_epoch":1,"title":"  \n","model":"model","activity":"idle","attention":{"approval":false,"question":false},"health":"live"}"#
        let old = WireCodec.summary(try JSONDecoder().decode(WireSessionInfo.self, from: Data(raw.utf8)))
        #expect(old.displayTitle == "Session abcdef12")
        #expect(old.turnCount == nil)
        let current = raw.dropLast() + #", "turn_count":42}"#
        #expect(WireCodec.summary(try JSONDecoder().decode(WireSessionInfo.self, from: Data(current.utf8))).turnCount == 42)
    }

    @Test func lastActivityDecodesHostTimestampsAndLabelsLikeAMessageList() throws {
        let raw = #"{"session_id":"s","registration_epoch":1,"title":"t","model":"m","activity":"idle","attention":{"approval":false,"question":false},"health":"live","last_active_at":"2026-10-14T08:59:26.230467+02:00"}"#
        let summary = WireCodec.summary(try JSONDecoder().decode(WireSessionInfo.self, from: Data(raw.utf8)))
        #expect(abs(summary.lastActiveAt!.timeIntervalSince1970 - 1_791_961_166.23) < 0.01)

        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(identifier: "Europe/Oslo")!
        let locale = Locale(identifier: "en_GB")
        let now = calendar.date(from: DateComponents(year: 2026, month: 10, day: 14, hour: 11, minute: 10))!
        func label(_ seconds: TimeInterval) -> String {
            LastActivePresentation.label(for: now.addingTimeInterval(-seconds), now: now, calendar: calendar, locale: locale)
        }
        #expect(label(20) == "Now")
        #expect(label(2 * 3600) == "09:10")
        #expect(label(24 * 3600) == "Yesterday")
        #expect(label(3 * 24 * 3600) == "Sunday")
        #expect(label(30 * 24 * 3600) == "14 Sep")
    }

    @Test func settingsCommandUsesExactHostProfileAndEffort() throws {
        let encoded = try WireCodec().encode(.setSessionSettings(SessionRef(sessionID: "s", registrationEpoch: 7), model: "profile-id", reasoningEffort: "default"), requestID: RequestID("r"))
        let request = try JSONDecoder().decode(WireRequest.self, from: Data(encoded.utf8))
        #expect(request.operation == .setSessionSettings(.init(model: "profile-id", reasoningEffort: "default")))
        #expect(request.registrationEpoch == 7)
    }

    @Test func tokenEstimateIsExplicitAndAppearsOnlyOnce() {
        let blocks = AssistantBlock.parse("<think>Plan</think>Answer<think>Check</think>Done")
        #expect(TimingPresentation.thinkingLabel(at: 0, in: blocks, milliseconds: 420, tokens: 25, estimated: true) == "Thinking · 420ms · ~25 tokens")
        #expect(TimingPresentation.thinkingLabel(at: 2, in: blocks, milliseconds: 420, tokens: 25, estimated: true) == "Thinking")
    }
}

@Suite struct SessionSettingsStateTests {
    @Test func estimatedTokensSurviveHistoryMappingAndStreamingCompletion() throws {
        let raw = #"{"message_id":"a","role":"assistant","content":{"text":"<think>Plan</think>Done","truncated":false,"offset":0,"total_bytes":23},"thought_tokens":12,"thought_tokens_estimated":true,"future_field":"kept"}"#
        let wire = try JSONDecoder().decode(WireMessage.self, from: Data(raw.utf8))
        guard case .assistant(let text) = WireCodec.item(wire).content else { Issue.record("missing assistant"); return }
        #expect(text.thoughtTokens == 12)
        #expect(text.thoughtTokensEstimated == true)
        #expect(text.metadata["future_field"] == .string("kept"))
        let ref = SessionRef(sessionID: "s", registrationEpoch: 1)
        var snapshot = SessionSnapshot(session: ref, activeTurnID: "t")
        snapshot.apply(.init(session: ref, sequence: 1, changes: [.responseDelta("<think>Plan"), .thoughtTokensChanged(9, estimated: true)]))
        snapshot.apply(.init(session: ref, sequence: 2, event: .turnFinished(turnID: "t", outcome: .completed)))
        guard case .assistant(let completed) = snapshot.items.last?.content else { Issue.record("missing response"); return }
        #expect(completed.thoughtTokens == 9)
        #expect(!completed.isStreaming)
    }

    @Test func demoSettingsValidateChoicesAndReplayReceiptsWithoutApplyingTwice() async throws {
        let client = DemoRemoteClient()
        let credential = try await client.pair(.scanned("demo"), deviceName: "test")
        try await client.connect(using: credential)
        let ref = SessionRef(sessionID: DemoRemoteClient.idleSessionID, registrationEpoch: 1)
        let first = try await client.attach(ref, resumeAfter: nil)
        guard case .snapshot(let initial) = first.start else { Issue.record("missing snapshot"); return }
        #expect(initial.settings?.selectedModel == "demo-balanced")
        let id = RequestID("settings-test")
        let result = try await client.setSessionSettings(model: "demo-fast", reasoningEffort: "default", in: ref, requestID: id)
        #expect(result.state == .applied)
        let replay = try await client.setSessionSettings(model: "demo-balanced", reasoningEffort: "high", in: ref, requestID: id)
        #expect(replay == result)
        let refreshed = try await client.attach(ref, resumeAfter: nil)
        guard case .snapshot(let state) = refreshed.start else { Issue.record("missing snapshot"); return }
        #expect(state.settings?.selectedModel == "demo-fast")
        let invalid = try await client.setSessionSettings(model: "demo-fast", reasoningEffort: "high", in: ref, requestID: RequestID())
        #expect(invalid.state == .rejected(.unsupportedOperation))
        await client.disconnect()
    }
}
