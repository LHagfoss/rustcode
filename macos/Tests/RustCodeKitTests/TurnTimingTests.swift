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
        guard case .agentText(let text, let turn) = SessionUpdate(notificationParams: params) else { return XCTFail("text") }
        XCTAssertEqual(text, "Done.")
        XCTAssertEqual(turn?.elapsed, 4)
    }
}
