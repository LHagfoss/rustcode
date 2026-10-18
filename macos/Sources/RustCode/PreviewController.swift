import AppKit
import RustCodeKit
import SwiftUI
import WebKit

/// The localhost preview beside the chat: one web view showing the project's
/// running dev server, with its address, history and load state.
///
/// The preview is view-only. Nothing here is reachable from the engine or the
/// agent: the agent cannot see, read, screenshot or drive this page, and the UI
/// says so. Agent access (navigate, click, fill, snapshot) should be added as
/// a separate bridge type that holds this controller and checks an explicit,
/// per-project user grant before every action, surfaced through the same
/// permission cards as tool approvals, so that showing a page never implies
/// the agent can use it.
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

    private let store: PreviewURLStore
    private var workspace: String?
    private var isVisible = false
    private var createdWebView: WKWebView?
    private var observations: [NSKeyValueObservation] = []

    init(store: PreviewURLStore = PreviewURLStore()) {
        self.store = store
        super.init()
    }

    /// The web view, made on first use so a closed preview costs nothing.
    var webView: WKWebView {
        if let createdWebView { return createdWebView }
        let configuration = WKWebViewConfiguration()
        configuration.defaultWebpagePreferences.allowsContentJavaScript = true
        // The app's own persistent store, so a dev app's sign-in survives a
        // restart; it is shared with no browser.
        configuration.websiteDataStore = .default()
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
    /// in the browser instead.
    private func allows(_ url: URL, isMainFrame: Bool) -> Bool {
        switch PreviewURL.policy(for: url, isMainFrame: isMainFrame) {
        case .allow: return true
        case .openExternally: NSWorkspace.shared.open(url)
        case .cancel: break
        }
        return false
    }
}

extension PreviewController: WKNavigationDelegate {
    func webView(_ webView: WKWebView, decidePolicyFor navigationAction: WKNavigationAction) async -> WKNavigationActionPolicy {
        guard let url = navigationAction.request.url else { return .allow }
        let isMainFrame = navigationAction.targetFrame?.isMainFrame ?? true
        return allows(url, isMainFrame: isMainFrame) ? .allow : .cancel
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
        let alert = NSAlert()
        alert.messageText = message
        alert.addButton(withTitle: "OK")
        alert.runModal()
    }

    func webView(_ webView: WKWebView, runJavaScriptConfirmPanelWithMessage message: String,
                 initiatedByFrame frame: WKFrameInfo) async -> Bool {
        let alert = NSAlert()
        alert.messageText = message
        alert.addButton(withTitle: "OK")
        alert.addButton(withTitle: "Cancel")
        return alert.runModal() == .alertFirstButtonReturn
    }
}
