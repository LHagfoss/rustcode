import AppKit
import WebKit
import XCTest
import RustCodeKit
@testable import RustCode

/// Smart UI (#2484) against real WebKit: isolation checks and the resource
/// workload recorded in docs/smart-ui.md. WebKit makes it slow, so it only
/// runs when asked:
///
///     RUSTCODE_SMART_UI_SOAK=1 swift test --package-path macos --filter SmartUISoakTests
@MainActor
final class SmartUISoakTests: XCTestCase {
    private var window: NSWindow!
    private var container: NSView!

    override func setUp() async throws {
        try XCTSkipUnless(ProcessInfo.processInfo.environment["RUSTCODE_SMART_UI_SOAK"] == "1",
                          "Set RUSTCODE_SMART_UI_SOAK=1 to run the Smart UI WebKit checks.")
        window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 700, height: 800), styleMask: [.titled],
                          backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.orderFront(nil)
        container = NSView(frame: NSRect(x: 0, y: 0, width: 640, height: 600))
        window.contentView!.addSubview(container)
    }

    override func tearDown() async throws {
        SmartUIHost.shared.stop()
        window?.close()
    }

    private func block(_ html: String) -> SmartUIBlock {
        for case .smartUI(let block) in MarkdownBlock.parse("```rustcode-ui v1\n\(html)\n```", smartUI: true) {
            return block
        }
        fatalError("not a Smart UI block")
    }

    /// Run `html` in the shared host and wait until its page has loaded.
    @discardableResult
    private func run(_ html: String) async throws -> (SmartUIWebView, Duration) {
        let host = SmartUIHost.shared
        let clock = ContinuousClock()
        let started = clock.now
        host.activate(block(html))
        let deadline = clock.now + .seconds(10)
        while host.webView == nil, clock.now < deadline { try await Task.sleep(for: .milliseconds(5)) }
        let view = try XCTUnwrap(host.webView)
        host.attach(to: container)
        while view.isLoading || view.url == nil, clock.now < deadline { try await Task.sleep(for: .milliseconds(5)) }
        return (view, clock.now - started)
    }

    private func evaluate(_ script: String, in view: WKWebView) async -> Any? {
        await withCheckedContinuation { continuation in
            view.evaluateJavaScript(script) { result, _ in continuation.resume(returning: result) }
        }
    }

    func testPagesCannotReachTheNetworkNavigateOrOpenWindows() async throws {
        let (view, _) = try await run("""
            <title>Probe</title><p id="out">pending</p>
            <img src="https://example.com/pixel.png" onerror="window.img='blocked'" onload="window.img='loaded'">
            <script>
            window.results = {};
            fetch('https://example.com/').then(() => results.fetch = 'reached', () => results.fetch = 'blocked');
            try { const ws = new WebSocket('wss://example.com/'); ws.onerror = () => results.ws = 'blocked'; } catch (e) { results.ws = 'blocked'; }
            results.popup = window.open('https://example.com/') ? 'opened' : 'blocked';
            setTimeout(() => { location.href = 'https://example.com/'; }, 50);
            setTimeout(() => { location.href = 'file:///etc/hosts'; }, 100);
            results.cookie = (() => { try { document.cookie = 'a=1'; return document.cookie || 'none'; } catch (e) { return 'blocked'; } })();
            results.bridge = (window.webkit && window.webkit.messageHandlers && Object.keys(window.webkit.messageHandlers).length) ? 'present' : 'absent';
            </script>
            """)
        try await Task.sleep(for: .seconds(2))
        XCTAssertEqual(view.url?.scheme, "about", "the page navigated away")
        let results = await evaluate("JSON.stringify(Object.assign({img: window.img}, window.results))", in: view) as? String
        print("[smart-ui] isolation \(results ?? "nil")")
        let decoded = try XCTUnwrap(results.flatMap { try? JSONSerialization.jsonObject(with: Data($0.utf8)) as? [String: String] })
        XCTAssertEqual(decoded["fetch"], "blocked")
        XCTAssertEqual(decoded["img"], "blocked")
        XCTAssertEqual(decoded["popup"], "blocked")
        XCTAssertEqual(decoded["bridge"], "absent")
        XCTAssertNotEqual(decoded["ws"], "open")
    }

    func testABusyPageIsStoppedAndShowsItsSource() async throws {
        let html = "<title>Spin</title><script>setTimeout(() => { for (;;) {} }, 300);</script>"
        try await run(html)
        let host = SmartUIHost.shared
        let deadline = ContinuousClock.now + .seconds(10)
        while host.webView != nil, ContinuousClock.now < deadline { try await Task.sleep(for: .milliseconds(50)) }
        XCTAssertNil(host.webView, "a page stuck in a loop kept running")
        XCTAssertEqual(host.failures[block(html).id], "the page stopped responding")
    }

    func testAnOversizedDOMIsStopped() async throws {
        let html = "<title>Big</title><div id=d></div><script>for (let i = 0; i < 8000; i++) d.appendChild(document.createElement('span'));</script>"
        try await run(html)
        let host = SmartUIHost.shared
        let deadline = ContinuousClock.now + .seconds(5)
        while host.webView != nil, ContinuousClock.now < deadline { try await Task.sleep(for: .milliseconds(50)) }
        XCTAssertNotNil(host.failures[block(html).id])
    }

    func testTheViewFitsItsContentHeight() async throws {
        try await run("<title>Tall</title><div style='height:300px'>x</div>")
        let deadline = ContinuousClock.now + .seconds(5)
        while SmartUIHost.shared.height < 299, ContinuousClock.now < deadline { try await Task.sleep(for: .milliseconds(50)) }
        XCTAssertEqual(SmartUIHost.shared.height, 300, accuracy: 20)
    }

    // MARK: Resources

    private func processStats(_ pid: Int32) -> (rssMB: Double, cpuSeconds: Double)? {
        let ps = Process()
        ps.executableURL = URL(fileURLWithPath: "/bin/ps")
        ps.arguments = ["-o", "rss=,time=", "-p", String(pid)]
        let pipe = Pipe()
        ps.standardOutput = pipe
        do { try ps.run() } catch { return nil }
        ps.waitUntilExit()
        let fields = String(decoding: pipe.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
            .split(whereSeparator: \.isWhitespace)
        guard fields.count >= 2, let rss = Double(fields[0]) else { return nil }
        let seconds = fields[1].split(separator: ":").reduce(0.0) { $0 * 60 + (Double($1) ?? 0) }
        return (rss / 1024, seconds)
    }

    /// WebKit's private process identifier, read defensively: test-only.
    private func webContentPID(_ view: WKWebView?) -> Int32? {
        guard let view, view.responds(to: NSSelectorFromString("_webProcessIdentifier")),
              let pid = view.value(forKey: "_webProcessIdentifier") as? Int32, pid > 0 else { return nil }
        return pid
    }

    private func sample(_ label: String) {
        let app = processStats(getpid())
        let web = webContentPID(SmartUIHost.shared.webView).flatMap(processStats)
        let webText = web.map { String(format: "%6.1f MB cpu %5.2f s", $0.rssMB, $0.cpuSeconds) } ?? "(none)"
        print(String(format: "[smart-ui] %-30@ app %6.1f MB cpu %5.2f s | WebContent %@",
                     label as NSString, app?.rssMB ?? 0, app?.cpuSeconds ?? 0, webText))
    }

    /// A filterable, sortable 200-row table: what a typical requested view is.
    private static func table(_ seed: Int) -> String {
        """
        <title>Results \(seed)</title><meta name="description" content="Filter and sort">
        <style>table{border-collapse:collapse;width:100%}td,th{padding:2px 6px;border-bottom:1px solid #8884}</style>
        <input id="q" placeholder="Filter"><table><thead><tr><th onclick="sortBy(0)">Name</th><th onclick="sortBy(1)">ms</th></tr></thead><tbody id="b"></tbody></table>
        <script>
        let rows = Array.from({length: 200}, (_, i) => ['test_' + ((i * \(seed + 7)) % 997), (i * 37 + \(seed)) % 500]);
        const body = document.getElementById('b'), q = document.getElementById('q');
        function draw() { body.innerHTML = rows.filter(r => r[0].includes(q.value)).map(r => '<tr><td>' + r[0] + '</td><td>' + r[1] + '</td></tr>').join(''); }
        function sortBy(c) { rows.sort((a, b) => a[c] > b[c] ? 1 : -1); draw(); }
        q.oninput = draw; draw();
        </script>
        """
    }

    func testResourceWorkload() async throws {
        print("[smart-ui] macOS \(ProcessInfo.processInfo.operatingSystemVersionString)")
        sample("baseline (no web view)")
        let (first, cold) = try await run(Self.table(0))
        print("[smart-ui] cold start to loaded: \(cold)")
        try await Task.sleep(for: .seconds(2))
        sample("first view, loaded")
        let firstPID = webContentPID(first)
        try await Task.sleep(for: .seconds(10))
        sample("first view, +10 s idle")

        SmartUIHost.shared.detach(from: container)
        try await Task.sleep(for: .seconds(10))
        sample("hidden +10 s")
        SmartUIHost.shared.attach(to: container)

        var pids: Set<Int32> = firstPID.map { [$0] } ?? []
        var warm: [Duration] = []
        for index in 1...30 {
            let (view, elapsed) = try await run(Self.table(index))
            warm.append(elapsed)
            if let pid = webContentPID(view) { pids.insert(pid) }
            if index % 10 == 0 { sample("after \(index) different views") }
        }
        let average = warm.reduce(Duration.zero, +) / warm.count
        print("[smart-ui] warm start to loaded, average of 30: \(average)")
        try await Task.sleep(for: .seconds(5))
        let current = webContentPID(SmartUIHost.shared.webView)
        let lingering = pids.subtracting(current.map { [$0] } ?? []).filter { processStats($0) != nil }
        print("[smart-ui] WebContent processes used: \(pids.count); old ones still running: \(lingering.count)")
        XCTAssertLessThanOrEqual(lingering.count, 1, "old Smart UI processes kept running")

        SmartUIHost.shared.stop()
        try await Task.sleep(for: .seconds(5))
        sample("stopped")
        if let current { XCTAssertNil(processStats(current), "the last WebContent process outlived teardown") }
    }
    // MARK: - smart_ui_preview renders

    private func preview(_ html: String) async -> JSONValue {
        let key = SmartUI.enabledKey
        let was = UserDefaults.standard.object(forKey: key)
        UserDefaults.standard.set(true, forKey: key)
        defer { UserDefaults.standard.set(was, forKey: key) }
        return await SmartUIPreviewRenderer.shared.render(["sessionId": "s", "html": .string(html)])
    }

    func testPreviewReturnsAScreenshotSizeAndPageErrors() async throws {
        let result = await preview("""
            <title>Probe</title>
            <div style="height: 900px; background: AccentColor">tall</div>
            <script>fetch('https://example.com/').catch(() => {}); undefinedFunction();</script>
            """)
        XCTAssertEqual(result["ok"], .bool(true), "\(result)")
        XCTAssertEqual(result["title"], .string("Probe"))
        XCTAssertEqual(result["width"]?.doubleValue, SmartUI.previewWidth)
        XCTAssertGreaterThanOrEqual(result["contentHeight"]?.intValue ?? 0, 900)
        let data = try XCTUnwrap(result["image"]?["data"]?.stringValue.flatMap { Data(base64Encoded: $0) })
        XCTAssertNotNil(NSImage(data: data))
        let errors = result["errors"]?.arrayValue?.compactMap(\.stringValue) ?? []
        print("[smart-ui] preview errors \(errors)")
        XCTAssertTrue(errors.contains { $0.contains("undefinedFunction") }, "\(errors)")
    }

    func testPreviewOfABusyLoopFailsInsteadOfHanging() async {
        let clock = ContinuousClock()
        let started = clock.now
        let result = await preview("<title>Spin</title><script>setTimeout(() => { for (;;) {} }, 0);</script>")
        XCTAssertEqual(result["ok"], .bool(false), "\(result)")
        XCTAssertLessThan(clock.now - started, .seconds(15))
    }
}
