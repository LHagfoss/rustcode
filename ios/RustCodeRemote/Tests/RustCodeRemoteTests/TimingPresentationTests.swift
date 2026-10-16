import Foundation
import Testing
@testable import RustCodeRemote

@Suite struct TimingPresentationTests {
    @Test func usesOnlyAuthoritativeWorkTimeAndPreservesUnknownOutcomes() {
        #expect(TimingPresentation.turnLabel(TurnTiming(turnID: "t", elapsedWorkMilliseconds: 32000, outcome: "failed")) == "Failed · Worked for 32s")
        #expect(TimingPresentation.turnLabel(TurnTiming(turnID: "t", outcome: "future")) == "Outcome: future")
        #expect(TimingPresentation.turnLabel(TurnTiming(turnID: "t", startedAt: "2026-10-09T18:41:00+02:00")) == "Working")
        #expect(TimingPresentation.duration(61000) == "1m 1s")
        #expect(TimingPresentation.duration(0) == "0ms")
    }

    @Test func subsecondThinkingPreservesMilliseconds() {
        #expect(TimingPresentation.duration(420) == "420ms")
        #expect(TimingPresentation.duration(999) == "999ms")
        #expect(TimingPresentation.duration(1000) == "1s")
    }

    @Test func thinkingAggregateAppearsOnceAcrossMultipleThinkingBlocks() {
        let blocks = AssistantBlock.parse("<think>Plan</think>Answer<think>Check</think>Done")
        let labels = blocks.indices.map { TimingPresentation.thinkingLabel(at: $0, in: blocks, milliseconds: 4000) }
        #expect(labels == ["Thinking · 4s", nil, "Thinking", nil])
    }

    @Test func usesAuthoritativeEndTimestampBeforeLegacyCompletion() {
        let timing = TurnTiming(turnID: "t", endedAt: "2026-10-09T16:42:00Z", outcome: "failed")
        #expect(TimingPresentation.endTimestamp(timing: timing, legacy: "2026-10-09T16:41:00Z") == timing.endedAt)
        #expect(TimingPresentation.endTimestamp(timing: nil, legacy: "legacy") == "legacy")
        #expect(TimingPresentation.endTimestamp(timing: TurnTiming(turnID: "t"), legacy: "legacy") == nil)
        #expect(TimingPresentation.endLabel(outcome: "failed") == "Failed at")
        #expect(TimingPresentation.endLabel(outcome: "cancelled") == "Cancelled at")
        #expect(TimingPresentation.endLabel(outcome: "completed") == "Completed at")
    }

    @Test func rendersOneTerminalFooterAfterTheFinalCorrelatedPhase() {
        let completed = TurnTiming(turnID: "t", elapsedWorkMilliseconds: 32000, outcome: "completed")
        let items = [
            TranscriptItem(id: "a1", .assistant(AssistantText(text: "First", turn: completed))),
            TranscriptItem(id: "tool", .tool(ToolCall(name: "read", summary: "", status: .succeeded))),
            TranscriptItem(id: "a2", .assistant(AssistantText(text: "Second", turn: completed)))
        ]
        #expect(TimingPresentation.footerOwners(items: items, lastTurn: completed) == ["a2": completed])
    }

    @Test func lastTurnWithoutTranscriptContentStillHasATerminalFooter() {
        let failed = TurnTiming(turnID: "failed", endedAt: "2026-10-09T16:41:00Z", outcome: "failed")
        #expect(TimingPresentation.standaloneFooter(items: [], lastTurn: failed) == failed)
        let user = TranscriptItem(id: "u", .user("First"), turn: failed)
        #expect(TimingPresentation.standaloneFooter(items: [user], lastTurn: failed) == nil)
        #expect(TimingPresentation.standaloneFooter(items: [], lastTurn: TurnTiming(turnID: "active")) == nil)
    }

    @Test func preservesHistoricalEmptyTurnOutcomesWhenTheLatestTurnChanges() throws {
        let failed = TurnTiming(turnID: "failed", endedAt: "2026-10-09T16:41:00Z", outcome: "failed")
        let completed = TurnTiming(turnID: "completed", endedAt: "2026-10-09T16:42:00Z", outcome: "completed")
        let raw = #"{"message_id":"u1","role":"user","content":{"text":"First","truncated":false,"offset":0,"total_bytes":5},"turn":{"turn_id":"failed","ended_at":"2026-10-09T16:41:00Z","outcome":"failed"}}"#
        let message = try JSONDecoder().decode(WireMessage.self, from: Data(raw.utf8))
        let item = WireCodec.item(message)
        let items = [item, TranscriptItem(id: "a2", .assistant(AssistantText(text: "Done", turn: completed)))]
        #expect(TimingPresentation.footerOwners(items: items, lastTurn: completed) == ["u1": failed, "a2": completed])
    }

    @Test func parsesCompletionTimestampsWithOffsetsAndFractions() throws {
        let offset = try #require(TimingPresentation.date("2026-10-09T18:40:32+02:00"))
        let utc = try #require(TimingPresentation.date("2026-10-09T16:40:32Z"))
        #expect(offset == utc)
        #expect(TimingPresentation.date("2026-10-09T16:40:32.123Z") != nil)
        #expect(TimingPresentation.date("future-format") == nil)
    }

    @Test func footerIsOneLineWithOutcomeWorkTimeAndEndTime() {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(identifier: "Europe/Oslo")!
        let locale = Locale(identifier: "en_GB")
        let ended = "2026-10-09T18:40:32+02:00"
        let timing = TurnTiming(turnID: "t", endedAt: ended, elapsedWorkMilliseconds: 32000, outcome: "completed")
        func line(_ timing: TurnTiming?, legacy: String? = nil, now: String) -> String? {
            TimingPresentation.footerLine(timing: timing, legacyCompletedAt: legacy,
                now: TimingPresentation.date(now)!, calendar: calendar, locale: locale)
        }

        // Same day: the time alone. Another day: day and month. Another year: the year too.
        #expect(line(timing, now: "2026-10-09T21:00:00+02:00") == "Completed · Worked for 32s · 18:40")
        // The joining word between date and time is the system's, so only the parts are pinned.
        let otherDay = line(timing, now: "2026-10-13T09:00:00+02:00") ?? ""
        #expect(otherDay.hasPrefix("Completed · Worked for 32s · 9 Oct") && otherDay.hasSuffix("18:40"))
        #expect(!otherDay.contains("2026"))
        let otherYear = line(timing, now: "2027-01-02T09:00:00+01:00") ?? ""
        #expect(otherYear.hasPrefix("Completed · Worked for 32s · 9 Oct 2026") && otherYear.hasSuffix("18:40"))

        // No work time, a legacy timestamp, an unparseable one, and nothing to say.
        let failed = TurnTiming(turnID: "t", endedAt: ended, outcome: "failed")
        #expect(line(failed, now: "2026-10-09T21:00:00+02:00") == "Failed · 18:40")
        #expect(line(nil, legacy: ended, now: "2026-10-09T21:00:00+02:00") == "Completed · 18:40")
        #expect(line(nil, legacy: "saved-time", now: "2026-10-09T21:00:00+02:00") == "Completed · saved-time")
        #expect(line(TurnTiming(turnID: "t"), now: "2026-10-09T21:00:00+02:00") == nil)
        #expect(line(nil, now: "2026-10-09T21:00:00+02:00") == nil)
    }
}
