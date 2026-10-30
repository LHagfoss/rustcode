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
        panel.message = "Choose a wide picture for new threads. It is cropped to at least 3:1."
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
/// the picture itself, cropped to a band at least 3:1 and revealed left to
/// right as it appears.
struct NewThreadBannerView: View {
    let name: String
    let style: String
    @Environment(\.colorScheme) private var colorScheme
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var width: CGFloat = 0
    @State private var image: CGImage?
    @State private var art: AttributedString?
    @State private var revealed = false

    /// The text's size: small enough that a band reads as a picture.
    static let fontSize: CGFloat = 7
    /// Rows the text banner takes at most; a band taller than this is drawn
    /// with fewer columns.
    static let maxRows = 22

    private var columns: Int {
        guard let image else { return 0 }
        let charWidth = Self.fontSize * 0.6
        let fit = Int(width / charWidth)
        let crop = AsciiBanner.crop(width: image.width, height: image.height)
        let byRows = Int(Double(Self.maxRows) * crop.width / max(crop.height, 1) * AsciiBanner.cellAspect)
        return max(20, min(fit, byRows, 200))
    }

    var body: some View {
        Group {
            if style == "picture", let image {
                let crop = AsciiBanner.crop(width: image.width, height: image.height)
                if let band = image.cropping(to: crop) {
                    Image(decorative: band, scale: 1)
                        .resizable()
                        .aspectRatio(contentMode: .fit)
                        .frame(maxHeight: 180)
                        .clipShape(.rect(cornerRadius: 14))
                }
            } else if let art {
                Text(art)
                    .font(.system(size: Self.fontSize, design: .monospaced))
                    .lineSpacing(0)
                    .fixedSize()
                    .accessibilityHidden(true)
            }
        }
        .mask(alignment: .leading) {
            GeometryReader { proxy in
                Rectangle().frame(width: revealed ? proxy.size.width : 0)
            }
        }
        .frame(maxWidth: .infinity)
        .onGeometryChange(for: CGFloat.self) { $0.size.width } action: { width = $0 }
        .task(id: name) {
            image = await Task.detached(priority: .userInitiated) {
                NewThreadBanner.url(name).flatMap { url in
                    CGImageSourceCreateWithURL(url as CFURL, nil).flatMap { CGImageSourceCreateImageAtIndex($0, 0, nil) }
                }
            }.value
        }
        .task(id: RenderKey(name: name, columns: columns, dark: colorScheme == .dark, loaded: image != nil)) {
            guard let image, columns > 0, style != "picture" else { return }
            let columns = columns, inverted = colorScheme != .dark
            let rendered = await Task.detached(priority: .userInitiated) {
                Self.attributed(AsciiBanner.render(image, columns: columns, inverted: inverted))
            }.value
            guard !Task.isCancelled else { return }
            art = rendered
        }
        .onChange(of: art != nil || (style == "picture" && image != nil), initial: true) { _, ready in
            guard ready, !revealed else { return }
            if reduceMotion { revealed = true } else { withAnimation(.easeOut(duration: 0.7)) { revealed = true } }
        }
    }

    private struct RenderKey: Equatable {
        var name: String
        var columns: Int
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
