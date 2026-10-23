import AppKit
import RustCodeKit
import SwiftUI

/// An image shown in the in-app viewer: one in the chat, from a file or the
/// web, or one waiting in the composer, which has no file yet.
enum ViewedImage: Identifiable, Equatable {
    case file(URL)
    case web(URL)
    case attachment(ImageAttachment)

    var id: String {
        switch self {
        case .file(let url), .web(let url): url.absoluteString
        case .attachment(let image): image.id.uuidString
        }
    }

    /// What the viewer's bar calls it: the file's name, never its folder.
    var name: String {
        switch self {
        case .file(let url): url.lastPathComponent
        case .web(let url): url.lastPathComponent.isEmpty ? (url.host ?? "Image") : url.lastPathComponent
        case .attachment: "Pasted image"
        }
    }

    func load() async -> NSImage? {
        switch self {
        case .file(let url):
            return await Task.detached(priority: .userInitiated) { NSImage(contentsOf: url) }.value
        case .web(let url):
            guard let (data, _) = try? await URLSession.shared.data(from: url) else { return nil }
            return NSImage(data: data)
        case .attachment(let image):
            return NSImage(data: image.data)
        }
    }
}

private struct OpenImageKey: EnvironmentKey {
    static let defaultValue: @MainActor (ViewedImage) -> Void = { _ in }
}

extension EnvironmentValues {
    /// Shows an image in the window's viewer.
    var openImage: @MainActor (ViewedImage) -> Void {
        get { self[OpenImageKey.self] }
        set { self[OpenImageKey.self] = newValue }
    }
}

/// The image viewer over the window: the image fitted to it, a click on it
/// toggling between that and a zoomed size (`ImageZoom.zoomed`), a pinch or
/// ⌘+, ⌘− and ⌘0 for any other scale, and scrolled when larger than the
/// window. Esc, the close button or a click beside the image closes it.
struct ImageViewer: View {
    let image: ViewedImage
    let close: () -> Void
    @State private var loaded: NSImage?
    @State private var failed = false
    @State private var zoom = ImageZoom()
    @State private var pinchStart: Double?
    @State private var bounds = CGSize(width: 1, height: 1)
    @State private var window = CGSize(width: 1, height: 1)

    var body: some View {
        ZStack {
            Color.black.opacity(0.85)
                .ignoresSafeArea()
                .contentShape(.rect)
                .onTapGesture(perform: close)
            GeometryReader { geometry in
                content(in: geometry.size)
                    .onAppear { resize(geometry.size) }
                    .onChange(of: geometry.size) { _, size in resize(size) }
            }
            .padding(.top, 52)
        }
        .overlay(alignment: .top) { bar }
        .task(id: image) {
            loaded = await image.load()
            failed = loaded == nil
        }
        .accessibilityAddTraits(.isModal)
    }

    private func resize(_ size: CGSize) {
        bounds = viewport(size)
        window = size
    }

    /// Where the fitted image goes: the window less a margin.
    private func viewport(_ size: CGSize) -> CGSize {
        CGSize(width: max(size.width - 48, 1), height: max(size.height - 32, 1))
    }

    @ViewBuilder
    private func content(in size: CGSize) -> some View {
        if let loaded {
            let scale = zoom.current(fitting: loaded.size, in: bounds)
            ScrollView([.horizontal, .vertical]) {
                ZStack {
                    // A click beside the image closes the viewer.
                    Color.clear.contentShape(.rect).onTapGesture(perform: close)
                    Image(nsImage: loaded)
                        .resizable()
                        .interpolation(scale > 2 ? .none : .high)
                        .frame(width: loaded.size.width * scale, height: loaded.size.height * scale)
                        .shadow(color: .black.opacity(0.4), radius: 20, y: 8)
                        .contentShape(.rect)
                        .onTapGesture { animate { zoom.toggleZoom(fitting: loaded.size, in: bounds, window: window) } }
                        .help(zoom.isFitted ? "Click to zoom" : "Click to fit")
                        .accessibilityLabel(image.name)
                        .accessibilityValue(zoom.isFitted ? "Fitted" : "Zoomed")
                        .accessibilityAddTraits(.isButton)
                        .accessibilityHint(zoom.isFitted ? "Zooms in" : "Fits to the window")
                }
                .frame(minWidth: size.width, minHeight: size.height)
            }
            .scrollIndicators(zoom.isFitted ? .never : .automatic)
            .simultaneousGesture(
                MagnifyGesture()
                    .onChanged { value in
                        let start = pinchStart ?? scale
                        pinchStart = start
                        zoom.magnify(from: start, by: value.magnification)
                    }
                    .onEnded { _ in pinchStart = nil }
            )
        } else if failed {
            Label("The image could not be opened", systemImage: "photo")
                .foregroundStyle(.white.opacity(0.8))
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            ProgressView().controlSize(.large).tint(.white)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    private var bar: some View {
        HStack(spacing: 4) {
            Text(image.name)
                .font(.headline)
                .lineLimit(1)
                .truncationMode(.middle)
                .padding(.leading, 6)
            Spacer(minLength: 12)
            if let loaded {
                // The zoom cue: a quiet percentage while fitted, a lit one
                // while zoomed.
                Text("\(Int((zoom.current(fitting: loaded.size, in: bounds) * 100).rounded()))%")
                    .font(.callout.monospacedDigit())
                    .foregroundStyle(.white.opacity(zoom.isFitted ? 0.6 : 0.95))
                    .padding(.horizontal, 8)
                    .frame(minWidth: 52, minHeight: 24)
                    .background(.white.opacity(zoom.isFitted ? 0 : 0.14), in: .capsule)
                    .accessibilityLabel(zoom.isFitted ? "Fitted" : "Zoomed")
                    .accessibilityValue("\(Int((zoom.current(fitting: loaded.size, in: bounds) * 100).rounded())) percent")
                    .background { zoomKeys(loaded.size) }
                barButton("doc.on.doc", help: "Copy Image") {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.writeObjects([loaded])
                }
            }
            barButton("xmark", help: "Close (Esc)", action: close)
                .keyboardShortcut(.cancelAction)
                .accessibilityLabel("Close")
        }
        .foregroundStyle(.white)
        .padding(.leading, 12)
        .padding(.trailing, 6)
        .frame(height: 40)
        .background(.black.opacity(0.35), in: .capsule)
        .padding(.horizontal, 80)
        .padding(.top, 8)
    }

    /// ⌘+, ⌘− and ⌘0, kept without buttons of their own: a click on the
    /// image does what the buttons did.
    private func zoomKeys(_ size: CGSize) -> some View {
        ZStack {
            Button("Zoom In") { animate { zoom.zoomIn(fitting: size, in: bounds) } }
                .keyboardShortcut("=", modifiers: .command)
            Button("Zoom Out") { animate { zoom.zoomOut(fitting: size, in: bounds) } }
                .keyboardShortcut("-", modifiers: .command)
            Button("Fit to Window") { animate { zoom.fit() } }
                .keyboardShortcut("0", modifiers: .command)
        }
        .buttonStyle(.plain)
        .opacity(0)
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }

    private func animate(_ change: () -> Void) {
        withAnimation(.snappy(duration: 0.2), change)
    }

    private func barButton(_ systemImage: String, help: String, action: @escaping () -> Void) -> some View {
        Button(action: action) { Image(systemName: systemImage) }
            .buttonStyle(RoundOverlayButtonStyle())
            .help(help)
    }
}

/// A round icon button on the viewer's dark bar: a faint disc on hover that
/// deepens and shrinks a little when pressed.
private struct RoundOverlayButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        RoundOverlayButton(configuration: configuration)
    }
}

private struct RoundOverlayButton: View {
    let configuration: ButtonStyleConfiguration
    @State private var hovered = false

    var body: some View {
        configuration.label
            .font(.system(size: 13, weight: .semibold))
            .frame(width: 30, height: 30)
            .background(.white.opacity(configuration.isPressed ? 0.28 : hovered ? 0.16 : 0), in: .circle)
            .contentShape(.circle)
            .scaleEffect(configuration.isPressed ? 0.92 : 1)
            .animation(.snappy(duration: 0.15), value: hovered)
            .animation(.snappy(duration: 0.1), value: configuration.isPressed)
            .onHover { hovered = $0 }
    }
}
