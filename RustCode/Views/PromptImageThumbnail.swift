import RustCodeRemote
import SwiftUI
import UIKit

/// Decode a small preview once per attachment, rather than decoding a full JPEG on every keystroke.
struct PromptImageThumbnail: View {
    let image: PromptImage
    @State private var thumbnail: UIImage?

    var body: some View {
        Group {
            if let thumbnail {
                Image(uiImage: thumbnail).resizable().scaledToFill()
            } else {
                Rectangle().fill(.fill.tertiary).overlay { ProgressView().controlSize(.small) }
            }
        }
        .frame(width: 72, height: 72).clipped().clipShape(.rect(cornerRadius: 12))
        .task(id: image.id) {
            let data = image.data
            let preview = try? await Task.detached { try PromptImageImport.prepare(data, maximumPixelSize: 216).data }.value
            guard !Task.isCancelled, let preview else { return }
            thumbnail = UIImage(data: preview)
        }
    }
}
