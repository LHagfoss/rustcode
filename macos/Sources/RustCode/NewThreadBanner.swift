import AppKit
import RustCodeKit
import SwiftUI
import UniformTypeIdentifiers

/// The picture over a new thread, chosen in Settings ▸ Appearance. A copy is
/// kept in Application Support, so moving or deleting the original keeps the
/// banner; the setting holds the copy's file name, or "" for none.
enum NewThreadBanner {
    static let key = "newThreadBanner"
    /// `ascii` (the picture drawn in text) or `picture`.
    static let styleKey = "newThreadBannerStyle"

    static var directory: URL {
        FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("RustCode", isDirectory: true)
    }

    static func url(_ name: String) -> URL? {
        name.isEmpty ? nil : directory.appendingPathComponent(name)
    }

    /// Ask for a picture and keep a copy. The new copy's name, or nil when
    /// none was chosen or it could not be read.
    @MainActor
    static func choose(replacing old: String) -> String? {
        let panel = NSOpenPanel()
        panel.allowedContentTypes = [.image]
        panel.message = "Choose a wide picture for the top of new threads. It is cropped to fit."
        panel.prompt = "Use Picture"
        guard panel.runModal() == .OK, let source = panel.url, NSImage(contentsOf: source) != nil else { return nil }
        // A new name each time, so a view showing the old one reloads.
        let name = "new-thread-banner-\(UUID().uuidString.prefix(8)).\(source.pathExtension.isEmpty ? "png" : source.pathExtension)"
        do {
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
            try FileManager.default.copyItem(at: source, to: directory.appendingPathComponent(name))
        } catch {
            return nil
        }
        remove(old)
        return name
    }

    static func remove(_ name: String) {
        if let url = url(name) { try? FileManager.default.removeItem(at: url) }
    }
}

/// The banner as the new thread shows it: the picture in colored text, or
/// the picture itself, across the room it has. Given a `height`, it fills
/// that much of the top of the chat, cropped to fit, and fades to the chat
/// toward its foot; without one (Settings' preview) it is a band at least
/// 3:1. Either way it fades in from the left as it appears.
struct NewThreadBannerView: View {
    let name: String
    let style: String
    /// The height to fill, or nil for a band as tall as the picture allows.
    var height: CGFloat?
    @Environment(\.colorScheme) private var colorScheme
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var width: CGFloat = 0
    @State private var drawnHeight: CGFloat = 0
    @State private var image: CGImage?
    @State private var art: AttributedString?
    @State private var revealed = false

    /// The text's size: small enough that a band reads as a picture.
    static let fontSize: CGFloat = 7
    static let font = NSFont.monospacedSystemFont(ofSize: fontSize, weight: .regular)
    static let charWidth = ("M" as NSString).size(withAttributes: [.font: font]).width
    static let lineHeight = NSLayoutManager().defaultLineHeight(for: font)
    /// Rows a band takes at most; a wider room crops the picture to a
    /// wider band instead of growing taller.
    static let maxRows = 22
    /// A band picture's height at most.
    static let maxPictureHeight: CGFloat = 180

    private var columns: Int {
        guard image != nil else { return 0 }
        return max(20, min(Int(width / Self.charWidth), 480))
    }

    private var rows: Int {
        guard let height else { return Self.maxRows }
        return max(4, Int(height / Self.lineHeight))
    }

    /// The crop's width over its height: the room's, in cells.
    private var aspect: Double {
        Double(columns) / (Double(rows) * AsciiBanner.cellAspect)
    }

    var body: some View {
        // The room sets the width; the banner, drawn over it, sets only the
        // height, so text a little wider than the room cannot widen the chat.
        Color.clear
            .frame(maxWidth: .infinity)
            .frame(height: height ?? drawnHeight)
            .onGeometryChange(for: CGFloat.self) { $0.size.width } action: { width = $0 }
            .overlay(alignment: .top) {
                ZStack {
                    // A soft glow of itself under the banner.
                    banner
                        .blur(radius: 14)
                        .opacity(colorScheme == .dark ? 0.5 : 0.3)
                    banner
                }
                .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { drawnHeight = $0 }
                .frame(width: width, alignment: .top)
                .mask { height == nil ? AnyView(Self.sideFade) : AnyView(Self.footFade) }
                .opacity(revealed ? 1 : 0)
                .mask(alignment: .leading) {
                    GeometryReader { proxy in
                        Rectangle().frame(width: revealed ? proxy.size.width : 0)
                    }
                }
            }
            .clipped()
            .allowsHitTesting(false)
            .accessibilityHidden(true)
        .task(id: name) {
            image = await Task.detached(priority: .userInitiated) {
                NewThreadBanner.url(name).flatMap { url in
                    CGImageSourceCreateWithURL(url as CFURL, nil).flatMap { CGImageSourceCreateImageAtIndex($0, 0, nil) }
                }
            }.value
        }
        .task(id: RenderKey(name: name, columns: columns, rows: rows, dark: colorScheme == .dark, loaded: image != nil)) {
            guard let image, columns > 0, style != "picture" else { return }
            // Settle a resize before drawing again.
            try? await Task.sleep(for: .milliseconds(art == nil ? 0 : 120))
            let columns = columns, inverted = colorScheme != .dark, aspect = aspect
            let rendered = await Task.detached(priority: .userInitiated) {
                Self.attributed(AsciiBanner.render(image, columns: columns, inverted: inverted, aspect: aspect))
            }.value
            guard !Task.isCancelled else { return }
            art = rendered
        }
        .onChange(of: art != nil || (style == "picture" && image != nil), initial: true) { _, ready in
            guard ready, !revealed else { return }
            if reduceMotion { revealed = true } else { withAnimation(.easeOut(duration: 0.8)) { revealed = true } }
        }
    }

    @ViewBuilder
    private var banner: some View {
        if style == "picture", let image, width > 0 {
            let tall = height ?? min(Self.maxPictureHeight, width / AsciiBanner.minAspect)
            Image(decorative: image, scale: 1)
                .resizable()
                .aspectRatio(contentMode: .fill)
                .frame(width: width, height: tall)
                .clipped()
        } else if let art {
            Text(art)
                .font(Font(Self.font))
                .lineSpacing(0)
                .fixedSize()
        }
    }

    /// Clear at the sides, for a band.
    private static var sideFade: some View {
        LinearGradient(stops: [.init(color: .clear, location: 0), .init(color: .black, location: 0.15),
                               .init(color: .black, location: 0.85), .init(color: .clear, location: 1)],
                       startPoint: .leading, endPoint: .trailing)
    }

    /// Full at the top, fading out toward the foot, so the chat's middle
    /// stays calm under the composer.
    private static var footFade: some View {
        LinearGradient(stops: [.init(color: .black.opacity(0.9), location: 0), .init(color: .black.opacity(0.6), location: 0.45),
                               .init(color: .clear, location: 1)],
                       startPoint: .top, endPoint: .bottom)
    }

    private struct RenderKey: Equatable {
        var name: String
        var columns: Int
        var rows: Int
        var dark: Bool
        var loaded: Bool
    }

    /// The banner's text with each run of like-colored cells in its color.
    nonisolated static func attributed(_ banner: AsciiBanner) -> AttributedString {
        var out = AttributedString()
        for (index, row) in banner.rows.enumerated() {
            var run = ""
            var runColor: (Int, Int, Int)?
            func flush() {
                guard !run.isEmpty else { return }
                var part = AttributedString(run)
                if let color = runColor {
                    part.foregroundColor = Color(red: Double(color.0) / 15, green: Double(color.1) / 15, blue: Double(color.2) / 15)
                }
                out += part
                run = ""
            }
            for cell in row {
                // Colors rounded to 16 steps a channel, so runs stay long.
                let color = (Int(cell.red * 15), Int(cell.green * 15), Int(cell.blue * 15))
                if cell.character != " ", runColor.map({ $0 != color }) ?? true {
                    flush()
                    runColor = color
                }
                run.append(cell.character)
            }
            flush()
            if index < banner.rows.count - 1 { out += AttributedString("\n") }
        }
        return out
    }
}
