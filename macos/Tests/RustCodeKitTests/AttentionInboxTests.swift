import Foundation
import XCTest
@testable import RustCodeKit

final class AttentionInboxTests: XCTestCase {
    private let t0 = Date(timeIntervalSince1970: 1_000)

    private func approval(_ id: String, in session: String) -> AttentionInbox.Request {
        .init(id: id, sessionId: session, question: nil, title: "run_command")
    }

    func testUnresolvedRequestsLeadOldestFirstThenNoticesNewestFirst() {
        var inbox = AttentionInbox()
        _ = inbox.entries(requests: [approval("a", in: "s1")], stopped: [:], unseen: ["s3"],
                          known: ["s1", "s2", "s3"], now: t0)
        let entries = inbox.entries(
            requests: [approval("a", in: "s1"),
                       .init(id: "q", sessionId: "s2", question: "Which branch?", title: "ask_question")],
            stopped: ["s4": "Stopped: the provider timed out"],
            unseen: ["s3", "s4"],
            known: ["s1", "s2", "s3", "s4"],
            now: t0.addingTimeInterval(60)
        )
        XCTAssertEqual(entries.map(\.sessionId), ["s1", "s2", "s4", "s3"])
        XCTAssertEqual(entries[0].since, t0, "an entry keeps its first-seen time")
        XCTAssertEqual(entries[1].kind, .question(text: "Which branch?"))
        // The stopped thread is also unseen; it is listed once, as stopped.
        XCTAssertEqual(entries.filter { $0.sessionId == "s4" }.count, 1)
        XCTAssertEqual(entries[2].kind, .stopped(reason: "Stopped: the provider timed out"))
    }

    func testRequestsCannotBeDismissedAndLeaveOnlyWhenAnswered() {
        var inbox = AttentionInbox()
        let entries = inbox.entries(requests: [approval("a", in: "s1")], stopped: [:], unseen: [],
                                    known: ["s1"], now: t0)
        XCTAssertFalse(entries[0].isDismissible)
        inbox.dismiss(entries[0])
        XCTAssertEqual(inbox.entries(requests: [approval("a", in: "s1")], stopped: [:], unseen: [],
                                     known: ["s1"], now: t0).count, 1)
        // Answered in its thread: gone.
        XCTAssertTrue(inbox.entries(requests: [], stopped: [:], unseen: [], known: ["s1"], now: t0).isEmpty)
    }

    func testDismissedNoticesStayHiddenUntilTheyChange() {
        var inbox = AttentionInbox()
        let stopped = ["s1": "Stopped: the provider timed out"]
        let first = inbox.entries(requests: [], stopped: stopped, unseen: ["s2"], known: ["s1", "s2"], now: t0)
        XCTAssertEqual(first.count, 2)
        first.forEach { inbox.dismiss($0) }
        XCTAssertTrue(inbox.entries(requests: [], stopped: stopped, unseen: ["s2"], known: ["s1", "s2"], now: t0).isEmpty)
        // The turn stops again for another reason: a new entry.
        let again = inbox.entries(requests: [], stopped: ["s1": "Stopped: the connection closed"], unseen: ["s2"],
                                  known: ["s1", "s2"], now: t0)
        XCTAssertEqual(again.map(\.kind), [.stopped(reason: "Stopped: the connection closed")])
        // Once read, the finished notice goes; when it finishes again later it comes back.
        _ = inbox.entries(requests: [], stopped: [:], unseen: [], known: ["s1", "s2"], now: t0)
        XCTAssertEqual(inbox.entries(requests: [], stopped: [:], unseen: ["s2"], known: ["s1", "s2"],
                                     now: t0.addingTimeInterval(5)).map(\.kind), [.finished])
    }

    func testThreadsThatAreGoneAreLeftOut() {
        var inbox = AttentionInbox()
        let entries = inbox.entries(requests: [approval("a", in: "gone")], stopped: ["gone": "x"], unseen: ["gone"],
                                    known: [], now: t0)
        XCTAssertTrue(entries.isEmpty)
    }
}
