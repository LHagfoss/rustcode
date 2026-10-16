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
    public static let latestURL = URL(string: "https://api.github.com/repos/LHagfoss/rustcode/releases/latest")!

    public var version: AppVersion
    public var tag: String
    public var notes: String
    public var pageURL: URL?
    public var appURL: URL
    public var checksumsURL: URL?

    /// Reads `GET /repos/{owner}/{repo}/releases/latest`. Nil for a release
    /// without a macOS app build.
    public init?(githubJSON data: Data) {
        guard let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let tag = json["tag_name"] as? String,
              let version = AppVersion(tag) else { return nil }
        let assets = (json["assets"] as? [[String: Any]]) ?? []
        func asset(_ name: String) -> URL? {
            assets.first { $0["name"] as? String == name }
                .flatMap { $0["browser_download_url"] as? String }
                .flatMap(URL.init(string:))
        }
        guard let app = asset(Self.appAssetName) else { return nil }
        self.version = version
        self.tag = tag
        self.notes = json["body"] as? String ?? ""
        self.pageURL = (json["html_url"] as? String).flatMap(URL.init(string:))
        self.appURL = app
        self.checksumsURL = asset(Self.checksumsAssetName)
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
