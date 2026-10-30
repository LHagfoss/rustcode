import XCTest
@testable import RustCodeKit

final class OrchestrationTests: XCTestCase {
    private let a = Orchestration.Thread(id: "a", title: "OAuth app", folder: "/p", updatedAt: Date(timeIntervalSince1970: 10))
    private let b = Orchestration.Thread(id: "b", title: "OAuth server", folder: "/p", updatedAt: Date(timeIntervalSince1970: 20), isRunning: true)
    private let other = Orchestration.Thread(id: "c", title: "Elsewhere", folder: "/q")

    func testCallsParseAndBadOnesFail() {
        XCTAssertEqual(try Orchestration.Call.parse(["tool": "list_sessions", "args": ["query": "oauth"]]).get(), .list(query: "oauth"))
        XCTAssertEqual(try Orchestration.Call.parse(["tool": "message_session", "args": ["session_id": "b", "message": "hi"]]).get(),
                       .message(target: "b", text: "hi"))
        XCTAssertEqual(try Orchestration.Call.parse(["tool": "wait_session", "args": ["link_id": "a/1", "timeout_ms": 99_999_999]]).get(),
                       .wait(links: ["a/1"], timeoutMs: Orchestration.maxWaitMs))
        let long = String(repeating: "x", count: Orchestration.maxBytes + 1)
        XCTAssertThrowsError(try Orchestration.Call.parse(["tool": "message_session", "args": ["session_id": "b", "message": .string(long)]]).get())
        XCTAssertThrowsError(try Orchestration.Call.parse(["tool": "wait_session", "args": [:]]).get())
    }

    func testTheListIsThisProjectsOtherThreadsOnly() {
        let listed = Orchestration.list([a, b, other], from: a, query: "")
        XCTAssertEqual(listed["sessions"]?.arrayValue?.compactMap { $0["session_id"]?.stringValue }, ["b"])
        XCTAssertEqual(listed["sessions"]?.arrayValue?.first?["working"]?.boolValue, true)
        XCTAssertEqual(Orchestration.list([a, b], from: a, query: "nothing")["sessions"]?.arrayValue?.count, 0)
    }

    func testTheRulesKeepToTheProjectAndRefuseChains() {
        var book = LinkBook()
        XCTAssertNil(Orchestration.check(caller: a, target: b, book: book))
        XCTAssertEqual(Orchestration.check(caller: a, target: nil, book: book)?.code, "not_found")
        XCTAssertEqual(Orchestration.check(caller: a, target: a, book: book)?.code, "not_allowed")
        XCTAssertEqual(Orchestration.check(caller: a, target: other, book: book)?.code, "not_allowed")
        book.links.append(OrchestrationLink(id: book.nextId(for: "a"), orchestrator: "a", target: "b", orchestratorTitle: "A",
                                            targetTitle: "B", message: "m", state: .working))
        // b works for a, so b cannot pass work on, and nobody else can drive b.
        XCTAssertEqual(Orchestration.check(caller: b, target: a, book: book)?.code, "not_allowed")
        let third = Orchestration.Thread(id: "d", title: "D", folder: "/p")
        XCTAssertEqual(Orchestration.check(caller: third, target: b, book: book)?.code, "target_linked")
        XCTAssertNil(Orchestration.check(caller: a, target: b, book: book), "a may queue more for b")
        XCTAssertEqual(book.nextId(for: "a"), "a/2")
    }

    func testTheMailboxAndActiveLinksAreBounded() {
        var book = LinkBook()
        for _ in 0..<Orchestration.maxActiveLinks {
            book.links.append(OrchestrationLink(id: book.nextId(for: "a"), orchestrator: "a", target: "b", orchestratorTitle: "A",
                                                targetTitle: "B", message: "m", state: .queued))
        }
        XCTAssertEqual(Orchestration.check(caller: a, target: b, book: book)?.code, "busy")
    }

    func testARestartInterruptsWhatWasUnderWayAndADeletedTargetBreaks() throws {
        var book = LinkBook()
        book.links.append(OrchestrationLink(id: "a/1", orchestrator: "a", target: "b", orchestratorTitle: "A",
                                            targetTitle: "B", message: "m", state: .working))
        book.grants["a"] = ["b"]
        let url = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString + ".json")
        defer { try? FileManager.default.removeItem(at: url) }
        try book.save(to: url)
        var loaded = LinkBook.load(from: url)
        XCTAssertEqual(loaded.link("a/1")?.state, .interrupted)
        XCTAssertEqual(loaded.grants["a"], ["b"])
        loaded.links[0].state = .working
        loaded.forget(thread: "b")
        XCTAssertEqual(loaded.link("a/1")?.state, .failed)
        XCTAssertEqual(loaded.grants["a"], [])
    }

    func testDeliveryIsAttributedAndAnswersAreCapped() {
        XCTAssertTrue(Orchestration.delivered("Which URI?", from: "OAuth app").hasPrefix("From thread “OAuth app” (agent):"))
        let (text, cut) = Orchestration.capped(String(repeating: "é", count: Orchestration.maxBytes))
        XCTAssertTrue(cut)
        XCTAssertLessThanOrEqual(text.utf8.count, Orchestration.maxBytes)
        var link = OrchestrationLink(id: "a/1", orchestrator: "a", target: "b", orchestratorTitle: "A", targetTitle: "B",
                                     message: "m", state: .needsUser)
        XCTAssertNotNil(link.result["note"])
        link.state = .done
        link.answer = "http://localhost/cb"
        XCTAssertEqual(link.result["answer"]?.stringValue, "http://localhost/cb")
        XCTAssertEqual(Orchestration.Failure("denied", "no").json["error"]?["code"]?.stringValue, "denied")
    }
}

extension OrchestrationTests {
    func testTheHostIsAdvertisedOnlyWhenOn() {
        XCTAssertNil(ACPRequests.initialize(appVersion: "1")["clientCapabilities"]?["_meta"]?["rustcode"]?["orchestration"])
        XCTAssertEqual(ACPRequests.initialize(appVersion: "1", orchestration: true)["clientCapabilities"]?["_meta"]?["rustcode"]?["orchestration"]?.intValue, 1)
    }
}
