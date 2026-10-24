import AppKit
import RustCodeKit
import SwiftUI
import WebKit

/// The localhost preview beside the chat: one web view showing the project's
/// running dev server, with its address, history and load state.
///
/// Showing a page grants the agent nothing. The agent reaches this page only
/// through `localhost_preview` (PreviewAgentBridge.swift), and only after the
/// user allows it for one chat in the panel's prompt; `agent` holds those
/// grants, which end on revoke, a project switch or a new engine.
///
/// Lifecycle (#2140): the controller lives as long as the app model and owns
/// at most one web view, made on first use. A project switch tears that web
/// view down (KVO observations invalidated, delegates cleared, removed from
/// its host) so the old page and its WebContent process can go; the next
/// project's page gets a new one. WebKit holds its navigation and UI
/// delegates weakly and the KVO closures capture the controller weakly, and
/// the agent bridge uses `callAsyncJavaScript` results rather than a script
/// message handler, so nothing retains the controller from inside WebKit.
/// Hiding the panel only removes the view from the window: the page keeps
/// running (WebKit throttles timers and stops animation frames for a page out
/// of a window), so a dev server's state and HMR connection survive.
@MainActor
final class PreviewController: NSObject, ObservableObject {
    @Published private(set) var state = PreviewState()
    /// The address field's text.
    @Published var address = ""
    @Published private(set) var addressError: PreviewURLError?
    @Published private(set) var canGoBack = false
    @Published private(set) var canGoForward = false
    @Published private(set) var pageTitle: String?
    /// Changes when the web view is replaced, so its host view is rebuilt.
    @Published private(set) var webViewGeneration = 0
    /// While the address field is being edited, page navigation leaves it alone.
    var isEditingAddress = false

    /// The agent's access: which chats the user allowed, and the prompt
    /// waiting for an answer. Mirrored into the published copies below.
    let agent: PreviewAgentGate
    @Published private(set) var agentAccess = PreviewAgentAccess()
    @Published private(set) var agentApproval: PreviewAgentApproval?
    /// What the agent is doing in the page right now, shown over it.
    @Published var agentActivity: String?
    /// True while an agent action runs: page dialogs are answered without a
    /// modal and links that leave the preview are blocked, not opened.
    var isAgentActing = false
    /// What the page tried during the current agent action (a blocked
    /// external link, a download, a dialog), reported back to the agent.
    var agentNotes: [String] = []
    /// A chat's title, for the access prompt; set by the app model.
    var chatTitle: (String) -> String? = { _ in nil }

    private let store: PreviewURLStore
    private(set) var workspace: String?
    private var isVisible = false
    private(set) var createdWebView: WKWebView?
    private var observations: [NSKeyValueObservation] = []

    init(store: PreviewURLStore = PreviewURLStore(), agent: PreviewAgentGate = PreviewAgentGate()) {
        self.store = store
        self.agent = agent
        super.init()
        agent.onChange = { [weak self] in self?.syncAgentState() }
    }

    private func syncAgentState() {
        if agentAccess != agent.access { agentAccess = agent.access }
        if agentApproval != agent.pending { agentApproval = agent.pending }
    }

    /// The user's answer to the access prompt.
    func answerAgentApproval(allow: Bool) {
        guard let approval = agent.pending else { return }
        agent.answer(approval.id, allow: allow)
    }

    /// The panel's revoke button, and a new engine: the agent loses access.
    func revokeAgentAccess() {
        agent.revokeAll()
    }

    /// The web view, made on first use so a closed preview costs nothing.
    var webView: WKWebView {
        if let createdWebView { return createdWebView }
        let configuration = WKWebViewConfiguration()
        configuration.defaultWebpagePreferences.allowsContentJavaScript = true
        // The app's own persistent store, so a dev app's sign-in survives a
        // restart; it is shared with no browser.
        configuration.websiteDataStore = .default()
        // What the page threw, for the agent: a blank page with no error in
        // the result sent it hunting through builds and tests for ten rounds
        // (session 01a16b37). Collected in the isolated client world, where
        // the page cannot see or reset it.
        configuration.userContentController.addUserScript(WKUserScript(
            source: Self.pageErrorCollector, injectionTime: .atDocumentStart,
            forMainFrameOnly: true, in: .defaultClient))
        let view = WKWebView(frame: .zero, configuration: configuration)
        view.navigationDelegate = self
        view.uiDelegate = self
        view.allowsBackForwardNavigationGestures = true
        // Right-click > Inspect Element opens Web Inspector on the page.
        view.isInspectable = true
        observations = [
            view.observe(\.url) { [weak self] _, _ in Task { @MainActor in self?.syncFromWebView() } },
            view.observe(\.title) { [weak self] _, _ in Task { @MainActor in self?.syncFromWebView() } },
            view.observe(\.canGoBack) { [weak self] _, _ in Task { @MainActor in self?.syncFromWebView() } },
            view.observe(\.canGoForward) { [weak self] _, _ in Task { @MainActor in self?.syncFromWebView() } },
        ]
        createdWebView = view
        return view
    }

    /// Switch to another project's preview: its own history and last URL.
    func setWorkspace(_ path: String) {
        guard path != workspace else { return }
        workspace = path
        agent.setWorkspace(path)
        tearDownWebView()
        state.reset()
        addressError = nil
        canGoBack = false
        canGoForward = false
        pageTitle = nil
        address = store.url(for: path)?.absoluteString ?? ""
        if isVisible { restore() }
    }

    /// The panel opened: reload the project's last URL if nothing is shown.
    func show() {
        isVisible = true
        restore()
    }

    func hide() {
        isVisible = false
    }

    func submitAddress() {
        switch PreviewURL.normalize(address) {
        case .success(let url): load(url)
        case .failure(let error): addressError = error
        }
    }

    func load(_ url: URL) {
        addressError = nil
        address = url.absoluteString
        state.begin(url)
        webView.load(URLRequest(url: url))
        if let workspace { store.remember(url, for: workspace) }
    }

    /// Load the failed or current page again from the server.
    func retry() {
        if let url = state.url { load(url) } else { submitAddress() }
    }

    /// The toolbar's reload button, which stops a load in progress.
    func reloadOrStop() {
        if state.isLoading { stop() } else { reload() }
    }

    func reload() {
        if state.phase == .loaded, createdWebView?.url != nil {
            state.begin(nil)
            webView.reload()
        } else {
            retry()
        }
    }

    func stop() {
        createdWebView?.stopLoading()
        state.cancel()
    }

    func goBack() { createdWebView?.goBack() }

    func goForward() { createdWebView?.goForward() }

    /// The page, a rejected remote address, or what is typed, in the default browser.
    func openInBrowser() {
        let target = addressError?.browserURL ?? state.url ?? (try? PreviewURL.normalize(address).get())
        if let target { NSWorkspace.shared.open(target) }
    }

    func dismissAddressError() {
        addressError = nil
    }

    private func restore() {
        guard state.phase == .idle, let workspace, let url = store.url(for: workspace) else { return }
        load(url)
    }

    private func tearDownWebView() {
        guard let view = createdWebView else { return }
        observations = []
        view.stopLoading()
        view.navigationDelegate = nil
        view.uiDelegate = nil
        view.removeFromSuperview()
        createdWebView = nil
        webViewGeneration += 1
    }

    private func syncFromWebView() {
        guard let view = createdWebView else { return }
        canGoBack = view.canGoBack
        canGoForward = view.canGoForward
        pageTitle = view.title.flatMap { $0.isEmpty ? nil : $0 }
        guard let url = view.url, PreviewURL.isLocal(url), state.phase == .loaded else { return }
        state.navigated(to: url)
        if !isEditingAddress { address = url.absoluteString }
        if let workspace { store.remember(url, for: workspace) }
    }

    private func fail(_ webView: WKWebView, _ error: Error) {
        guard webView === createdWebView else { return }
        let error = error as NSError
        guard let failure = PreviewFailure(domain: error.domain, code: error.code, description: error.localizedDescription) else {
            return
        }
        state.fail(failure, url: error.userInfo[NSURLErrorFailingURLErrorKey] as? URL)
    }

    /// Whether the page may go to `url`; one that leaves the preview opens
    /// in the browser instead, unless the agent caused it: the agent never
    /// opens anything outside the preview.
    private func allows(_ url: URL, isMainFrame: Bool) -> Bool {
        switch PreviewURL.policy(for: url, isMainFrame: isMainFrame) {
        case .allow: return true
        case .openExternally:
            if isAgentActing {
                agentNotes.append("Blocked a navigation outside the preview to \(url.absoluteString.prefix(200)).")
            } else {
                NSWorkspace.shared.open(url)
            }
        case .cancel: break
        }
        return false
    }
}

extension PreviewController: WKNavigationDelegate {
    func webView(_ webView: WKWebView, decidePolicyFor navigationAction: WKNavigationAction) async -> WKNavigationActionPolicy {
        guard let url = navigationAction.request.url else { return .allow }
        let isMainFrame = navigationAction.targetFrame?.isMainFrame ?? true
        if navigationAction.shouldPerformDownload {
            if isAgentActing { agentNotes.append("Blocked a download of \(url.lastPathComponent).") }
            return .cancel
        }
        return allows(url, isMainFrame: isMainFrame) ? .allow : .cancel
    }

    /// The preview shows pages; it never saves files. A response it cannot
    /// display (a download) is cancelled.
    func webView(_ webView: WKWebView, decidePolicyFor navigationResponse: WKNavigationResponse) async -> WKNavigationResponsePolicy {
        guard navigationResponse.canShowMIMEType else {
            if isAgentActing {
                agentNotes.append("Blocked a download of \(navigationResponse.response.url?.lastPathComponent ?? "a file").")
            }
            return .cancel
        }
        return .allow
    }

    func webView(_ webView: WKWebView, didStartProvisionalNavigation navigation: WKNavigation!) {
        guard webView === createdWebView else { return }
        if !state.isLoading { state.begin(nil) }
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        guard webView === createdWebView else { return }
        state.finish(webView.url)
        syncFromWebView()
    }

    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
        fail(webView, error)
    }

    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
        fail(webView, error)
    }

    func webViewWebContentProcessDidTerminate(_ webView: WKWebView) {
        guard webView === createdWebView else { return }
        state.fail(PreviewFailure(kind: .other, message: "The page's web content process stopped."), url: nil)
    }
}

extension PreviewController: WKUIDelegate {
    /// `target="_blank"` links and `window.open`: local pages open in the
    /// preview itself, others in the browser.
    func webView(_ webView: WKWebView, createWebViewWith configuration: WKWebViewConfiguration,
                 for navigationAction: WKNavigationAction, windowFeatures: WKWindowFeatures) -> WKWebView? {
        if let url = navigationAction.request.url,
           allows(url, isMainFrame: true) {
            webView.load(navigationAction.request)
        }
        return nil
    }

    func webView(_ webView: WKWebView, runJavaScriptAlertPanelWithMessage message: String,
                 initiatedByFrame frame: WKFrameInfo) async {
        // A modal would stall the agent's action until the user noticed it.
        if isAgentActing {
            agentNotes.append("The page showed an alert: \(message.prefix(300))")
            return
        }
        let alert = NSAlert()
        alert.messageText = message
        alert.addButton(withTitle: "OK")
        alert.runModal()
    }

    func webView(_ webView: WKWebView, runJavaScriptConfirmPanelWithMessage message: String,
                 initiatedByFrame frame: WKFrameInfo) async -> Bool {
        if isAgentActing {
            agentNotes.append("The page asked to confirm \"\(message.prefix(300))\"; it was answered Cancel.")
            return false
        }
        let alert = NSAlert()
        alert.messageText = message
        alert.addButton(withTitle: "OK")
        alert.addButton(withTitle: "Cancel")
        return alert.runModal() == .alertFirstButtonReturn
    }
}
