import AppKit
import WebKit
import XCTest
import RustCodeKit
@testable import RustCode

/// The preview resource workload from #2140, run against the real
/// `PreviewController` and WebKit. It takes a few minutes, so it only runs
/// when asked:
///
///     RUSTCODE_PREVIEW_SOAK=1 swift test --package-path macos --filter PreviewSoakTests
///
/// It prints memory (RSS) and CPU time of this process and of the page's
/// WebContent process while visible, hidden, across reloads and across
/// project switches, and fails only if old WebContent processes or web
/// views are never released. Numbers are recorded in docs/preview.md.
@MainActor
final class PreviewSoakTests: XCTestCase {
    /// A busy single-page app: a large DOM, a 60 Hz timer, a canvas
    /// animation and fresh allocations on every load.
    private static let page = """
    <!doctype html><html><head><title>Soak</title></head><body>
    <canvas id="c" width="512" height="512"></canvas><p id="tick">0</p><div id="list"></div>
    <script>
    const list = document.getElementById('list');
    for (let i = 0; i < 5000; i++) { const row = document.createElement('div'); row.textContent = 'Row ' + i + ' ' + 'x'.repeat(40); list.appendChild(row); }
    window.payload = Array.from({ length: 200000 }, (_, i) => ({ i, s: 'item' + i }));
    let n = 0; setInterval(() => { document.getElementById('tick').textContent = String(++n); }, 16);
    const ctx = document.getElementById('c').getContext('2d');
    const frame = (t) => { ctx.fillStyle = 'hsl(' + (t / 10 % 360) + ',70%,50%)'; ctx.fillRect(Math.random() * 512, Math.random() * 512, 40, 40); requestAnimationFrame(frame); };
    requestAnimationFrame(frame);
    </script></body></html>
    """

    private struct Sample: CustomStringConvertible {
        var label: String
        var appRSSMB: Double
        var appCPUSeconds: Double
        var webRSSMB: Double?
        var webCPUSeconds: Double?
        var description: String {
            let web = webRSSMB.map { String(format: "%7.1f MB  cpu %7.2f s", $0, webCPUSeconds ?? 0) } ?? "      (none)"
            return String(format: "%-28@ app %7.1f MB  cpu %7.2f s | WebContent %@", label as NSString, appRSSMB, appCPUSeconds, web)
        }
    }

    private func processStats(_ pid: Int32) -> (rssMB: Double, cpuSeconds: Double)? {
        let ps = Process()
        ps.executableURL = URL(fileURLWithPath: "/bin/ps")
        ps.arguments = ["-o", "rss=,time=", "-p", String(pid)]
        let pipe = Pipe()
        ps.standardOutput = pipe
        do { try ps.run() } catch { return nil }
        ps.waitUntilExit()
        let text = String(decoding: pipe.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
        let fields = text.split(whereSeparator: \.isWhitespace)
        guard fields.count >= 2, let rss = Double(fields[0]) else { return nil }
        // time= is [[dd-]hh:]mm:ss.ss
        let seconds = fields[1].split(separator: ":").reduce(0.0) { $0 * 60 + (Double($1) ?? 0) }
        return (rss / 1024, seconds)
    }

    /// WebKit's private process identifier, read defensively: test-only.
    private func webContentPID(_ view: WKWebView?) -> Int32? {
        guard let view, view.responds(to: NSSelectorFromString("_webProcessIdentifier")),
              let pid = view.value(forKey: "_webProcessIdentifier") as? Int32, pid > 0 else { return nil }
        return pid
    }

    private func sample(_ label: String, _ view: WKWebView?) -> Sample {
        let app = processStats(getpid())
        let web = webContentPID(view).flatMap(processStats)
        let sample = Sample(label: label, appRSSMB: app?.rssMB ?? 0, appCPUSeconds: app?.cpuSeconds ?? 0,
                            webRSSMB: web?.rssMB, webCPUSeconds: web?.cpuSeconds)
        print("[soak] \(sample)")
        return sample
    }

    private func waitLoaded(_ controller: PreviewController) async {
        let deadline = ContinuousClock.now + .seconds(20)
        while controller.state.phase != .loaded, ContinuousClock.now < deadline {
            try? await Task.sleep(for: .milliseconds(20))
        }
    }

    private func idle(_ duration: Duration) async {
        try? await Task.sleep(for: duration)
    }

    func testPreviewResourceWorkload() async throws {
        try XCTSkipUnless(ProcessInfo.processInfo.environment["RUSTCODE_PREVIEW_SOAK"] == "1",
                          "Set RUSTCODE_PREVIEW_SOAK=1 to run the preview soak workload.")
        let phase = Duration.seconds(Int(ProcessInfo.processInfo.environment["RUSTCODE_PREVIEW_SOAK_PHASE"] ?? "30") ?? 30)
        let server = try TestPageServer(html: Self.page)
        try await server.start()
        defer { server.stop() }
        print("[soak] macOS \(ProcessInfo.processInfo.operatingSystemVersionString)")

        let suite = "rustcode-preview-soak"
        let defaults = UserDefaults(suiteName: suite)!
        defaults.removePersistentDomain(forName: suite)
        defer { defaults.removePersistentDomain(forName: suite) }
        let controller = PreviewController(store: PreviewURLStore(defaults: defaults))
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1000, height: 800), styleMask: [.titled],
                              backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.orderFront(nil)
        defer { window.close() }
        func show() {
            controller.webView.frame = window.contentView!.bounds
            window.contentView!.addSubview(controller.webView)
        }

        _ = sample("baseline (no web view)", nil)
        controller.setWorkspace("/soak/project-0")
        show()
        controller.load(server.url)
        await waitLoaded(controller)
        await idle(.seconds(3))
        let firstPID = webContentPID(controller.createdWebView)
        _ = sample("loaded, visible", controller.createdWebView)
        await idle(phase)
        _ = sample("visible +\(phase.components.seconds)s", controller.createdWebView)

        // Hidden: the panel closed, so the view leaves the window.
        controller.createdWebView?.removeFromSuperview()
        await idle(phase)
        _ = sample("hidden +\(phase.components.seconds)s", controller.createdWebView)
        show()
        await idle(.seconds(2))
        _ = sample("shown again", controller.createdWebView)

        for round in 1...100 {
            controller.reload()
            await waitLoaded(controller)
            if round % 25 == 0 { _ = sample("after \(round) reloads", controller.createdWebView) }
        }
        await idle(.seconds(5))
        _ = sample("100 reloads, settled", controller.createdWebView)

        var oldPIDs: Set<Int32> = firstPID.map { [$0] } ?? []
        var oldViews: [() -> WKWebView?] = []
        for index in 1...30 {
            weak var old = controller.createdWebView
            oldViews.append { old }
            controller.setWorkspace("/soak/project-\(index)")
            show()
            controller.load(server.url)
            await waitLoaded(controller)
            if let pid = webContentPID(controller.createdWebView) { oldPIDs.insert(pid) }
            if index % 10 == 0 { _ = sample("after \(index) project switches", controller.createdWebView) }
        }
        let currentPID = webContentPID(controller.createdWebView)
        await idle(.seconds(10))
        _ = sample("30 switches, settled", controller.createdWebView)
        let lingering = oldPIDs.subtracting(currentPID.map { [$0] } ?? []).filter { processStats($0) != nil }
        let liveViews = oldViews.filter { $0() != nil }.count
        print("[soak] distinct WebContent processes used: \(oldPIDs.count); old ones still running: \(lingering.count); old web views alive: \(liveViews)")
        XCTAssertEqual(liveViews, 0, "web views of earlier projects were not released")
        XCTAssertLessThanOrEqual(lingering.count, 1, "WebContent processes of earlier projects kept running")
    }
}
