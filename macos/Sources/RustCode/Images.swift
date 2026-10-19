import AppKit
import QuickLook
import RustCodeKit
import SwiftUI
import UniformTypeIdentifiers

extension ImageAttachment {
    /// An image file as the engine takes it: PNG, JPEG, GIF and WebP as they
    /// are, any other image type converted to PNG. Nil when it is no image.
    static func load(_ url: URL) throws -> ImageAttachment? {
        guard url.isFileURL, let type = UTType(filenameExtension: url.pathExtension), type.conforms(to: .image) else { return nil }
        let data = try Data(contentsOf: url)
        if let mime = mimeType(forExtension: url.pathExtension) {
            return try checked(ImageAttachment(data: data, mimeType: mime))
        }
        guard let png = NSBitmapImageRep(data: data)?.representation(using: .png, properties: [:]) else { return nil }
        return try checked(ImageAttachment(data: png, mimeType: "image/png"))
    }

    /// The images on a pasteboard: image files copied in Finder, or image
    /// data such as a screenshot. Text, and files that are no images, are
    /// left to the text field's own paste.
    static func fromPasteboard(_ pasteboard: NSPasteboard) throws -> [ImageAttachment] {
        let files = pasteboard.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL] ?? []
        if !files.isEmpty { return try files.compactMap(load) }
        guard pasteboard.string(forType: .string) == nil else { return [] }
        if let png = pasteboard.data(forType: .png) {
            return [try checked(ImageAttachment(data: png, mimeType: "image/png"))]
        }
        if let tiff = pasteboard.data(forType: .tiff),
           let png = NSBitmapImageRep(data: tiff)?.representation(using: .png, properties: [:]) {
            return [try checked(ImageAttachment(data: png, mimeType: "image/png"))]
        }
        return []
    }

    private static func checked(_ image: ImageAttachment) throws -> ImageAttachment {
        guard image.data.count <= maxBytes else { throw ImageTooLarge() }
        return image
    }

    struct ImageTooLarge: LocalizedError {
        var errorDescription: String? { "Images must be 20 MB or smaller." }
    }

    /// A copy on disk for the sent message to show until the chat is reopened,
    /// when the engine's saved copy takes its place.
    func writePreview() -> URL? {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent("RustCode Images", isDirectory: true)
        let url = directory.appendingPathComponent("\(id.uuidString).\(fileExtension)")
        do {
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
            try data.write(to: url)
            return url
        } catch { return nil }
    }
}

/// An image in the transcript: a local file or a web image, opened in Quick
/// Look with a click.
struct InlineImage: View {
    let url: URL
    var alt = ""
    var maxSize: CGFloat = 420
    @State private var preview: URL?
    @State private var loaded: NSImage?
    @State private var failed = false

    var body: some View {
        Group {
            if url.isFileURL {
                if let image = loaded ?? Thumbnails.cached(url, maxSize: maxSize) {
                    Image(nsImage: image).resizable().scaledToFit()
                } else if failed {
                    missing
                } else {
                    RoundedRectangle(cornerRadius: 12).fill(.fill.quaternary)
                        .frame(width: min(maxSize, 220), height: min(maxSize, 220) * 0.75)
                }
            } else {
                AsyncImage(url: url) { phase in
                    if let image = phase.image { image.resizable().scaledToFit() } else if phase.error != nil { missing } else { ProgressView() }
                }
            }
        }
        .frame(maxWidth: maxSize, maxHeight: maxSize, alignment: .leading)
        .clipShape(.rect(cornerRadius: 12))
        .contentShape(.rect)
        .onTapGesture { if url.isFileURL { preview = url } else { NSWorkspace.shared.open(url) } }
        .help(alt.isEmpty ? url.lastPathComponent : alt)
        .accessibilityLabel(alt.isEmpty ? "Image" : alt)
        .quickLookPreview($preview)
        // Decoded off the main thread, at the size it is shown: a chat with
        // many screenshots read every file in full on each redraw.
        .task(id: url) {
            guard url.isFileURL, Thumbnails.cached(url, maxSize: maxSize) == nil else { return }
            loaded = await Thumbnails.load(url, maxSize: maxSize)
            failed = loaded == nil
        }
    }

    private var missing: some View {
        Label(alt.isEmpty ? url.lastPathComponent : alt, systemImage: "photo")
            .font(.callout)
            .foregroundStyle(.secondary)
            .padding(10)
            .background(.fill.tertiary, in: .rect(cornerRadius: 12))
    }
}

/// Images scaled down to the size the chat shows them, kept between redraws.
@MainActor
enum Thumbnails {
    private static let cache: NSCache<NSString, NSImage> = {
        let cache = NSCache<NSString, NSImage>()
        cache.countLimit = 300
        return cache
    }()

    private static func key(_ url: URL, _ maxSize: CGFloat) -> NSString {
        "\(url.path)|\(Int(maxSize))" as NSString
    }

    static func cached(_ url: URL, maxSize: CGFloat) -> NSImage? {
        cache.object(forKey: key(url, maxSize))
    }

    /// The file as an image at most `maxSize` points across, at Retina scale.
    static func load(_ url: URL, maxSize: CGFloat) async -> NSImage? {
        let pixels = Int(maxSize * 2)
        let cgImage = await Task.detached(priority: .userInitiated) { () -> CGImage? in
            guard let source = CGImageSourceCreateWithURL(url as CFURL, nil) else { return nil }
            return thumbnail(source, pixels: pixels)
        }.value
        // Formats Image I/O cannot thumbnail, such as PDF, load whole.
        guard let image = cgImage.map({ NSImage(cgImage: $0, size: NSSize(width: CGFloat($0.width) / 2, height: CGFloat($0.height) / 2)) })
                ?? NSImage(contentsOf: url) else { return nil }
        cache.setObject(image, forKey: key(url, maxSize))
        return image
    }

    /// An attachment's preview, decoded once per attachment.
    static func chip(_ attachment: ImageAttachment) -> NSImage? {
        let key = "chip|\(attachment.id.uuidString)" as NSString
        if let hit = cache.object(forKey: key) { return hit }
        guard let source = CGImageSourceCreateWithData(attachment.data as CFData, nil),
              let cgImage = thumbnail(source, pixels: 112) else { return nil }
        let image = NSImage(cgImage: cgImage, size: NSSize(width: CGFloat(cgImage.width) / 2, height: CGFloat(cgImage.height) / 2))
        cache.setObject(image, forKey: key)
        return image
    }

    nonisolated private static func thumbnail(_ source: CGImageSource, pixels: Int) -> CGImage? {
        CGImageSourceCreateThumbnailAtIndex(source, 0, [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceCreateThumbnailWithTransform: true,
            kCGImageSourceShouldCacheImmediately: true,
            kCGImageSourceThumbnailMaxPixelSize: pixels,
        ] as CFDictionary)
    }
}

/// An image waiting in the composer, with a button to take it out.
struct ImageChip: View {
    let image: ImageAttachment
    let remove: () -> Void

    var body: some View {
        Group {
            if let picture = Thumbnails.chip(image) {
                Image(nsImage: picture).resizable().scaledToFill()
            } else {
                Image(systemName: "photo").foregroundStyle(.secondary)
            }
        }
        .frame(width: 56, height: 56)
        .clipShape(.rect(cornerRadius: 10))
        .overlay(alignment: .topTrailing) {
            Button(action: remove) {
                Image(systemName: "xmark.circle.fill")
                    .symbolRenderingMode(.palette)
                    .foregroundStyle(.white, .black.opacity(0.6))
            }
            .buttonStyle(.plain)
            .padding(3)
            .help("Remove")
        }
    }
}
