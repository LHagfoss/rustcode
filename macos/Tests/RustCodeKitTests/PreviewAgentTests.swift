import XCTest
@testable import RustCodeKit

final class PreviewAgentTests: XCTestCase {
    private let local = URL(string: "http://localhost:3000/app")!
    private var origin: PreviewOrigin { PreviewOrigin(local)! }

    // MARK: Capability and requests

    func testInitializeAdvertisesThePreviewCapability() {
        let params = ACPRequests.initialize(appVersion: "1")
        XCTAssertEqual(params["clientCapabilities"]?["_meta"]?["rustcode"]?["preview"],
                       .number(Double(rustcodePreviewCapabilityVersion)))
        XCTAssertEqual(ACPMethod.preview, "_rustcode/preview")
    }

    func testRequestsParseEveryAction() throws {
        func operation(_ value: JSONValue) throws -> PreviewAgentOperation {
            try PreviewAgentRequest(params: ["sessionId": "s1", "operation": value]).operation
        }
        XCTAssertEqual(try operation(["action": "inspect"]), .inspect(maxChars: 6000))
        XCTAssertEqual(try operation(["action": "inspect", "maxChars": 5]), .inspect(maxChars: 200))
        XCTAssertEqual(try operation(["action": "screenshot"]), .screenshot)
        XCTAssertEqual(try operation(["action": "navigate", "url": "/x"]), .navigate("/x"))
        XCTAssertEqual(try operation(["action": "click", "selector": "#a"]), .click(selector: "#a"))
        XCTAssertEqual(try operation(["action": "fill", "selector": "#a", "value": ""]), .fill(selector: "#a", value: ""))
        XCTAssertEqual(try operation(["action": "select", "selector": "#a", "value": "b"]), .select(selector: "#a", value: "b"))
        XCTAssertEqual(try operation(["action": "scroll", "dy": -300]), .scroll(selector: nil, dy: -300))
        XCTAssertEqual(try operation(["action": "scroll", "selector": "#f"]), .scroll(selector: "#f", dy: nil))
    }

    func testMalformedRequestsAreRejected() {
        let bad: [JSONValue] = [
            ["operation": ["action": "inspect"]],
            ["sessionId": "", "operation": ["action": "inspect"]],
            ["sessionId": "s1"],
            ["sessionId": "s1", "operation": "inspect"],
            ["sessionId": "s1", "operation": ["action": "hover"]],
            ["sessionId": "s1", "operation": ["action": "click"]],
            ["sessionId": "s1", "operation": ["action": "click", "selector": " "]],
            ["sessionId": "s1", "operation": ["action": "click", "selector": 3]],
            ["sessionId": "s1", "operation": ["action": "fill", "selector": "#a"]],
            ["sessionId": "s1", "operation": ["action": "scroll"]],
            ["sessionId": "s1", "operation": ["action": "scroll", "dy": .number(1.5)]],
            ["sessionId": "s1", "operation": ["action": "navigate"]],
            ["sessionId": "s1", "operation": ["action": "click", "selector": .string(String(repeating: "a", count: 3000))]],
        ]
        for params in bad {
            XCTAssertThrowsError(try PreviewAgentRequest(params: params), "\(params)") { error in
                XCTAssertEqual((error as? PreviewAgentError)?.code, "invalid")
            }
        }
    }

    func testErrorsAnswerOkFalseWithAStableCode() {
        XCTAssertEqual(PreviewAgentError.denied.result["ok"], false)
        XCTAssertEqual(PreviewAgentError.denied.result["error"]?["code"], "denied")
        XCTAssertEqual(PreviewAgentError.approvalTimedOut.result["error"]?["code"], "approval_timeout")
        XCTAssertEqual(PreviewAgentError.outOfScope("x").result["error"]?["code"], "out_of_scope")
        XCTAssertEqual(PreviewAgentError.timedOut("x").result["error"]?["code"], "timeout")
        XCTAssertEqual(PreviewAgentError.noPage.result["error"]?["code"], "no_page")
    }

    // MARK: Origins and navigation scope

    func testOriginsNormalizeAndRejectRemoteHosts() {
        XCTAssertEqual(PreviewOrigin(URL(string: "http://LOCALHOST:3000/a")!), origin)
        XCTAssertEqual(PreviewOrigin(URL(string: "http://localhost/")!)?.port, 80)
        XCTAssertEqual(PreviewOrigin(URL(string: "http://[::1]:8080/")!)?.description, "http://[::1]:8080")
        XCTAssertNotEqual(PreviewOrigin(URL(string: "http://localhost:3001/")!), origin)
        XCTAssertNotEqual(PreviewOrigin(URL(string: "https://localhost:3000/")!), origin)
        XCTAssertNil(PreviewOrigin(URL(string: "https://example.com/")!))
        XCTAssertNil(PreviewOrigin(URL(string: "file:///etc/hosts")!))
    }

    func testNavigationStaysOnTheApprovedOrigin() throws {
        XCTAssertEqual(try PreviewOrigin.navigationTarget("/settings?tab=2", from: local, approved: origin).get().absoluteString,
                       "http://localhost:3000/settings?tab=2")
        XCTAssertEqual(try PreviewOrigin.navigationTarget("http://localhost:3000/x", from: nil, approved: origin).get().path, "/x")
        XCTAssertEqual(try PreviewOrigin.navigationTarget("localhost:3000", from: nil, approved: nil).get().absoluteString,
                       "http://localhost:3000/")
        for target in ["http://localhost:3001/", "https://example.com", "//example.com/a", "http://127.0.0.1:3000/",
                       "javascript:alert(1)", "file:///etc/passwd"] {
            guard case .failure(let error) = PreviewOrigin.navigationTarget(target, from: local, approved: origin) else {
                return XCTFail("\(target) was allowed")
            }
            XCTAssertEqual(error.code, "out_of_scope", target)
        }
        XCTAssertEqual(PreviewOrigin.navigationTarget("/x", from: nil, approved: origin), .failure(.noPage))
    }

    // MARK: Grants

    func testAccessIsPerChatPerProjectAndPerOrigin() {
        var access = PreviewAgentAccess()
        access.setWorkspace("/a")
        XCTAssertEqual(access.check(session: "s1", workspace: "/a", pageURL: local), .denied)
        access.grant(session: "s1", origin: origin)
        XCTAssertNil(access.check(session: "s1", workspace: "/a", pageURL: local))
        XCTAssertEqual(access.check(session: "s2", workspace: "/a", pageURL: local), .denied)
        XCTAssertEqual(access.check(session: "s1", workspace: "/b", pageURL: local), .revoked)
        XCTAssertEqual(access.check(session: "s1", workspace: "/a", pageURL: URL(string: "http://localhost:4000/")!)?.code,
                       "out_of_scope")
        XCTAssertFalse(access.setWorkspace("/a"))
        XCTAssertFalse(access.isEmpty)
        XCTAssertTrue(access.setWorkspace("/b"))
        XCTAssertTrue(access.isEmpty)
    }

    @MainActor
    private func gate(timeout: Duration = .seconds(30)) -> PreviewAgentGate {
        let gate = PreviewAgentGate(approvalTimeout: timeout)
        gate.setWorkspace("/project")
        return gate
    }

    @MainActor
    private func waitForPrompt(_ gate: PreviewAgentGate) async -> PreviewAgentApproval? {
        for _ in 0..<200 {
            if let pending = gate.pending { return pending }
            await Task.yield()
        }
        return nil
    }

    @MainActor
    func testNoGrantWithoutApprovalAndDenyGrantsNothing() async {
        let gate = gate()
        let request = Task { await gate.authorize(session: "s1", workspace: "/project", origin: origin) }
        guard let prompt = await waitForPrompt(gate) else { return XCTFail("no prompt") }
        XCTAssertEqual(prompt.sessionId, "s1")
        XCTAssertEqual(prompt.origin, origin)
        XCTAssertTrue(gate.access.isEmpty)
        gate.answer(prompt.id, allow: false)
        let outcome = await request.value
        XCTAssertEqual(outcome, .denied)
        XCTAssertTrue(gate.access.isEmpty)
        XCTAssertNil(gate.pending)
    }

    @MainActor
    func testAllowGrantsThisChatOnlyAndRevokeEndsIt() async {
        let gate = gate()
        let request = Task { await gate.authorize(session: "s1", workspace: "/project", origin: origin) }
        guard let prompt = await waitForPrompt(gate) else { return XCTFail("no prompt") }
        gate.answer(prompt.id, allow: true)
        let allowed = await request.value
        XCTAssertNil(allowed)
        // Granted: no second prompt for the same chat and origin.
        let again = await gate.authorize(session: "s1", workspace: "/project", origin: origin)
        XCTAssertNil(again)
        XCTAssertNil(gate.pending)
        // Another origin is out of scope, not a new prompt.
        let other = await gate.authorize(session: "s1", workspace: "/project", origin: PreviewOrigin(URL(string: "http://localhost:9999/")!)!)
        XCTAssertEqual(other?.code, "out_of_scope")
        gate.revokeAll()
        XCTAssertTrue(gate.access.isEmpty)
        let afterRevoke = Task { await gate.authorize(session: "s1", workspace: "/project", origin: origin) }
        guard let reprompt = await waitForPrompt(gate) else { return XCTFail("revoked access must ask again") }
        gate.answer(reprompt.id, allow: false)
        let denied = await afterRevoke.value
        XCTAssertEqual(denied, .denied)
    }

    @MainActor
    func testProjectChangeEndsGrantsAndOpenPrompts() async {
        let gate = gate()
        let first = Task { await gate.authorize(session: "s1", workspace: "/project", origin: origin) }
        guard let prompt = await waitForPrompt(gate) else { return XCTFail("no prompt") }
        gate.answer(prompt.id, allow: true)
        _ = await first.value
        let pending = Task { await gate.authorize(session: "s2", workspace: "/project", origin: origin) }
        guard let second = await waitForPrompt(gate) else { return XCTFail("no prompt") }
        gate.setWorkspace("/other")
        let outcome = await pending.value
        XCTAssertEqual(outcome, .revoked)
        XCTAssertTrue(gate.access.isEmpty)
        // A late answer to the old prompt does nothing.
        gate.answer(second.id, allow: true)
        XCTAssertTrue(gate.access.isEmpty)
        // Requests for the old project are refused outright.
        let stale = await gate.authorize(session: "s1", workspace: "/project", origin: origin)
        XCTAssertEqual(stale, .revoked)
    }

    @MainActor
    func testOnePromptAtATimeAndUnansweredPromptsTimeOut() async {
        let gate = gate(timeout: .milliseconds(50))
        let first = Task { await gate.authorize(session: "s1", workspace: "/project", origin: origin) }
        guard await waitForPrompt(gate) != nil else { return XCTFail("no prompt") }
        let second = await gate.authorize(session: "s2", workspace: "/project", origin: origin)
        XCTAssertEqual(second, .busy)
        let outcome = await first.value
        XCTAssertEqual(outcome, .approvalTimedOut)
        XCTAssertNil(gate.pending)
        XCTAssertTrue(gate.access.isEmpty)
    }

    // MARK: Timeouts

    func testTimeoutReleasesTheCallerEvenIfTheWorkNeverEnds() async {
        let started = ContinuousClock.now
        do {
            _ = try await withPreviewTimeout(.milliseconds(50), action: "click") { () async throws -> Int in
                // Like a page script that never returns: ignores cancellation.
                await withCheckedContinuation { (_: CheckedContinuation<Void, Never>) in }
                return 1
            }
            XCTFail("expected a timeout")
        } catch {
            XCTAssertEqual((error as? PreviewAgentError)?.code, "timeout")
        }
        XCTAssertLessThan(ContinuousClock.now - started, .seconds(5))
    }

    func testTimeoutPassesResultsAndErrorsThrough() async throws {
        let value = try await withPreviewTimeout(.seconds(5), action: "inspect") { 42 }
        XCTAssertEqual(value, 42)
        do {
            _ = try await withPreviewTimeout(.seconds(5), action: "click") { () async throws -> Int in
                throw PreviewAgentError.notFound("No element matches #x")
            }
            XCTFail("expected the error")
        } catch {
            XCTAssertEqual(error as? PreviewAgentError, .notFound("No element matches #x"))
        }
    }
}
