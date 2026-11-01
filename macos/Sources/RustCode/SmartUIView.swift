import AppKit
import RustCodeKit
import SwiftUI
import WebKit

/// A `rustcode-ui v1` block in an assistant reply (#2484): a card that runs
/// the block's page in the shared Smart UI web view, a placeholder with a Run
/// button when another block holds it, an offer to turn Smart UI on while it
/// is off (#2536), and the source with a reason when the block is malformed or
/// its page was stopped.
struct SmartUIView: View {
    let block: SmartUIBlock
    /// The reply is still arriving, so an open fence may yet close.
    var isStreaming = false
    @ObservedObject private var host = SmartUIHost.shared
    @AppStorage(SmartUI.enabledKey) private var isEnabled = SmartUI.enabledByDefault
    /// The block was written while on screen, so it runs once complete.
    @State private var wasBuilding = false
    @State private var showsSource = false

    var body: some View {
        Group {
            switch block.state {
            case .ready(let title, let summary):
                if let reason = host.failures[block.id] {
                    fallback(reason)
                } else if !isEnabled {
                    offCard(title: title, summary: summary)
                } else {
                    card(title: title, summary: summary)
                }
            case .invalid(let reason):
                fallback(reason)
            case .incomplete:
                if isStreaming {
                    HStack(spacing: 8) {
                        ProgressView().controlSize(.small)
                        Text("Building interactive view…").foregroundStyle(.secondary)
                    }
                    .font(.callout)
                    .padding(.vertical, 6)
                    .accessibilityElement(children: .combine)
                    .onAppear { wasBuilding = true }
                } else {
                    fallback("incomplete block")
                }
            }
        }
        .onChange(of: block.state) { _, state in
            if wasBuilding, isEnabled, case .ready = state { host.activate(block) }
        }
    }

    private func card(title: String, summary: String?) -> some View {
        let isActive = host.activeID == block.id
        return VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 6) {
                Image(systemName: "sparkles.rectangle.stack").foregroundStyle(.secondary)
                Text(title).font(.headline)
                Spacer(minLength: 8)
                Button { showsSource.toggle() } label: {
                    Image(systemName: "chevron.left.forwardslash.chevron.right")
                }
                .buttonStyle(.borderless)
                .help(showsSource ? "Hide the block's source" : "Show the block's source")
                if isActive {
                    Button { host.stop() } label: { Image(systemName: "stop.circle") }
                        .buttonStyle(.borderless)
                        .help("Stop the interactive view")
                }
                CopyButton(help: "Copy the block's source", action: copy)
                    .padding(.trailing, -6)
            }
            if let summary {
                Text(summary).font(.callout).foregroundStyle(.secondary)
            }
            if isActive {
                SmartUIWebContainer(host: host)
                    .frame(height: host.height)
                    .frame(maxWidth: .infinity)
            } else {
                Button { host.activate(block) } label: {
                    Label("Run interactive view", systemImage: "play.fill")
                }
                .buttonStyle(PillButtonStyle())
                .help("Runs in a sandbox with no network, file or app access")
            }
            if showsSource {
                CodeBlock(language: "html", text: block.body)
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.fill.quaternary, in: .rect(cornerRadius: 12))
        .contextMenu {
            Button("Copy Source", systemImage: "doc.on.doc", action: copy)
            Button(showsSource ? "Hide Source" : "Show Source",
                   systemImage: "chevron.left.forwardslash.chevron.right") { showsSource.toggle() }
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel(title)
        .accessibilityHint(summary ?? "Interactive view")
    }

    /// A block written while Smart UI is off, as when the agent learned the
    /// format from the docs: its title and an offer to turn it on, not a page
    /// of HTML. Turning it on only lets blocks run here; the agent is told at
    /// the next engine start.
    private func offCard(title: String, summary: String?) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 6) {
                Image(systemName: "sparkles.rectangle.stack").foregroundStyle(.secondary)
                Text(title).font(.headline)
                Spacer(minLength: 8)
                Button { showsSource.toggle() } label: {
                    Image(systemName: "chevron.left.forwardslash.chevron.right")
                }
                .buttonStyle(.borderless)
                .help(showsSource ? "Hide the block's source" : "Show the block's source")
                CopyButton(help: "Copy the block's source", action: copy)
                    .padding(.trailing, -6)
            }
            if let summary {
                Text(summary).font(.callout).foregroundStyle(.secondary)
            }
            HStack(spacing: 10) {
                Text("Smart UI is off, so this interactive view can't run.")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                Button("Turn On Smart UI") { isEnabled = true }
                    .buttonStyle(PillButtonStyle())
                    .help("Lets interactive views run, sandboxed with no network, file or app access")
            }
            if showsSource {
                CodeBlock(language: "html", text: block.body)
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.fill.quaternary, in: .rect(cornerRadius: 12))
        .accessibilityElement(children: .contain)
        .accessibilityLabel(title)
        .accessibilityHint("Smart UI is off")
    }

    private func fallback(_ reason: String) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Label("Couldn't run this interactive view (\(reason)) — showing its source.",
                  systemImage: "sparkles.rectangle.stack")
                .font(.caption)
                .foregroundStyle(.secondary)
            CodeBlock(language: "html", text: block.body)
        }
    }

    private func copy() {
        Clipboard.copy(block.source)
    }
}

/// Where the live page sits in the active block's card.
private struct SmartUIWebContainer: NSViewRepresentable {
    @ObservedObject var host: SmartUIHost

    func makeNSView(context: Context) -> NSView {
        let container = NSView()
        host.attach(to: container)
        return container
    }

    func updateNSView(_ container: NSView, context: Context) {
        host.attach(to: container)
    }

    static func dismantleNSView(_ container: NSView, coordinator: ()) {
        MainActor.assumeIsolated { SmartUIHost.shared.detach(from: container) }
    }
}

/// Owns the one Smart UI web view. It is created only when a block runs, and
/// each page gets a new one: reloading pages into one web view grows its
/// WebContent process (docs/preview.md), while a new view lets the old
/// process go. A page out of sight for `hiddenGrace` is torn down.
///
/// Isolation: a non-persistent data store shared with nothing, a content
/// policy that allows only inline script and style, a rule list that blocks
/// every network and file load, no navigation after the first load, no
/// popups, panels, downloads or media capture, and no script message handler.
/// The app reads the page's height by evaluating a fixed expression, which is
/// also the watchdog: a page that stops answering is torn down.
@MainActor
final class SmartUIHost: NSObject, ObservableObject {
    static let shared = SmartUIHost()

    /// The block whose page is live.
    @Published private(set) var activeID: String?
    /// The live page's fitted height.
    @Published private(set) var height = SmartUI.minHeight
    /// Blocks whose page was stopped, with the reason; they show source.
    @Published private(set) var failures: [String: String] = [:]

    /// Test hook: the live web view.
    private(set) var webView: SmartUIWebView?
    private var watchdog: Task<Void, Never>?
    private var hiddenTeardown: Task<Void, Never>?
    private var loaded = false
    /// The active block's container. The web view is made after the rule
    /// list compiles, which can be after the container appeared.
    private weak var container: NSView?
    private static var rules: WKContentRuleList?

    static let hiddenGrace = Duration.seconds(30)
    static let loadDeadline = Duration.seconds(5)
    static let answerDeadline = Duration.seconds(3)
    static let pollInterval = Duration.milliseconds(500)

    /// Run `block`'s page, replacing any other.
    func activate(_ block: SmartUIBlock) {
        guard case .ready = block.state, activeID != block.id, failures[block.id] == nil else { return }
        tearDown()
        activeID = block.id
        height = SmartUI.minHeight
        let document = SmartUI.document(for: block.body)
        let id = block.id
        Task {
            let rules = await Self.compiledRules()
            guard activeID == id, webView == nil else { return }
            let view = makeWebView(rules: rules)
            webView = view
            loaded = false
            view.loadHTMLString(document, baseURL: nil)
            if let container { attach(to: container) }
            startWatchdog(view, id: id)
        }
    }

    /// Stop the live page; its block shows the Run button again.
    func stop() { tearDown() }

    func attach(to container: NSView) {
        self.container = container
        hiddenTeardown?.cancel()
        hiddenTeardown = nil
        guard let webView, webView.superview !== container else { return }
        webView.removeFromSuperview()
        webView.frame = container.bounds
        webView.autoresizingMask = [.width, .height]
        container.addSubview(webView)
    }

    func detach(from container: NSView) {
        if self.container === container { self.container = nil }
        guard let webView, webView.superview === container else { return }
        webView.removeFromSuperview()
        hiddenTeardown?.cancel()
        hiddenTeardown = Task { [weak self] in
            try? await Task.sleep(for: Self.hiddenGrace)
            guard !Task.isCancelled, let self, self.webView?.superview == nil else { return }
            self.tearDown()
        }
    }

    private func fail(_ reason: String) {
        if let activeID { failures[activeID] = reason }
        tearDown()
    }

    private func tearDown() {
        watchdog?.cancel()
        watchdog = nil
        hiddenTeardown?.cancel()
        hiddenTeardown = nil
        if let webView {
            webView.stopLoading()
            webView.navigationDelegate = nil
            webView.uiDelegate = nil
            webView.removeFromSuperview()
        }
        webView = nil
        container = nil
        activeID = nil
    }

    private func makeWebView(rules: WKContentRuleList?) -> SmartUIWebView {
        let configuration = WKWebViewConfiguration()
        configuration.websiteDataStore = .nonPersistent()
        configuration.defaultWebpagePreferences.allowsContentJavaScript = true
        configuration.preferences.javaScriptCanOpenWindowsAutomatically = false
        configuration.preferences.isElementFullscreenEnabled = false
        configuration.preferences.isFraudulentWebsiteWarningEnabled = false
        configuration.mediaTypesRequiringUserActionForPlayback = .all
        configuration.allowsAirPlayForMediaPlayback = false
        if let rules { configuration.userContentController.add(rules) }
        let view = SmartUIWebView(frame: .zero, configuration: configuration)
        view.navigationDelegate = self
        view.uiDelegate = self
        view.allowsBackForwardNavigationGestures = false
        view.allowsMagnification = false
        view.setValue(false, forKey: "drawsBackground")
        view.setAccessibilityLabel("Interactive view")
        return view
    }

    /// Blocks every load with a scheme that reaches outside the page. The
    /// content policy already forbids them; this holds if a page's markup
    /// gets around it.
    private static func compiledRules() async -> WKContentRuleList? {
        if let rules { return rules }
        let schemes = ["https?", "wss?", "file", "ftp", "blob"]
        let json = "[" + schemes.map {
            #"{"trigger":{"url-filter":"^\#($0):"},"action":{"type":"block"}}"#
        }.joined(separator: ",") + "]"
        let compiled = try? await WKContentRuleListStore.default()
            .compileContentRuleList(forIdentifier: "rustcode-smart-ui", encodedContentRuleList: json)
        rules = compiled
        return compiled
    }

    /// The document's own height (its scroll height never drops below the
    /// view's) and its element count.
    private static let measure = "[Math.ceil(document.documentElement.getBoundingClientRect().height), document.getElementsByTagName('*').length]"

    private func startWatchdog(_ view: SmartUIWebView, id: String) {
        watchdog?.cancel()
        watchdog = Task { [weak self] in
            let started = ContinuousClock.now
            while !Task.isCancelled {
                try? await Task.sleep(for: Self.pollInterval)
                guard let self, !Task.isCancelled, self.webView === view, self.activeID == id else { return }
                if !self.loaded {
                    if ContinuousClock.now - started > Self.loadDeadline { self.fail("the page did not finish loading") }
                    continue
                }
                // Out of a window WebKit suspends the page; nothing to measure.
                guard view.window != nil else { continue }
                guard let (scrollHeight, elements) = await Self.evaluate(Self.measure, in: view) else {
                    if self.webView === view { self.fail("the page stopped responding") }
                    return
                }
                guard self.webView === view else { return }
                if elements > SmartUI.maxElements {
                    self.fail("the page built more than \(SmartUI.maxElements) elements")
                    return
                }
                let fitted = min(max(Double(scrollHeight), SmartUI.minHeight), SmartUI.maxHeight)
                view.contentFits = Double(scrollHeight) <= SmartUI.maxHeight
                if abs(fitted - self.height) >= 1 { self.height = fitted }
            }
        }
    }

    /// The page's answer to `script`, or nil when it errors or takes longer
    /// than `answerDeadline` (a busy loop).
    private static func evaluate(_ script: String, in view: WKWebView) async -> (Int, Int)? {
        await withCheckedContinuation { (continuation: CheckedContinuation<(Int, Int)?, Never>) in
            var finished = false
            let finish: ((Int, Int)?) -> Void = { value in
                guard !finished else { return }
                finished = true
                continuation.resume(returning: value)
            }
            view.evaluateJavaScript(script) { result, _ in
                MainActor.assumeIsolated {
                    let pair = (result as? [NSNumber]).flatMap { $0.count == 2 ? ($0[0].intValue, $0[1].intValue) : nil }
                    finish(pair)
                }
            }
            Task { @MainActor in
                try? await Task.sleep(for: answerDeadline)
                finish(nil)
            }
        }
    }
}

extension SmartUIHost: WKNavigationDelegate {
    /// Only the initial `about:blank` document and in-page anchors; no link,
    /// form, redirect, frame or download goes anywhere.
    func webView(_ webView: WKWebView, decidePolicyFor navigationAction: WKNavigationAction) async -> WKNavigationActionPolicy {
        guard webView === self.webView,
              navigationAction.targetFrame?.isMainFrame == true,
              !navigationAction.shouldPerformDownload,
              navigationAction.request.url?.scheme == "about" else { return .cancel }
        return .allow
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        guard webView === self.webView else { return }
        loaded = true
    }

    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
        guard webView === self.webView else { return }
        fail("the page failed to load")
    }

    func webViewWebContentProcessDidTerminate(_ webView: WKWebView) {
        guard webView === self.webView else { return }
        fail("the page used too much memory or crashed")
    }
}

extension SmartUIHost: WKUIDelegate {
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

/// The Smart UI web view. While its page fits, wheel events go to the
/// transcript, so scrolling over a card scrolls the chat.
final class SmartUIWebView: WKWebView {
    var contentFits = true

    override func scrollWheel(with event: NSEvent) {
        if contentFits {
            nextResponder?.scrollWheel(with: event)
        } else {
            super.scrollWheel(with: event)
        }
    }
}
