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

    var body: some View {
        Group {
            if url.isFileURL {
                if let image = NSImage(contentsOf: url) {
                    Image(nsImage: image).resizable().scaledToFit()
                } else {
                    missing
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
    }

    private var missing: some View {
        Label(alt.isEmpty ? url.lastPathComponent : alt, systemImage: "photo")
            .font(.callout)
            .foregroundStyle(.secondary)
            .padding(10)
            .background(.fill.tertiary, in: .rect(cornerRadius: 12))
    }
}

/// An image waiting in the composer, with a button to take it out.
struct ImageChip: View {
    let image: ImageAttachment
    let remove: () -> Void

    var body: some View {
        Group {
            if let picture = NSImage(data: image.data) {
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
