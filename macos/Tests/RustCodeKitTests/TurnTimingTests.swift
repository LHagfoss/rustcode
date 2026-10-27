import XCTest
@testable import RustCodeKit

final class TurnTimingTests: XCTestCase {
    private let utc = TimeZone(identifier: "UTC")!
    private let locale = Locale(identifier: "en_GB")

    func testDurationReadsLikeTheThoughtLabel() {
        XCTAssertEqual(TurnTiming.duration(0.2), "1s")
        XCTAssertEqual(TurnTiming.duration(12), "12s")
        XCTAssertEqual(TurnTiming.duration(60), "1m")
        XCTAssertEqual(TurnTiming.duration(72), "1m 12s")
        XCTAssertEqual(TurnTiming.duration(3_780), "1h 3m")
        XCTAssertEqual(TurnTiming.duration(7_200), "2h")
    }

    func testClockCountsUpWhileWorking() {
        XCTAssertEqual(TurnTiming.clock(-1), "00:00")
        XCTAssertEqual(TurnTiming.clock(12.9), "00:12")
        XCTAssertEqual(TurnTiming.clock(725), "12:05")
        XCTAssertEqual(TurnTiming.clock(3_723), "1:02:03")
    }

    func testLabelShowsTheFinishTimeAndItsDateWhenNotToday() {
        let finished = Date(timeIntervalSince1970: 1_761_143_520) // 2025-10-22 14:32 UTC
        let timing = TurnTiming(finished: finished, elapsed: 72, outcome: .completed)
        XCTAssertEqual(timing.label(now: finished.addingTimeInterval(60), locale: locale, timeZone: utc), "1m 12s · 14:32")
        let yesterday = timing.label(now: finished.addingTimeInterval(86_400), locale: locale, timeZone: utc)
        XCTAssertTrue(yesterday.hasPrefix("1m 12s · 22 Oct"), yesterday)
        XCTAssertTrue(yesterday.hasSuffix("14:32"), yesterday)
        XCTAssertEqual(TurnTiming(finished: finished, elapsed: 8, outcome: .cancelled).label(), "Stopped after 8s")
    }

    func testReadsTheEngineTimingOfAReplayedTurn() {
        let json: JSONValue = ["id": "turn:1", "startedAt": "2026-10-22T20:58:02.912347+00:00",
                               "endedAt": "2026-10-22T20:59:52.495464+00:00", "elapsedMs": 109_582, "outcome": "completed"]
        let timing = TurnTiming(json: json)
        XCTAssertEqual(timing?.id, "turn:1")
        XCTAssertEqual(timing?.elapsed, 109.582)
        XCTAssertEqual(timing?.outcome, .completed)
        XCTAssertEqual(timing?.finished.timeIntervalSince1970 ?? 0, 1_792_702_792.495, accuracy: 0.01)
        // Without the engine's measure, the timestamps tell.
        let stamps: JSONValue = ["startedAt": "2026-10-22T20:58:00Z", "endedAt": "2026-10-22T20:58:08Z", "outcome": "cancelled"]
        XCTAssertEqual(TurnTiming(json: stamps)?.elapsed, 8)
        XCTAssertEqual(TurnTiming(json: stamps)?.outcome, .cancelled)
        XCTAssertNil(TurnTiming(json: ["endedAt": "2026-10-22T20:58:08Z", "elapsedMs": 1, "outcome": "failed"]))
        XCTAssertNil(TurnTiming(json: ["elapsedMs": 1, "outcome": "completed"]))
    }

    func testReplayedReplyCarriesItsTurnTiming() {
        let params: JSONValue = ["sessionId": "s", "update": [
            "sessionUpdate": "agent_message_chunk", "content": ["type": "text", "text": "Done."],
            "_meta": ["rustcode": ["turn": ["endedAt": "2026-10-22T20:59:52Z", "elapsedMs": 4_000, "outcome": "completed"]]],
        ]]
        guard case .agentText(let text, let turn, _) = SessionUpdate(notificationParams: params) else { return XCTFail("text") }
        XCTAssertEqual(text, "Done.")
        XCTAssertEqual(turn?.elapsed, 4)
    }

    func testReplayedMessagesCarryTheTimeTheyWereWritten() {
        let user: JSONValue = ["sessionId": "s", "update": [
            "sessionUpdate": "user_message_chunk", "content": ["type": "text", "text": "Hi"],
            "_meta": ["rustcode": ["at": "2026-10-27T09:16:01.473007+01:00"]],
        ]]
        guard case .userText("Hi", let sent) = SessionUpdate(notificationParams: user) else { return XCTFail("user") }
        XCTAssertEqual(sent?.timeIntervalSince1970 ?? 0, 1_793_088_961.473, accuracy: 0.01)
        let reply: JSONValue = ["sessionId": "s", "update": [
            "sessionUpdate": "agent_message_chunk", "content": ["type": "text", "text": "Hello"],
            "_meta": ["rustcode": ["at": "2026-10-27T08:16:16Z"]],
        ]]
        guard case .agentText("Hello", nil, let written) = SessionUpdate(notificationParams: reply) else { return XCTFail("reply") }
        XCTAssertEqual(written?.timeIntervalSince1970, 1_793_088_976)

        var transcript = Transcript()
        transcript.apply(SessionUpdate(notificationParams: user))
        transcript.apply(SessionUpdate(notificationParams: reply))
        XCTAssertEqual(transcript.items.map(\.at), [sent, written])
    }

    func testOldHistoryGetsNoMadeUpTime() {
        var transcript = Transcript()
        transcript.apply(.userText("Hi"))
        transcript.apply(.agentText("Hello"))
        transcript.appendUser("Again", at: Date(timeIntervalSince1970: 100))
        XCTAssertEqual(transcript.items.map(\.at), [nil, nil, Date(timeIntervalSince1970: 100)])
    }

    func testLiveReplyIsTimedWhenItsTurnMovesOn() {
        var transcript = Transcript()
        let start = Date(timeIntervalSince1970: 1_000)
        transcript.appendUser("Go", at: start)
        transcript.beginTurn(at: start)
        transcript.apply(.agentText("Looking"), at: start.addingTimeInterval(1))
        transcript.apply(.agentText(" now."), at: start.addingTimeInterval(2))
        XCTAssertNil(transcript.items[1].at, "a reply being written has no time yet")
        transcript.apply(.toolCall(id: "t", title: "Read", kind: "read", status: .pending, input: nil), at: start.addingTimeInterval(3))
        XCTAssertEqual(transcript.items[1].at, start.addingTimeInterval(3))
        transcript.apply(.agentText("Done."), at: start.addingTimeInterval(4))
        transcript.finishTurn(at: start.addingTimeInterval(9))
        XCTAssertEqual(transcript.items[3].at, start.addingTimeInterval(9))
        XCTAssertEqual(transcript.items[3].turn?.elapsed, 9)
    }

    func testTimeShowsItsDateWhenNotToday() {
        let sent = Date(timeIntervalSince1970: 1_761_143_520) // 2025-10-22 14:32 UTC
        XCTAssertEqual(TurnTiming.time(sent, now: sent.addingTimeInterval(60), locale: locale, timeZone: utc), "14:32")
        let later = TurnTiming.time(sent, now: sent.addingTimeInterval(86_400 * 400), locale: locale, timeZone: utc)
        XCTAssertTrue(later.contains("22 Oct 2025"), later)
    }
}
