import AppKit
import RustCodeKit
import WebKit

/// Renders a Smart UI block offscreen for the agent's `smart_ui_preview`
/// tool, so it sees its page before the user does (T3-style self-check).
///
/// The page runs in the same sandbox as the transcript's live view
/// (`SmartUIHost.sandboxConfiguration`) at the card's width and height
/// limits, in a window placed off every screen. One render runs at a time;
/// each gets a new web view that is torn down as soon as it answers. The only
/// addition is a document-start script that records what the page throws, so
/// the agent can read its errors.
@MainActor
final class SmartUIPreviewRenderer: NSObject {
    static let shared = SmartUIPreviewRenderer()

    static let loadDeadline = Duration.seconds(5)
    static let answerDeadline = Duration.seconds(3)
    /// Time for scripts that build the page after load to run.
    static let settle = Duration.milliseconds(400)
    /// The page's base URL. With no base URL the page's origin is opaque and
    /// WebKit reports every error as "Script error.", which tells the agent
    /// nothing. This host never resolves (`.invalid`), and the content rule
    /// list and policy still block every load.
    static let origin = URL(string: "https://smart-ui-preview.invalid/")!

    private var queue: Task<Void, Never>?
    private var webView: WKWebView?
    private var window: NSWindow?
    private var loaded = false
    private var loadFailed = false

    /// Answer one `_rustcode/smartUi/preview` request. Never throws.
    func render(_ params: JSONValue) async -> JSONValue {
        let request: SmartUIPreviewRequest
        do { request = try SmartUIPreviewRequest(params: params) } catch SmartUIPreviewRequest.Failure.invalid(let reason) {
            return SmartUIPreviewRequest.failure(reason)
        } catch {
            return SmartUIPreviewRequest.failure("invalid request")
        }
        guard SmartUI.isEnabled else { return SmartUIPreviewRequest.failure("Smart UI is turned off in Settings") }
        // Serialize renders: wait for the previous one, then run.
        let previous = queue
        let task = Task { @MainActor [weak self] () -> JSONValue in
            await previous?.value
            guard let self else { return SmartUIPreviewRequest.failure("the app is closing") }
            return await self.run(request)
        }
        queue = Task { _ = await task.value }
        return await task.value
    }

    private func run(_ request: SmartUIPreviewRequest) async -> JSONValue {
        defer { tearDown() }
        let rules = await SmartUIHost.compiledRules()
        let configuration = SmartUIHost.sandboxConfiguration(rules: rules)
        configuration.userContentController.addUserScript(WKUserScript(
            source: SmartUIPreviewRequest.errorCaptureScript, injectionTime: .atDocumentStart, forMainFrameOnly: true))
        let width = SmartUI.previewWidth
        let frame = NSRect(x: 0, y: 0, width: width, height: SmartUI.maxHeight)
        let view = WKWebView(frame: frame, configuration: configuration)
        view.navigationDelegate = self
        view.uiDelegate = self
        view.setValue(false, forKey: "drawsBackground")
        // WebKit pauses pages outside a visible window, so host it in one
        // placed beyond every screen. It never takes focus or clicks.
        let offscreen = NSScreen.screens.map(\.frame).reduce(NSRect.zero) { $0.union($1) }
        let window = NSWindow(contentRect: NSRect(x: offscreen.maxX + 200, y: offscreen.maxY + 200,
                                                  width: width, height: SmartUI.maxHeight),
                              styleMask: [.borderless], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.ignoresMouseEvents = true
        window.hasShadow = false
        window.backgroundColor = .windowBackgroundColor
        window.appearance = NSApp.effectiveAppearance
        window.contentView = view
        window.orderFrontRegardless()
        self.window = window
        webView = view
        loaded = false
        loadFailed = false
        view.loadHTMLString(SmartUI.document(for: request.html), baseURL: Self.origin)

        let clock = ContinuousClock()
        let started = clock.now
        while !loaded, !loadFailed {
            if clock.now - started > Self.loadDeadline {
                return SmartUIPreviewRequest.failure("the page did not finish loading in 5 s")
            }
            try? await Task.sleep(for: .milliseconds(20))
        }
        if loadFailed { return SmartUIPreviewRequest.failure("the page failed to load") }
        try? await Task.sleep(for: Self.settle)

        guard let measured = await Self.evaluate(
            "JSON.stringify([\(SmartUIHost.measure), window.__rustcodeErrors || []])", in: view),
            let data = measured.data(using: .utf8),
            let decoded = try? JSONSerialization.jsonObject(with: data) as? [Any],
            decoded.count == 2,
            let pair = decoded[0] as? [NSNumber], pair.count == 2 else {
            return SmartUIPreviewRequest.failure("the page stopped responding (a busy loop?)")
        }
        let errors = (decoded[1] as? [String]) ?? []
        let contentHeight = pair[0].intValue
        let elements = pair[1].intValue
        if elements > SmartUI.maxElements {
            return SmartUIPreviewRequest.failure("the page built more than \(SmartUI.maxElements) elements", errors: errors)
        }
        let fitted = min(max(Double(contentHeight), SmartUI.minHeight), SmartUI.maxHeight)
        view.frame = NSRect(x: 0, y: 0, width: width, height: fitted)
        window.setContentSize(NSSize(width: width, height: fitted))
        try? await Task.sleep(for: .milliseconds(100))

        let image: NSImage
        do {
            image = try await view.takeSnapshot(configuration: WKSnapshotConfiguration())
        } catch {
            return SmartUIPreviewRequest.failure("the page could not be captured: \(error.localizedDescription)", errors: errors)
        }
        let encoded: (Data, String, Int, Int)
        do { encoded = try PreviewController.encodeForAgent(image) } catch {
            return SmartUIPreviewRequest.failure("the screenshot could not be encoded", errors: errors)
        }
        let (bytes, mime, pixelWidth, pixelHeight) = encoded
        return .object([
            "ok": true,
            "title": .string(request.title),
            "width": .number(width),
            "contentHeight": .number(Double(contentHeight)),
            "maxHeight": .number(SmartUI.maxHeight),
            "elements": .number(Double(elements)),
            "errors": .array(errors.map(JSONValue.string)),
            "image": ["mimeType": .string(mime), "data": .string(bytes.base64EncodedString()),
                      "width": .number(Double(pixelWidth)), "height": .number(Double(pixelHeight))],
        ])
    }

    private func tearDown() {
        if let webView {
            webView.stopLoading()
            webView.navigationDelegate = nil
            webView.uiDelegate = nil
        }
        webView = nil
        window?.orderOut(nil)
        window?.contentView = nil
        window?.close()
        window = nil
    }

    /// `script`'s string answer, or nil when it fails or exceeds
    /// `answerDeadline`.
    private static func evaluate(_ script: String, in view: WKWebView) async -> String? {
        await withCheckedContinuation { (continuation: CheckedContinuation<String?, Never>) in
            var finished = false
            let finish: (String?) -> Void = { value in
                guard !finished else { return }
                finished = true
                continuation.resume(returning: value)
            }
            view.evaluateJavaScript(script) { result, _ in
                MainActor.assumeIsolated { finish(result as? String) }
            }
            Task { @MainActor in
                try? await Task.sleep(for: answerDeadline)
                finish(nil)
            }
        }
    }
}

extension SmartUIPreviewRenderer: WKNavigationDelegate {
    /// Only the initial `about:blank` document, as in the live view.
    func webView(_ webView: WKWebView, decidePolicyFor navigationAction: WKNavigationAction) async -> WKNavigationActionPolicy {
        guard webView === self.webView,
              navigationAction.targetFrame?.isMainFrame == true,
              !navigationAction.shouldPerformDownload,
              let url = navigationAction.request.url,
              url.scheme == "about" || url == Self.origin else { return .cancel }
        return .allow
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        guard webView === self.webView else { return }
        loaded = true
    }

    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
        guard webView === self.webView else { return }
        loadFailed = true
    }

    func webViewWebContentProcessDidTerminate(_ webView: WKWebView) {
        guard webView === self.webView else { return }
        loadFailed = true
    }
}

extension SmartUIPreviewRenderer: WKUIDelegate {
    func webView(_ webView: WKWebView, createWebViewWith configuration: WKWebViewConfiguration,
                 for navigationAction: WKNavigationAction, windowFeatures: WKWindowFeatures) -> WKWebView? {
        nil
    }

    func webView(_ webView: WKWebView, runJavaScriptAlertPanelWithMessage message: String,
                 initiatedByFrame frame: WKFrameInfo) async {}

    func webView(_ webView: WKWebView, runJavaScriptConfirmPanelWithMessage message: String,
                 initiatedByFrame frame: WKFrameInfo) async -> Bool { false }

    func webView(_ webView: WKWebView, runJavaScriptTextInputPanelWithPrompt prompt: String,
                 defaultText: String?, initiatedByFrame frame: WKFrameInfo) async -> String? { nil }

    func webView(_ webView: WKWebView, runOpenPanelWith parameters: WKOpenPanelParameters,
                 initiatedByFrame frame: WKFrameInfo) async -> [URL]? { nil }

    func webView(_ webView: WKWebView, requestMediaCapturePermissionFor origin: WKSecurityOrigin,
                 initiatedByFrame frame: WKFrameInfo, type: WKMediaCaptureType) async -> WKPermissionDecision { .deny }
}
