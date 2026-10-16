import Foundation
import ImageIO
import RustCodeRemote
import UniformTypeIdentifiers

/// Downsample before decoding full-resolution camera images, and strip location/EXIF metadata.
enum PromptImageImport {
    nonisolated static func prepare(_ data: Data, maximumPixelSize: Int = 2048) throws -> PromptImage {
        guard data.count <= 40 * 1024 * 1024,
              let source = CGImageSourceCreateWithData(data as CFData, [kCGImageSourceShouldCache: false] as CFDictionary),
              let thumbnail = CGImageSourceCreateThumbnailAtIndex(source, 0, [
                kCGImageSourceCreateThumbnailFromImageAlways: true,
                kCGImageSourceCreateThumbnailWithTransform: true,
                kCGImageSourceThumbnailMaxPixelSize: maximumPixelSize,
                kCGImageSourceShouldCacheImmediately: true
              ] as CFDictionary) else { throw ImportError.unreadable }
        let output = NSMutableData()
        guard let destination = CGImageDestinationCreateWithData(output, UTType.jpeg.identifier as CFString, 1, nil) else {
            throw ImportError.unreadable
        }
        CGImageDestinationAddImage(destination, thumbnail, [kCGImageDestinationLossyCompressionQuality: 0.82] as CFDictionary)
        guard CGImageDestinationFinalize(destination), output.length <= PromptImage.maximumBytes else { throw ImportError.tooLarge }
        return PromptImage(data: output as Data, mimeType: "image/jpeg")
    }

    enum ImportError: LocalizedError {
        case unreadable, tooLarge
        var errorDescription: String? {
            switch self {
            case .unreadable: "This image could not be opened. Choose another photo or a smaller image."
            case .tooLarge: "This image is too large to send. Choose a smaller image."
            }
        }
    }
}
