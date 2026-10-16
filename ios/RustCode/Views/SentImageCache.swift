import RustCodeRemote
import SwiftUI
import UIKit

/// Keeps copies of images sent from this phone so matching transcript rows can show them later.
struct SentImageCache: Sendable {
    static let shared = SentImageCache()

    private let directory: URL

    init() {
        directory = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("SentImages", isDirectory: true)
    }

    func imageURL(for attachmentID: String) -> URL? {
        guard !attachmentID.isEmpty, attachmentID.utf8.count <= 64,
              attachmentID.utf8.allSatisfy({
                  (48...57).contains($0) || (65...90).contains($0) || (97...122).contains($0) || $0 == 45 || $0 == 95
              }) else { return nil }
        return directory.appendingPathComponent(attachmentID, isDirectory: false)
    }

    func contains(_ attachmentID: String) -> Bool {
        guard let url = imageURL(for: attachmentID) else { return false }
        return FileManager.default.fileExists(atPath: url.path)
    }

    func save(_ images: [PromptImage]) throws {
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        for image in images {
            guard let url = imageURL(for: image.id) else { continue }
            try image.data.write(to: url, options: .atomic)
        }
        pruneIfNeeded()
    }

    private func pruneIfNeeded() {
        guard let files = try? FileManager.default.contentsOfDirectory(
            at: directory,
            includingPropertiesForKeys: [.fileSizeKey, .contentModificationDateKey],
            options: [.skipsHiddenFiles]
        ) else { return }
        let entries = files.compactMap { url -> (URL, Int, Date)? in
            guard let values = try? url.resourceValues(forKeys: [.fileSizeKey, .contentModificationDateKey]),
                  let size = values.fileSize, let date = values.contentModificationDate else { return nil }
            return (url, size, date)
        }.sorted { $0.2 < $1.2 }
        var total = entries.reduce(0) { $0 + $1.1 }
        for (url, size, _) in entries where total > 256 * 1024 * 1024 {
            try? FileManager.default.removeItem(at: url)
            total -= size
        }
    }
}

struct SentImageThumbnail: View {
    let attachmentID: String
    var onOpen: () -> Void
    @State private var thumbnail: UIImage?
    @State private var isUnavailable = false

    var body: some View {
        Button(action: onOpen) {
            Group {
            if let thumbnail {
                Image(uiImage: thumbnail).resizable().scaledToFill()
            } else if isUnavailable {
                Rectangle().fill(.fill.tertiary).overlay {
                    VStack(spacing: 5) {
                        Image(systemName: "photo").font(.title2)
                        Text("Image attachment").font(.caption2)
                    }
                    .foregroundStyle(.secondary)
                }
            } else {
                Rectangle().fill(.fill.tertiary).overlay {
                    ProgressView().controlSize(.small)
                }
            }
            }
            .frame(width: 112, height: 112)
            .clipped()
            .clipShape(.rect(cornerRadius: 12))
        }
        .buttonStyle(.plain)
        .disabled(thumbnail == nil)
        .task(id: attachmentID) {
            guard let url = SentImageCache.shared.imageURL(for: attachmentID) else {
                isUnavailable = true
                return
            }
            for _ in 0..<12 {
                guard !Task.isCancelled else { return }
                if let data = try? await Task.detached(priority: .utility, operation: { try Data(contentsOf: url) }).value {
                    let preview = try? await Task.detached(priority: .utility) {
                        try PromptImageImport.prepare(data, maximumPixelSize: 336).data
                    }.value
                    guard !Task.isCancelled else { return }
                    thumbnail = preview.flatMap(UIImage.init(data:))
                    isUnavailable = thumbnail == nil
                    return
                }
                try? await Task.sleep(for: .milliseconds(100))
            }
            isUnavailable = true
        }
        .accessibilityLabel("Attached image")
    }
}

struct SentImageViewer: View {
    let attachmentID: String
    @Environment(\.dismiss) private var dismiss
    @State private var image: UIImage?
    @State private var zoom = 1.0
    @GestureState private var pinchScale = 1.0

    var body: some View {
        NavigationStack {
            Group {
                if let image {
                    Image(uiImage: image)
                        .resizable()
                        .scaledToFit()
                        .scaleEffect(zoom * pinchScale)
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                        .contentShape(.rect)
                        .gesture(MagnifyGesture()
                            .updating($pinchScale) { value, state, _ in state = value.magnification }
                            .onEnded { value in zoom = min(max(zoom * value.magnification, 1), 5) })
                        .onTapGesture(count: 2) { withAnimation(.snappy) { zoom = zoom > 1 ? 1 : 2 } }
                } else {
                    ProgressView().tint(.white)
                }
            }
            .background(.black)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Done", systemImage: "xmark") { dismiss() }
                        .labelStyle(.iconOnly)
                        .accessibilityLabel("Close image")
                }
            }
            .toolbarBackground(.hidden, for: .navigationBar)
            .task(id: attachmentID) {
                guard let url = SentImageCache.shared.imageURL(for: attachmentID) else { return }
                let data = try? await Task.detached(priority: .userInitiated) { try Data(contentsOf: url) }.value
                guard !Task.isCancelled, let data else { return }
                image = UIImage(data: data)
            }
        }
        .preferredColorScheme(.dark)
    }
}
