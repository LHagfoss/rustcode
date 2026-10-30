import XCTest
@testable import RustCodeKit

final class PullWatchesTests: XCTestCase {
    private let watch: JSONValue = [
        "sessionId": "s1", "cwd": "/r", "number": 7, "title": "feat: x", "url": "https://github.com/o/r/pull/7",
        "status": "watching", "failures": 0, "createdAt": 1, "nextCheckAt": 2,
        "events": [["kind": "checks_failed", "message": "Checks failed: test", "at": "2026-10-30T10:00:00+00:00"]],
    ]

    func testAWatchAndItsEventsDecode() throws {
        let decoded = try XCTUnwrap(PullWatch(json: watch))
        XCTAssertEqual(decoded.id, "s1#7")
        XCTAssertTrue(decoded.isWatching)
        XCTAssertEqual(decoded.label, "Watching #7")
        XCTAssertEqual(decoded.events.first?.kind, "checks_failed")
        XCTAssertTrue(decoded.events.first?.needsAttention ?? false)
        XCTAssertNotNil(decoded.events.first?.at)
        XCTAssertEqual(PullWatch.list(result: ["watches": [watch, ["number": 1]]]).count, 1)
    }

    func testAStoppedWatchSaysWhy() throws {
        var fields = watch.objectValue ?? [:]
        fields["status"] = "stopped"
        fields["stopReason"] = "merged"
        let stopped = try XCTUnwrap(PullWatch(json: .object(fields)))
        XCTAssertFalse(stopped.isWatching)
        XCTAssertEqual(stopped.label, "#7 · merged")
    }

    func testAnUpdateCarriesItsNewEventsAndADraftAsksFirst() throws {
        let update = try XCTUnwrap(PullWatchUpdate(params: ["watch": watch, "events": [["kind": "conflicts", "message": "Has merge conflicts with its base"]]]))
        XCTAssertEqual(update.events.map(\.kind), ["conflicts"])
        let draft = update.watch.draft(for: update.events)
        XCTAssertTrue(draft.contains("#7 (feat: x)"))
        XCTAssertTrue(draft.contains("- Has merge conflicts"))
        XCTAssertTrue(draft.contains("before changing anything"))
        XCTAssertNil(PullWatchUpdate(params: ["events": []]))
    }

    func testRequests() {
        let start = ACPRequests.pullWatchStart(session: "s1", cwd: URL(fileURLWithPath: "/r"), number: 7)
        XCTAssertEqual(start["cwd"]?.stringValue, "/r")
        XCTAssertEqual(ACPRequests.pullWatchStop(session: "s1", number: 7, remove: true)["remove"]?.boolValue, true)
    }
}
