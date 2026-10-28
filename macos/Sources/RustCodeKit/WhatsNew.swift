import Foundation

/// The notes of one release, for the What's New card and its full changelog
/// (#2506). Always tied to an exact tag, never to `releases/latest`, so the
/// notes shown are the ones of the version installed.
public struct ReleaseNotes: Codable, Equatable, Sendable, Identifiable {
    public static let repository = "LHagfoss/rustcode"

    public var tag: String
    /// The release's Markdown body; empty when it could not be fetched.
    public var body: String

    public var id: String { tag }
    public var version: AppVersion? { AppVersion(tag) }

    public init(tag: String, body: String) {
        self.tag = tag
        self.body = body
    }

    /// The release's page on GitHub, for "View on GitHub".
    public var pageURL: URL { Self.pageURL(tag: tag) }

    public static func pageURL(tag: String) -> URL {
        URL(string: "https://github.com/\(repository)/releases/tag/\(tag)")!
    }

    /// The REST endpoint for exactly this tag.
    public static func apiURL(tag: String) -> URL {
        URL(string: "https://api.github.com/repos/\(repository)/releases/tags/\(tag)")!
    }

    /// The tag a version is released under, as `scripts/release.sh` names it.
    public static func tag(for version: AppVersion) -> String { "v\(version)" }

    /// Reads a GitHub release object. Nil unless it is the release of
    /// `expectedTag`: a redirect or a wrong answer never shows another
    /// release's notes as this one's.
    public init?(apiJSON data: Data, expectedTag: String) {
        guard let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let tag = object["tag_name"] as? String, tag == expectedTag else { return nil }
        self.init(tag: tag, body: (object["body"] as? String) ?? "")
    }

    /// The body for reading: without GitHub's generator comment, its
    /// "What's Changed" heading and compare link, and with each generated
    /// line (`feat(macos): add x by @user in https://…/pull/12`) as a
    /// sentence linking its pull request.
    public var readableBody: String {
        body.components(separatedBy: .newlines)
            .compactMap { line -> String? in
                let trimmed = line.trimmingCharacters(in: .whitespaces)
                if trimmed.hasPrefix("<!--") && trimmed.hasSuffix("-->") { return nil }
                if trimmed.hasPrefix("**Full Changelog**") { return nil }
                if trimmed == "## What's Changed" { return nil }
                guard trimmed.hasPrefix("- ") || trimmed.hasPrefix("* ") else { return line }
                return "- " + Self.readableEntry(String(trimmed.dropFirst(2)))
            }
            .joined(separator: "\n")
            .trimmingCharacters(in: .whitespacesAndNewlines)
    }

    /// A generated line as `Add x ([#12](https://…/pull/12))`; any other
    /// line as it is.
    static func readableEntry(_ raw: String) -> String {
        guard let by = raw.range(of: " by @", options: .backwards),
              let pull = raw.range(of: #"https://github\.com/[^\s]+/pull/(\d+)$"#, options: .regularExpression)
        else { return raw }
        var text = String(raw[..<by.lowerBound])
        if let prefix = text.range(of: #"^[a-z]+(\([^)]*\))?!?:\s*"#, options: .regularExpression) {
            text.removeSubrange(prefix)
        }
        let url = String(raw[pull])
        let number = url.split(separator: "/").last.map(String.init) ?? ""
        return text.prefix(1).uppercased() + text.dropFirst() + " ([#\(number)](\(url)))"
    }

    /// A few notable changes for the card: features first, then fixes and
    /// speed-ups, each as a short sentence. Housekeeping entries (tests,
    /// chores, CI, docs, refactors) are left out.
    public func highlights(limit: Int = 4) -> [String] {
        var ranked: [(rank: Int, order: Int, text: String)] = []
        for (order, line) in body.components(separatedBy: .newlines).enumerated() {
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            guard trimmed.hasPrefix("- ") || trimmed.hasPrefix("* ") else { continue }
            guard let entry = Self.entry(String(trimmed.dropFirst(2))) else { continue }
            ranked.append((entry.rank, order, entry.text))
        }
        var seen = Set<String>()
        return ranked
            .sorted { ($0.rank, $0.order) < ($1.rank, $1.order) }
            .map(\.text)
            .filter { seen.insert($0).inserted }
            .prefix(limit)
            .map { $0 }
    }

    /// One bullet as a sentence and how notable it is (lower first), or nil
    /// for housekeeping. Handles GitHub's generated lines
    /// (`feat(macos): add x by @user in https://…/pull/12`) and curated ones
    /// (`Added x (#12)`).
    static func entry(_ raw: String) -> (rank: Int, text: String)? {
        var text = raw
        if let by = text.range(of: " by @", options: .backwards) { text = String(text[..<by.lowerBound]) }
        if let number = text.range(of: #"\s*\(#\d+\)\s*$"#, options: .regularExpression) {
            text.removeSubrange(number)
        }
        var rank = 1
        if let colon = text.range(of: #"^[a-z]+(\([^)]*\))?!?:\s*"#, options: .regularExpression) {
            let type = text[colon].prefix { $0.isLetter }
            switch type {
            case "feat": rank = 0
            case "fix", "perf": rank = 1
            default: return nil
            }
            text.removeSubrange(colon)
        }
        text = text.replacingOccurrences(of: "**", with: "").replacingOccurrences(of: "`", with: "")
            .trimmingCharacters(in: .whitespaces)
        guard !text.isEmpty, text.lowercased() != "none" else { return nil }
        let sentence = text.prefix(1).uppercased() + text.dropFirst()
        return (rank, sentence.hasSuffix(".") ? String(sentence) : sentence + ".")
    }
}

/// Whether a launch shows the What's New card: once per release, after the
/// app was updated to it (#2506).
public enum WhatsNewGate {
    public static let announcedKey = "whatsNewAnnouncedVersion"

    public enum Decision: Equatable, Sendable {
        /// Show this version's notes, then record it.
        case announce(AppVersion)
        /// The first launch ever: remember this version without a card, so
        /// a fresh install is not greeted with a changelog.
        case record(AppVersion)
        /// Nothing to show: already announced, or a development build.
        case nothing
    }

    /// `current` is the running app's version, nil for a development build;
    /// `announced` the version last recorded. `launchedBefore` says the app
    /// had saved state before this launch: with no version recorded, that is
    /// an update from a release older than the card, not a fresh install,
    /// and it is announced. Every update to 0.104.0 was taken for a fresh
    /// install and recorded silently.
    public static func decide(current: AppVersion?, announced: String?, launchedBefore: Bool = false) -> Decision {
        guard let current else { return .nothing }
        guard let announced, let last = AppVersion(announced) else {
            return launchedBefore ? .announce(current) : .record(current)
        }
        return current > last ? .announce(current) : .nothing
    }
}
