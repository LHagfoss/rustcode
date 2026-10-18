import AppKit
import RustCodeKit
import SwiftUI
import WebKit

/// The localhost preview, a panel on the chat's right: an address bar with
/// back, forward and reload, the live page, and its load failures. ⇧⌘P shows
/// or hides it.
struct PreviewPanel: View {
    @ObservedObject var preview: PreviewController
    let close: () -> Void
    @FocusState private var addressFocused: Bool

    var body: some View {
        VStack(spacing: 0) {
            toolbar
            if let error = preview.addressError { addressErrorBar(error) }
            ZStack {
                PreviewWebView(controller: preview)
                    .id(preview.webViewGeneration)
                overlay
            }
        }
        .background(Color.chatBackground)
        .onAppear { preview.show() }
        .onDisappear { preview.hide() }
        .onChange(of: addressFocused) { _, focused in preview.isEditingAddress = focused }
    }

    /// Lines up with the chat's title bar.
    private var toolbar: some View {
        HStack(spacing: 8) {
            ToolbarCluster {
                ClusterButton(systemImage: "chevron.left", help: "Back") { preview.goBack() }
                    .disabled(!preview.canGoBack)
                ClusterButton(systemImage: "chevron.right", help: "Forward") { preview.goForward() }
                    .disabled(!preview.canGoForward)
                ClusterButton(systemImage: preview.state.isLoading ? "xmark" : "arrow.clockwise",
                              help: preview.state.isLoading ? "Stop loading" : "Reload (⌘R)") { preview.reloadOrStop() }
            }
            HStack(spacing: 6) {
                Image(systemName: "network").foregroundStyle(.secondary)
                TextField("http://localhost:3000", text: $preview.address)
                    .textFieldStyle(.plain)
                    .focused($addressFocused)
                    .onSubmit {
                        preview.submitAddress()
                        addressFocused = false
                    }
                    .help(preview.pageTitle ?? "A local URL to preview")
                if preview.state.isLoading {
                    ProgressView().controlSize(.small)
                }
            }
            .font(.callout)
            .padding(.horizontal, 12)
            .frame(height: 28)
            .glassSurface(in: .capsule)
            ToolbarCluster {
                ClusterButton(systemImage: "safari", help: "Open in browser") { preview.openInBrowser() }
                ClusterButton(systemImage: "xmark", help: "Hide preview (⇧⌘P)", action: close)
            }
        }
        .padding(.horizontal, 12)
        .frame(height: 28)
        .padding(.top, 6)
        .padding(.bottom, 8)
    }

    private func addressErrorBar(_ error: PreviewURLError) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(Color.attention)
            Text(error.message).font(.callout).fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 8)
            if error.browserURL != nil {
                Button("Open in Browser") { preview.openInBrowser() }.buttonStyle(.borderless)
            }
            Button("Dismiss") { preview.dismissAddressError() }.buttonStyle(.borderless)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 8)
        .background(.fill.quaternary)
    }

    @ViewBuilder
    private var overlay: some View {
        switch preview.state.phase {
        case .idle:
            message(systemImage: "globe", title: "Preview a local app",
                    detail: "Enter the URL of the project's running dev server above, such as http://localhost:3000.") {
                Text("Only you see this page. The agent cannot view or use it.")
                    .font(.caption)
                    .foregroundStyle(.tertiary)
            }
        case .failed(let failure):
            message(systemImage: failure.kind == .serverUnavailable ? "bolt.horizontal.circle" : "exclamationmark.triangle",
                    title: failure.title(for: preview.state.url), detail: failure.detail) {
                HStack(spacing: 8) {
                    Button("Retry") { preview.retry() }
                        .buttonStyle(PillButtonStyle(prominent: true))
                    Button("Open in Browser") { preview.openInBrowser() }
                        .buttonStyle(PillButtonStyle())
                }
                .padding(.top, 4)
            }
        case .loading, .loaded:
            EmptyView()
        }
    }

    private func message<Actions: View>(systemImage: String, title: String, detail: String,
                                        @ViewBuilder actions: () -> Actions) -> some View {
        VStack(spacing: 10) {
            Image(systemName: systemImage)
                .font(.system(size: 34, weight: .light))
                .foregroundStyle(.secondary)
            Text(title)
                .font(.title3.weight(.semibold))
                .multilineTextAlignment(.center)
            Text(detail)
                .font(.callout)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
                .textSelection(.enabled)
            actions()
        }
        .padding(28)
        .frame(maxWidth: 420)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Color.chatBackground)
    }
}

/// Hosts the controller's web view, which outlives this view so hiding the
/// panel keeps the page and its history.
private struct PreviewWebView: NSViewRepresentable {
    let controller: PreviewController

    func makeNSView(context: Context) -> NSView {
        let container = NSView()
        embed(in: container)
        return container
    }

    func updateNSView(_ container: NSView, context: Context) {
        if controller.webView.superview !== container { embed(in: container) }
    }

    private func embed(in container: NSView) {
        let webView = controller.webView
        webView.removeFromSuperview()
        webView.frame = container.bounds
        webView.autoresizingMask = [.width, .height]
        container.addSubview(webView)
    }
}

/// The line between the chat and the preview, dragged to resize the preview.
struct PreviewResizeHandle: View {
    @Binding var width: Double
    @State private var startWidth: Double?
    static let range: ClosedRange<Double> = 320...1200

    var body: some View {
        Rectangle()
            .fill(Color.primary.opacity(0.1))
            .frame(width: 1)
            .padding(.horizontal, 3)
            .contentShape(.rect)
            .onHover { inside in
                if inside { NSCursor.resizeLeftRight.push() } else { NSCursor.pop() }
            }
            .gesture(
                DragGesture(minimumDistance: 1, coordinateSpace: .global)
                    .onChanged { drag in
                        let start = startWidth ?? width
                        startWidth = start
                        width = min(max(start - drag.translation.width, Self.range.lowerBound), Self.range.upperBound)
                    }
                    .onEnded { _ in startWidth = nil }
            )
            .accessibilityHidden(true)
    }
}
