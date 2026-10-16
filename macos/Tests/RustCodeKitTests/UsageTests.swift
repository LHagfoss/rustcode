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

final class ContextBreakdownTests: XCTestCase {
    private let contextJSON: JSONValue = [
        "version": 1, "model": "gpt", "window": 200_000, "used": 50_000, "estimated": false, "free": 150_000,
        "categories": [
            ["name": "User messages", "tokens": 20_000],
            ["name": "Agent responses", "tokens": 0],
            ["name": "Tool calls", "tokens": 30_000],
        ],
        "reserves": ["output": 32_000, "thinking": 8_000, "tools": 4_000, "safety": 2_000, "providerOverhead": 1_000],
    ]

    func testUsageUpdateCarriesTheContextBreakdown() {
        let update = SessionUpdate(notificationParams: ["sessionId": "s1", "update": [
            "sessionUpdate": "usage_update", "used": 50_000, "size": 200_000,
            "_meta": ["rustcode": ["kind": "usage", "version": 1, "context": contextJSON]],
        ]])
        guard case .usage(let usage) = update else { return XCTFail("expected usage, got \(update)") }
        guard let context = usage.context else { return XCTFail("expected a breakdown") }
        XCTAssertEqual(context.model, "gpt")
        XCTAssertEqual(context.categories.map(\.name), ["User messages", "Agent responses", "Tool calls"])
        XCTAssertEqual(context.reserves?.thinking, 8_000)
        XCTAssertEqual(context.percent(20_000), "10.0%")
    }

    func testContextReplyKeepsItsTextAndOpensTheBreakdown() {
        let update = SessionUpdate(notificationParams: ["sessionId": "s1", "update": [
            "sessionUpdate": "agent_message_chunk",
            "content": ["type": "text", "text": "**Context**\n\n| Category | Tokens |"],
            "_meta": ["rustcode": ["kind": "context", "version": 1, "context": contextJSON]],
        ]])
        guard case .contextReport(let text, let context) = update else { return XCTFail("expected a context report, got \(update)") }
        XCTAssertTrue(text.hasPrefix("**Context**"))
        XCTAssertEqual(context.used, 50_000)

        var transcript = Transcript()
        transcript.appendCommand("/context")
        transcript.apply(update)
        guard case .command(_, let output) = transcript.items.last?.content else { return XCTFail("expected the command card") }
        XCTAssertEqual(output, text)
    }

    func testUnknownVersionsAndEmptyWindowsAreIgnored() {
        XCTAssertNil(ContextBreakdown(json: ["version": 2, "window": 10, "used": 1]))
        XCTAssertNil(ContextBreakdown(json: ["version": 1, "window": 0, "used": 1]))
        let plain = SessionUpdate(notificationParams: ["sessionId": "s1", "update": [
            "sessionUpdate": "agent_message_chunk", "content": ["type": "text", "text": "hi"],
            "_meta": ["rustcode": ["kind": "context", "version": 9]],
        ]])
        XCTAssertEqual(plain, .agentText("hi"))
    }

    func testSegmentsSkipEmptyCategoriesAndNeverOverfill() {
        let context = ContextBreakdown(json: contextJSON)!
        XCTAssertEqual(context.segments.map(\.category.name), ["User messages", "Tool calls"])
        XCTAssertEqual(context.segments.map(\.share).reduce(0, +), 0.25, accuracy: 0.0001)
        let full = ContextBreakdown(window: 10, used: 30, free: 0, categories: [
            .init(name: "a", tokens: 20), .init(name: "b", tokens: 20),
        ])
        XCTAssertLessThanOrEqual(full.segments.map(\.share).reduce(0, +), 1)
        let headline = ContextBreakdown(usage: SessionUsage(used: 10, size: 100, estimated: true))
        XCTAssertEqual(headline.free, 90)
        XCTAssertTrue(headline.estimated)
    }

    /// The engine's `/context` Markdown, as iOS and other clients get it.
    func testContextMarkdownReportRendersItsTable() {
        let report = """
            Model: `gpt`  
            **Used 50.0k of 200.0k tokens · 25%** (measured provider prompt)

            `████████░░░░░░░░░░░░░░░░░░░░░░` 75% free

            | Category | Tokens | % of window |
            |---|---:|---:|
            | User messages | 20.0k | 10.0% |
            | Free | 150.0k | 75.0% |

            Categories are estimated from the saved history.
            """
        let tables = MarkdownBlock.parse(report).compactMap { block -> [[String]]? in
            if case .table(let headers, let rows) = block { return [headers] + rows }
            return nil
        }
        XCTAssertEqual(tables, [[["Category", "Tokens", "% of window"], ["User messages", "20.0k", "10.0%"], ["Free", "150.0k", "75.0%"]]])
    }
}
