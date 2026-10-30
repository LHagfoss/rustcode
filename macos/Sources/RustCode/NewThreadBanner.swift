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

    /// Decoded pictures and drawn text, kept so a new thread shows its
    /// banner at once instead of loading it each time it opens.
    @MainActor static var images: [String: CGImage] = [:]
    @MainActor static var drawings: [String: NewThreadBannerView.Drawing] = [:]

    /// The picture `name`, read at most 2400 pixels wide: enough for any
    /// window, and a camera's original is many times that.
    nonisolated static func load(_ name: String) -> CGImage? {
        guard let url = url(name), let source = CGImageSourceCreateWithURL(url as CFURL, nil) else { return nil }
        let options: [CFString: Any] = [kCGImageSourceCreateThumbnailFromImageAlways: true,
                                        kCGImageSourceCreateThumbnailWithTransform: true,
                                        kCGImageSourceThumbnailMaxPixelSize: 2400]
        return CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary)
    }

    /// Read the chosen picture ahead of the first new thread.
    @MainActor static func preload() {
        let name = UserDefaults.standard.string(forKey: key) ?? ""
        guard !name.isEmpty, images[name] == nil else { return }
        Task.detached(priority: .utility) {
            guard let image = load(name) else { return }
            await MainActor.run { images[name] = image }
        }
    }
}

/// The banner as the new thread shows it: the picture in colored text, or
/// the picture itself, across the room it has. Given a `height`, it fills
/// that much of the top of the chat, cropped to fit, and fades to the chat
/// toward its foot; without one it is a band at least 3:1. A picture seen
/// before shows at once; a new one fades in when it is ready.
struct NewThreadBannerView: View {
    let name: String
    let style: String
    /// The height to fill, or nil for a band as tall as the picture allows.
    var height: CGFloat?

    init(name: String, style: String, height: CGFloat? = nil) {
        self.name = name
        self.style = style
        self.height = height
        _image = State(initialValue: NewThreadBanner.images[name])
    }
    @Environment(\.colorScheme) private var colorScheme
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.displayScale) private var displayScale
    @State private var width: CGFloat = 0
    @State private var drawnHeight: CGFloat = 0
    @State private var image: CGImage?
    /// The text drawn once into a bitmap: as styled text, its thousands of
    /// colored runs were laid out and drawn again on every frame the chat
    /// changed size, which stalled the sidebar's slide.
    @State private var art: Drawing?

    /// The banner's text as a picture, and the size the text takes.
    struct Drawing: @unchecked Sendable {
        let image: CGImage
        let size: CGSize
        let scale: CGFloat
    }

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
                // Exactly the room, so the foot's fade stays where the room
                // ends while the drawing under it is scaled or redrawn.
                .frame(width: width, height: height, alignment: .top)
                .clipped()
                .mask { height == nil ? AnyView(Self.sideFade) : AnyView(Self.footFade) }
                .animation(reduceMotion ? nil : .easeOut(duration: 0.25), value: art == nil && image == nil)
            }
            .clipped()
            .allowsHitTesting(false)
            .accessibilityHidden(true)
        .task(id: name) {
            if let cached = NewThreadBanner.images[name] {
                image = cached
                return
            }
            let name = name
            let loaded = await Task.detached(priority: .userInitiated) { NewThreadBanner.load(name) }.value
            if let loaded { NewThreadBanner.images[name] = loaded }
            image = loaded
        }
        .task(id: key) {
            guard let image, columns > 0, style != "picture" else { return }
            let key = key
            if let cached = NewThreadBanner.drawings[key.cacheKey] {
                art = cached
                return
            }
            // Settle a resize before drawing again.
            if art != nil { try? await Task.sleep(for: .milliseconds(120)) }
            let columns = columns, inverted = colorScheme != .dark, aspect = aspect, fill = height != nil
            let scale = displayScale
            let drawn = await Task.detached(priority: .userInitiated) {
                Self.draw(AsciiBanner.render(image, columns: columns, inverted: inverted, aspect: aspect, fill: fill), scale: scale)
            }.value
            guard !Task.isCancelled, let rendered = drawn else { return }
            NewThreadBanner.drawings[key.cacheKey] = rendered
            art = rendered
        }
    }

    private var key: RenderKey {
        RenderKey(name: name, columns: columns, rows: rows, dark: colorScheme == .dark, loaded: image != nil)
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
            // While a resize waits for the next drawing, this one is scaled
            // to fill the new room, so the banner follows the window live.
            Image(decorative: art.image, scale: art.scale)
                .resizable()
                .frame(width: art.size.width, height: art.size.height)
                .scaleEffect(fillScale, anchor: .top)
        }
    }

    /// How much the drawing grows to cover the room, cropped at the sides
    /// when it is relatively taller: the room's width, and with a `height`
    /// its height too.
    private var fillScale: CGFloat {
        guard let artSize = art?.size, artSize.width > 0, width > 0 else { return 1 }
        var scale = width / artSize.width
        if let height, artSize.height > 0 { scale = max(scale, height / artSize.height) }
        return scale
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
        var cacheKey: String { "\(name) \(columns)x\(rows) \(dark)" }
    }

    /// `banner` drawn as its text would be, at `scale` pixels a point: each
    /// row a line in the banner's font, each run of like-colored cells in
    /// its color.
    nonisolated static func draw(_ banner: AsciiBanner, scale: CGFloat) -> Drawing? {
        let size = CGSize(width: ceil(CGFloat(banner.columns) * charWidth), height: ceil(CGFloat(banner.rows.count) * lineHeight))
        guard size.width > 0, size.height > 0,
              let context = CGContext(data: nil, width: Int(size.width * scale), height: Int(size.height * scale),
                                      bitsPerComponent: 8, bytesPerRow: 0, space: CGColorSpace(name: CGColorSpace.sRGB)!,
                                      bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
        else { return nil }
        context.scaleBy(x: scale, y: scale)
        let ctFont = font as CTFont
        let baseline = lineHeight - CTFontGetDescent(ctFont) - (lineHeight - CTFontGetAscent(ctFont) - CTFontGetDescent(ctFont)) / 2
        for (index, row) in banner.rows.enumerated() {
            let line = NSMutableAttributedString()
            var run = ""
            var runColor: (Int, Int, Int)?
            func flush() {
                guard !run.isEmpty else { return }
                let color = runColor.map { CGColor(srgbRed: CGFloat($0.0) / 15, green: CGFloat($0.1) / 15, blue: CGFloat($0.2) / 15, alpha: 1) }
                    ?? CGColor(gray: 0.5, alpha: 1)
                line.append(NSAttributedString(string: run, attributes: [.font: ctFont, kCTForegroundColorAttributeName as NSAttributedString.Key: color]))
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
            // Core Graphics counts up from the foot.
            context.textPosition = CGPoint(x: 0, y: size.height - CGFloat(index) * lineHeight - baseline)
            CTLineDraw(CTLineCreateWithAttributedString(line), context)
        }
        return context.makeImage().map { Drawing(image: $0, size: size, scale: scale) }
    }
}
