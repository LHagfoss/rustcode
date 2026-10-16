import Foundation

/// An image added to the composer, sent to the engine as an ACP image block.
/// The engine saves it and refers to it with a `![image](file://…)` marker.
public struct ImageAttachment: Identifiable, Equatable, Sendable {
    /// The engine refuses larger images; checked here to fail before sending.
    public static let maxBytes = 20 * 1024 * 1024

    public let id: UUID
    public let data: Data
    public let mimeType: String

    public init(id: UUID = UUID(), data: Data, mimeType: String) {
        self.id = id
        self.data = data
        self.mimeType = mimeType
    }

    /// The MIME type the engine accepts for a file extension, if any.
    public static func mimeType(forExtension pathExtension: String) -> String? {
        switch pathExtension.lowercased() {
        case "png": "image/png"
        case "jpg", "jpeg": "image/jpeg"
        case "gif": "image/gif"
        case "webp": "image/webp"
        default: nil
        }
    }

    public var fileExtension: String {
        switch mimeType {
        case "image/jpeg": "jpg"
        case "image/gif": "gif"
        case "image/webp": "webp"
        default: "png"
        }
    }

    public var block: JSONValue {
        ["type": "image", "mimeType": .string(mimeType), "data": .string(data.base64EncodedString())]
    }
}

/// The `![image](file://…)` markers the engine stores in a user message for
/// each attached image, separated from the text around them.
public enum ImageMarkers {
    static let prefix = "![image](file://"

    public static func marker(for url: URL) -> String { "![image](file://\(url.path))" }

    /// The message text without its markers, and the marked files in order.
    public static func split(_ text: String) -> (text: String, images: [URL]) {
        guard text.contains(prefix) else { return (text, []) }
        var images: [URL] = []
        var rest = Substring(text)
        var kept = ""
        while let start = rest.range(of: prefix) {
            guard let close = rest[start.upperBound...].firstIndex(of: ")") else { break }
            let path = String(rest[start.upperBound..<close])
            guard path.hasPrefix("/"), !path.contains("\n") else {
                kept += rest[..<start.upperBound]
                rest = rest[start.upperBound...]
                continue
            }
            kept += rest[..<start.lowerBound]
            images.append(URL(fileURLWithPath: path))
            rest = rest[rest.index(after: close)...]
            // The engine puts each marker on a line of its own.
            if rest.first == "\n" { rest = rest.dropFirst() }
        }
        kept += rest
        return (kept.trimmingCharacters(in: .whitespacesAndNewlines), images)
    }
}
