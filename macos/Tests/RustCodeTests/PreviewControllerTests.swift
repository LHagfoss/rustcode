import AppKit
import Network
import WebKit
import XCTest
import RustCodeKit
@testable import RustCode

/// A one-page HTTP server on 127.0.0.1, so the preview loads a real local
/// origin the way it does a dev server.
final class TestPageServer: @unchecked Sendable {
    private let listener: NWListener
    private let queue = DispatchQueue(label: "test-page-server")
    let html: String
    private(set) var port: UInt16 = 0

    init(html: String) throws {
        self.html = html
        let parameters = NWParameters.tcp
        parameters.requiredLocalEndpoint = .hostPort(host: "127.0.0.1", port: .any)
        listener = try NWListener(using: parameters)
    }

    func start() async throws {
        let ready = AsyncStream<UInt16?> { continuation in
            listener.stateUpdateHandler = { [listener] state in
                switch state {
                case .ready: continuation.yield(listener.port?.rawValue); continuation.finish()
                case .failed, .cancelled: continuation.yield(nil); continuation.finish()
                default: break
                }
            }
        }
        listener.newConnectionHandler = { [weak self] connection in self?.serve(connection) }
        listener.start(queue: queue)
        for await port in ready {
            guard let port else { throw URLError(.cannotConnectToHost) }
            self.port = port
        }
    }

    var url: URL { URL(string: "http://127.0.0.1:\(port)/")! }

    func stop() { listener.cancel() }

    private func serve(_ connection: NWConnection) {
        connection.start(queue: queue)
        connection.receive(minimumIncompleteLength: 1, maximumLength: 64 * 1024) { [html] _, _, _, _ in
            let body = Data(html.utf8)
            let head = "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: \(body.count)\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n"
            connection.send(content: Data(head.utf8) + body, completion: .contentProcessed { _ in connection.cancel() })
        }
    }
}

@MainActor
final class PreviewControllerTests: XCTestCase {
    private static let page = """
    <!doctype html><html><head><title>Todo</title></head><body>
    <h1>Todo list</h1>
    <p id="status">Nothing added</p>
    <label>New item <input id="item" name="item" type="text"></label>
    <input id="secret" name="secret" type="password" value="hunter2">
    <select id="plan"><option value="free">Free</option><option value="pro">Pro</option></select>
    <button id="add" onclick="document.getElementById('status').textContent = 'Added ' + document.getElementById('item').value + ' on ' + document.getElementById('plan').value">Add</button>
    <button id="warn" onclick="alert('careful'); document.getElementById('status').textContent = 'Warned'">Warn</button>
    <a id="away" href="https://example.com/">Elsewhere</a>
    <div style="height: 3000px"></div>
    </body></html>
    """

    private var server: TestPageServer!

    override func setUp() async throws {
        server = try TestPageServer(html: Self.page)
        try await server.start()
    }

    override func tearDown() async throws {
        server.stop()
        server = nil
    }

    private func makeController() -> PreviewController {
        // One fixed suite, emptied after each test: a suite per test leaves a
        // plist behind in ~/Library/Preferences on every run (#2620).
        let suite = "rustcode-preview-tests"
        let defaults = UserDefaults(suiteName: suite)!
        defaults.removePersistentDomain(forName: suite)
        addTeardownBlock { defaults.removePersistentDomain(forName: suite) }
        return PreviewController(store: PreviewURLStore(defaults: defaults), agent: PreviewAgentGate(approvalTimeout: .seconds(10)))
    }

    private func waitUntil(_ timeout: Duration = .seconds(10), _ condition: () -> Bool) async -> Bool {
        let deadline = ContinuousClock.now + timeout
        while !condition() {
            if ContinuousClock.now > deadline { return false }
            try? await Task.sleep(for: .milliseconds(20))
        }
        return true
    }

    private func loadPage(_ controller: PreviewController) async {
        controller.load(server.url)
        let loaded = await waitUntil { controller.state.phase == .loaded }
        XCTAssertTrue(loaded, "page did not load: \(controller.state)")
    }

    // MARK: Lifecycle (#2140)

    func testProjectSwitchReleasesTheOldWebView() async {
        let controller = makeController()
        controller.setWorkspace("/project-a")
        weak var oldView: WKWebView?
        oldView = controller.webView
        await loadPage(controller)
        let generation = controller.webViewGeneration
        controller.setWorkspace("/project-b")
        XCTAssertEqual(controller.webViewGeneration, generation + 1)
        XCTAssertNil(controller.createdWebView)
        let released = await waitUntil(.seconds(5)) { oldView == nil }
        XCTAssertTrue(released, "the previous project's web view is still alive")
        XCTAssertFalse(controller.webView === oldView)
    }

    func testControllerAndWebViewAreReleasedTogether() async {
        weak var weakController: PreviewController?
        weak var weakView: WKWebView?
        do {
            let controller = makeController()
            controller.setWorkspace("/project")
            weakController = controller
            weakView = controller.webView
            await loadPage(controller)
            // An agent action exercises the scripts and the KVO paths too.
            _ = await grantAndRun(controller, ["action": "inspect"])
        }
        let released = await waitUntil(.seconds(5)) { weakController == nil && weakView == nil }
        XCTAssertTrue(released, "controller or web view retained: controller \(weakController == nil ? "freed" : "alive"), view \(weakView == nil ? "freed" : "alive")")
    }

    func testRepeatedProjectSwitchesKeepAtMostOneWebView() async {
        let controller = makeController()
        var views: [() -> WKWebView?] = []
        for index in 0..<5 {
            controller.setWorkspace("/project-\(index)")
            weak var view = controller.webView
            views.append { view }
            await loadPage(controller)
        }
        let released = await waitUntil(.seconds(5)) { views.dropLast().allSatisfy { $0() == nil } }
        XCTAssertTrue(released, "old web views were kept: \(views.filter { $0() != nil }.count - 1)")
    }

    // MARK: Agent bridge (#2138)

    private func request(_ operation: JSONValue, session: String = "s1") -> JSONValue {
        ["sessionId": .string(session), "operation": operation]
    }

    /// Run a request, approving the access prompt if one appears.
    private func grantAndRun(_ controller: PreviewController, _ operation: JSONValue, session: String = "s1") async -> JSONValue {
        let task = Task { await controller.runAgentRequest(request(operation, session: session)) }
        let deadline = ContinuousClock.now + .seconds(2)
        while ContinuousClock.now < deadline {
            if controller.agent.pending != nil {
                controller.answerAgentApproval(allow: true)
                break
            }
            if controller.agent.access.origin(for: session) != nil { break }
            try? await Task.sleep(for: .milliseconds(10))
        }
        return await task.value
    }

    /// A blank page's runtime error comes back with the answer (session
    /// 01a16b37 spent ten rounds finding one the browser already knew).
    func testThePagesErrorsComeBackWithTheAnswer() async throws {
        let broken = try TestPageServer(html: """
        <!doctype html><html><head><title>Broken</title></head><body>
        <div id="root"></div>
        <script>Promise.reject(new TypeError('no data')); tutorial.show();</script>
        <button id="boom" onclick="missingHandler()">Boom</button>
        </body></html>
        """)
        try await broken.start()
        defer { broken.stop() }
        let controller = makeController()
        controller.setWorkspace("/project")
        controller.load(broken.url)
        let loaded = await waitUntil { controller.state.phase == .loaded }
        XCTAssertTrue(loaded)

        let page = await grantAndRun(controller, ["action": "inspect"])
        XCTAssertEqual(page["ok"], true, "\(page)")
        let errors = page["errors"]?.arrayValue?.compactMap(\.stringValue) ?? []
        XCTAssertTrue(errors.contains { $0.contains("tutorial") }, "\(errors)")
        XCTAssertTrue(errors.contains { $0.contains("Unhandled promise rejection: TypeError: no data") }, "\(errors)")
        let clicked = await controller.runAgentRequest(request(["action": "click", "selector": "#boom"]))
        let after = clicked["errors"]?.arrayValue?.compactMap(\.stringValue) ?? []
        XCTAssertTrue(after.contains { $0.contains("missingHandler") }, "\(after)")
    }

    func testAgentActionsRunInThePageAfterApproval() async {
        let controller = makeController()
        controller.setWorkspace("/project")
        await loadPage(controller)

        let page = await grantAndRun(controller, ["action": "inspect"])
        XCTAssertEqual(page["ok"], true, "\(page)")
        XCTAssertEqual(page["title"], "Todo")
        XCTAssertTrue(page["text"]?.stringValue?.contains("Nothing added") == true)
        let elements = page["elements"]?.arrayValue ?? []
        XCTAssertTrue(elements.contains { $0["selector"] == "#add" && $0["text"] == "Add" }, "\(elements)")
        let secret = elements.first { $0["selector"] == "#secret" }
        XCTAssertNotNil(secret)
        XCTAssertNil(secret?["value"], "a password field's value must never be returned")

        let filled = await controller.runAgentRequest(request(["action": "fill", "selector": "#item", "value": "milk"]))
        XCTAssertEqual(filled["ok"], true, "\(filled)")
        let selected = await controller.runAgentRequest(request(["action": "select", "selector": "#plan", "value": "Pro"]))
        XCTAssertEqual(selected["ok"], true, "\(selected)")
        let clicked = await controller.runAgentRequest(request(["action": "click", "selector": "#add"]))
        XCTAssertEqual(clicked["ok"], true, "\(clicked)")
        let after = await controller.runAgentRequest(request(["action": "inspect"]))
        XCTAssertTrue(after["text"]?.stringValue?.contains("Added milk on pro") == true, "\(after)")

        let scrolled = await controller.runAgentRequest(request(["action": "scroll", "dy": 500]))
        XCTAssertEqual(scrolled["ok"], true, "\(scrolled)")
        XCTAssertGreaterThan(scrolled["scrollY"]?.doubleValue ?? 0, 0)

        let missing = await controller.runAgentRequest(request(["action": "click", "selector": "#nope"]))
        XCTAssertEqual(missing["error"]?["code"], "not_found")
        let invalid = await controller.runAgentRequest(request(["action": "click", "selector": "##"]))
        XCTAssertEqual(invalid["error"]?["code"], "invalid")
    }

    func testPageDialogsDoNotBlockAnAgentAction() async {
        let controller = makeController()
        controller.setWorkspace("/project")
        await loadPage(controller)
        let clicked = await grantAndRun(controller, ["action": "click", "selector": "#warn"])
        XCTAssertEqual(clicked["ok"], true, "\(clicked)")
        XCTAssertTrue(clicked["note"]?.stringValue?.contains("careful") == true, "\(clicked)")
        XCTAssertFalse(controller.isAgentActing)
    }

    func testNavigationOutsideTheApprovedOriginIsBlocked() async {
        let controller = makeController()
        controller.setWorkspace("/project")
        await loadPage(controller)
        _ = await grantAndRun(controller, ["action": "inspect"])
        let external = await controller.runAgentRequest(request(["action": "navigate", "url": "https://example.com/"]))
        XCTAssertEqual(external["error"]?["code"], "out_of_scope")
        let otherPort = await controller.runAgentRequest(request(["action": "navigate", "url": "http://127.0.0.1:1/"]))
        XCTAssertEqual(otherPort["error"]?["code"], "out_of_scope")
        // A link that leaves the preview is blocked, not opened in the browser.
        let away = await controller.runAgentRequest(request(["action": "click", "selector": "#away"]))
        XCTAssertEqual(away["ok"], true, "\(away)")
        XCTAssertTrue(away["note"]?.stringValue?.contains("Blocked a navigation outside the preview") == true, "\(away)")
        XCTAssertEqual(controller.createdWebView?.url?.host, "127.0.0.1")
        let path = await controller.runAgentRequest(request(["action": "navigate", "url": "/second"]))
        XCTAssertEqual(path["ok"], true, "\(path)")
        XCTAssertEqual(controller.createdWebView?.url?.path, "/second")
    }

    func testDenialRevocationAndProjectSwitchStopTheAgent() async {
        let controller = makeController()
        controller.setWorkspace("/project")
        await loadPage(controller)

        // Denied: nothing runs.
        let denied = Task { await controller.runAgentRequest(self.request(["action": "inspect"])) }
        let prompted = await waitUntil(.seconds(2)) { controller.agentApproval != nil }
        XCTAssertTrue(prompted)
        controller.answerAgentApproval(allow: false)
        let deniedResult = await denied.value
        XCTAssertEqual(deniedResult["ok"], false)
        XCTAssertEqual(deniedResult["error"]?["code"], "denied")
        XCTAssertTrue(controller.agentAccess.isEmpty)

        // Allowed, then revoked from the panel: asks again.
        let allowed = await grantAndRun(controller, ["action": "inspect"])
        XCTAssertEqual(allowed["ok"], true)
        XCTAssertFalse(controller.agentAccess.isEmpty)
        controller.revokeAgentAccess()
        XCTAssertTrue(controller.agentAccess.isEmpty)
        let again = Task { await controller.runAgentRequest(self.request(["action": "inspect"])) }
        let reprompted = await waitUntil(.seconds(2)) { controller.agentApproval != nil }
        XCTAssertTrue(reprompted, "a revoked grant must ask again")
        // Switching project while the prompt waits ends it.
        controller.setWorkspace("/other")
        let switched = await again.value
        XCTAssertEqual(switched["error"]?["code"], "revoked")
        XCTAssertNil(controller.agentApproval)
    }

    func testNoPageAndMalformedRequestsAreRefused() async {
        let controller = makeController()
        controller.setWorkspace("/project")
        let noPage = await controller.runAgentRequest(request(["action": "inspect"]))
        XCTAssertEqual(noPage["error"]?["code"], "no_page")
        let malformed = await controller.runAgentRequest(["operation": ["action": "inspect"]])
        XCTAssertEqual(malformed["error"]?["code"], "invalid")
        XCTAssertNil(controller.agentApproval, "a refused request must not prompt")
    }

    func testScreenshotIsAnImageOfThePage() async throws {
        let controller = makeController()
        controller.setWorkspace("/project")
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 800, height: 600), styleMask: [.borderless],
                              backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        controller.webView.frame = window.contentView!.bounds
        window.contentView!.addSubview(controller.webView)
        window.orderBack(nil)
        defer { window.close() }
        await loadPage(controller)
        let shot = await grantAndRun(controller, ["action": "screenshot"])
        XCTAssertEqual(shot["ok"], true, "\(shot)")
        let data = try XCTUnwrap(shot["image"]?["data"]?.stringValue.flatMap { Data(base64Encoded: $0) })
        XCTAssertTrue(data.starts(with: [0x89, 0x50, 0x4E, 0x47]) || data.starts(with: [0xFF, 0xD8, 0xFF]))
        XCTAssertLessThanOrEqual(shot["image"]?["width"]?.intValue ?? .max, PreviewController.maxScreenshotWidth)
    }
}
