import Foundation

/// A dotted numeric version (`0.69.0`, `v1.2`), compared part by part.
public struct AppVersion: Comparable, Equatable, Sendable, CustomStringConvertible {
    public let parts: [Int]

    public init?(_ text: String) {
        let trimmed = text.trimmingCharacters(in: .whitespaces)
        let core = (trimmed.hasPrefix("v") ? trimmed.dropFirst() : Substring(trimmed))
            .split(separator: "-").first ?? ""
        let parts = core.split(separator: ".").map { Int($0) }
        guard !parts.isEmpty, parts.allSatisfy({ $0 != nil }) else { return nil }
        self.parts = parts.compactMap { $0 }
    }

    public var description: String { parts.map(String.init).joined(separator: ".") }

    public static func < (lhs: AppVersion, rhs: AppVersion) -> Bool {
        for index in 0..<max(lhs.parts.count, rhs.parts.count) {
            let left = index < lhs.parts.count ? lhs.parts[index] : 0
            let right = index < rhs.parts.count ? rhs.parts[index] : 0
            if left != right { return left < right }
        }
        return false
    }

    public static func == (lhs: AppVersion, rhs: AppVersion) -> Bool { !(lhs < rhs) && !(rhs < lhs) }
}

/// The parts of a GitHub release the app updater needs.
public struct AppRelease: Equatable, Sendable {
    public static let appAssetName = "rustcode-macos-aarch64-swift-app.zip"
    public static let checksumsAssetName = "SHA256SUMS"
    /// The web page GitHub redirects to the latest release's tag. Unlike the
    /// REST API, which allows 60 unauthenticated requests an hour per address
    /// and answered 403 once polling had spent them, it has no such limit.
    public static let latestURL = URL(string: "https://github.com/LHagfoss/rustcode/releases/latest")!

    public var version: AppVersion
    public var tag: String
    public var pageURL: URL?
    public var appURL: URL
    public var checksumsURL: URL?

    /// Reads the release page `releases/latest` redirected to,
    /// `…/releases/tag/v0.95.0`; its assets download from the tag. Nil for any
    /// other page.
    public init?(releasePage url: URL) {
        let parts = url.pathComponents
        guard let index = parts.lastIndex(of: "tag"), index > 0, parts[index - 1] == "releases",
              index + 1 < parts.count, let version = AppVersion(parts[index + 1]) else { return nil }
        let tag = parts[index + 1]
        let download = url.deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("download").appendingPathComponent(tag)
        self.version = version
        self.tag = tag
        self.pageURL = url
        self.appURL = download.appendingPathComponent(Self.appAssetName)
        self.checksumsURL = download.appendingPathComponent(Self.checksumsAssetName)
    }

    /// The hex SHA-256 listed for `name` in a `sha256sum`-style file.
    public static func checksum(for name: String, in sums: String) -> String? {
        for line in sums.split(whereSeparator: \.isNewline) {
            let fields = line.split(separator: " ", omittingEmptySubsequences: true)
            guard fields.count >= 2 else { continue }
            let file = fields[fields.count - 1].trimmingCharacters(in: CharacterSet(charactersIn: "*"))
            if file == name || file.hasSuffix("/" + name) { return fields[0].lowercased() }
        }
        return nil
    }
}
