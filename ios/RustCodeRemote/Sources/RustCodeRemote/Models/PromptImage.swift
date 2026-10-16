import Foundation

/// Prepared JPEG or PNG bytes. Keep the identifier stable while retrying an upload.
public struct PromptImage: Identifiable, Equatable, Sendable {
    public static let maximumBytes = 8 * 1024 * 1024
    public static let chunkBytes = 128 * 1024
    public let id: String
    public let data: Data
    public let mimeType: String
    public init(id: String = UUID().uuidString.lowercased(), data: Data, mimeType: String) {
        self.id = id; self.data = data; self.mimeType = mimeType
    }
}

public struct ImageUploadChunk: Codable, Equatable, Sendable {
    public var attachmentID: String
    public var mimeType: String
    public var offset: UInt64
    public var totalBytes: UInt64
    public var dataBase64: String
    public init(attachmentID: String, mimeType: String, offset: UInt64, totalBytes: UInt64, dataBase64: String) {
        self.attachmentID = attachmentID; self.mimeType = mimeType; self.offset = offset
        self.totalBytes = totalBytes; self.dataBase64 = dataBase64
    }
    enum CodingKeys: String, CodingKey {
        case attachmentID = "attachment_id", mimeType = "mime_type", offset
        case totalBytes = "total_bytes", dataBase64 = "data_base64"
    }
}

public struct ImageUploadProgress: Codable, Equatable, Sendable {
    public var attachmentID: String
    public var nextOffset: UInt64
    public var totalBytes: UInt64
    public var complete: Bool
    public var promptReference: String?
    public init(attachmentID: String, nextOffset: UInt64, totalBytes: UInt64, complete: Bool, promptReference: String? = nil) {
        self.attachmentID = attachmentID; self.nextOffset = nextOffset; self.totalBytes = totalBytes
        self.complete = complete; self.promptReference = promptReference
    }
    enum CodingKeys: String, CodingKey {
        case attachmentID = "attachment_id", nextOffset = "next_offset", totalBytes = "total_bytes"
        case complete, promptReference = "prompt_reference"
    }
}
