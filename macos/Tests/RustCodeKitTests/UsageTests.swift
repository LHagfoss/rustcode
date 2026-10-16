import Foundation
import XCTest
@testable import RustCodeKit

final class UsageTests: XCTestCase {
    private func notification(_ update: JSONValue) -> JSONValue {
        ["sessionId": "s1", "update": update]
    }

    func testUsageUpdateDecodesContextAndRustCodeTokenTotals() {
        let update = SessionUpdate(notificationParams: notification([
            "sessionUpdate": "usage_update", "used": 34_210, "size": 128_000,
            "_meta": ["rustcode": [
                "kind": "usage", "version": 1, "estimated": false,
                "inputTokens": 1_234_567, "outputTokens": 8_900, "cachedTokens": 1_000_000, "responses": 12,
            ]],
        ]))
        guard case .usage(let usage) = update else { return XCTFail("expected usage, got \(update)") }
        XCTAssertEqual(usage, SessionUsage(
            used: 34_210, size: 128_000, inputTokens: 1_234_567, outputTokens: 8_900, cachedTokens: 1_000_000))
        XCTAssertEqual(usage.label, "34k / 128k")
        XCTAssertEqual(usage.percentLeft, 73)
        XCTAssertEqual(usage.detail, """
            Context: 34,210 of 128,000 tokens (73% left)
            Input: 1,234,567 tokens (1,000,000 cached)
            Output: 8,900 tokens
            """)
    }

    func testStandardUsageUpdateWithoutRustCodeMetaKeepsContextAndCost() {
        let update = SessionUpdate(notificationParams: notification([
            "sessionUpdate": "usage_update", "used": 53_000, "size": 200_000,
            "cost": ["amount": .number(0.045), "currency": "USD"],
            "_meta": ["rustcode": ["kind": "usage", "version": 99, "inputTokens": 5]],
        ]))
        guard case .usage(let usage) = update else { return XCTFail("expected usage, got \(update)") }
        XCTAssertNil(usage.inputTokens, "an unknown meta version is ignored")
        XCTAssertEqual(usage.cost, 0.045)
        XCTAssertEqual(usage.label, "53k / 200k")
        XCTAssertTrue(usage.detail.hasSuffix("Cost: $0.045"), usage.detail)
    }

    func testEstimatedUsageIsMarkedAndMalformedUpdatesAreUnknown() {
        let estimated = SessionUsage(json: [
            "used": 1_500_000, "size": 1_000_000, "_meta": ["rustcode": ["kind": "usage", "version": 1, "estimated": true]],
        ])
        XCTAssertEqual(estimated?.label, "~1.5M / 1M")
        XCTAssertEqual(estimated?.fraction, 1)
        XCTAssertEqual(estimated?.percentLeft, 0)
        XCTAssertEqual(
            SessionUpdate(notificationParams: notification(["sessionUpdate": "usage_update", "used": 5, "size": 0])),
            .unknown("usage_update"))
    }

    func testTranscriptKeepsLatestUsageUntilCleared() {
        var transcript = Transcript()
        XCTAssertNil(transcript.usage)
        transcript.apply(.usage(SessionUsage(used: 10, size: 100)))
        transcript.apply(.agentText("hi"))
        transcript.apply(.usage(SessionUsage(used: 900, size: 100_000)))
        XCTAssertEqual(transcript.usage?.label, "900 / 100k")
        XCTAssertEqual(transcript.items.count, 1, "usage is not a transcript row")

        // A copy (as stashed for a background chat) keeps its own usage.
        let stashed = transcript
        transcript.removeAll()
        XCTAssertNil(transcript.usage)
        XCTAssertEqual(stashed.usage?.used, 900)
    }
}
