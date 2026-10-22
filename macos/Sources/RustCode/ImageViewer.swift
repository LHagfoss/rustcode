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

/// The image viewer over the window: the image fitted to it, zoomed with a
/// pinch, ⌘+ and ⌘− or a double-click, and scrolled when larger than the
/// window. Esc, the close button or a click beside the image closes it.
struct ImageViewer: View {
    let image: ViewedImage
    let close: () -> Void
    @State private var loaded: NSImage?
    @State private var failed = false
    @State private var zoom = ImageZoom()
    @State private var pinchStart: Double?
    @State private var bounds = CGSize(width: 1, height: 1)

    var body: some View {
        ZStack {
            Color.black.opacity(0.85)
                .ignoresSafeArea()
                .contentShape(.rect)
                .onTapGesture(perform: close)
            GeometryReader { geometry in
                content(in: geometry.size)
                    .onAppear { bounds = viewport(geometry.size) }
                    .onChange(of: geometry.size) { _, size in bounds = viewport(size) }
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
                        .onTapGesture(count: 2) { withAnimation(.snappy(duration: 0.2)) { zoom.toggleActualSize() } }
                        .accessibilityLabel(image.name)
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
                Text("\(Int((zoom.current(fitting: loaded.size, in: bounds) * 100).rounded()))%")
                    .font(.callout.monospacedDigit())
                    .foregroundStyle(.white.opacity(0.7))
                    .frame(minWidth: 48)
                barButton("minus.magnifyingglass", help: "Zoom Out (⌘−)", key: "-") { zoom.zoomOut(fitting: loaded.size, in: bounds) }
                barButton("arrow.up.left.and.down.right.magnifyingglass", help: "Fit to Window (⌘0)", key: "0") { zoom.fit() }
                barButton("plus.magnifyingglass", help: "Zoom In (⌘+)", key: "=") { zoom.zoomIn(fitting: loaded.size, in: bounds) }
                barButton("doc.on.doc", help: "Copy Image") {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.writeObjects([loaded])
                }
            }
            Button(action: close) { Image(systemName: "xmark").frame(width: 28, height: 28).contentShape(.rect) }
                .buttonStyle(.plain)
                .keyboardShortcut(.cancelAction)
                .help("Close (Esc)")
        }
        .foregroundStyle(.white)
        .padding(.horizontal, 12)
        .frame(height: 40)
        .background(.black.opacity(0.35), in: .capsule)
        .padding(.horizontal, 80)
        .padding(.top, 8)
    }

    @ViewBuilder
    private func barButton(_ systemImage: String, help: String, key: KeyEquivalent? = nil, action: @escaping () -> Void) -> some View {
        let button = Button { withAnimation(.snappy(duration: 0.2)) { action() } } label: {
            Image(systemName: systemImage).frame(width: 28, height: 28).contentShape(.rect)
        }
        .buttonStyle(.plain)
        .help(help)
        if let key { button.keyboardShortcut(key, modifiers: .command) } else { button }
    }
}
