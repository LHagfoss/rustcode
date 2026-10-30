import Foundation

/// An order to read a pull request in (#2647): stops through its changed
/// files, contracts and risky code first and tests after, each pointing at
/// real changed lines. Built from the diff alone, on this Mac: it reads
/// nothing else, changes nothing, and posts nothing.
///
/// What it says about a stop is analysis of the diff's text: the patterns it
/// flags are places to look, not findings, and nothing in it is verified.
/// Files the diff did not carry are listed as not covered.
public struct ReviewGuide: Codable, Equatable, Sendable {
    /// A changed line the guide points at.
    public struct Anchor: Codable, Equatable, Sendable, Hashable {
        public var path: String
        public var line: Int
        public var text: String
        /// The line was removed: `line` is in the old file.
        public var removed: Bool

        public init(path: String, line: Int, text: String, removed: Bool = false) {
            self.path = path
            self.line = line
            self.text = text
            self.removed = removed
        }

        public var label: String { "\(path):\(line)\(removed ? " (removed)" : "")" }
    }

    /// Something in the diff worth a careful look, with where it is.
    public struct Flag: Codable, Equatable, Sendable {
        public var message: String
        public var anchor: Anchor?
    }

    public enum Kind: String, Codable, Sendable, CaseIterable {
        case contract, sensitive, change, tests, docs

        /// Why a file of this kind comes where it does.
        public var reason: String {
            switch self {
            case .contract: "Defines an interface, schema, protocol or build setting other code depends on: read it first."
            case .sensitive: "Touches permissions, credentials, sandboxing or deletion: check what it allows."
            case .change: "Changed code: the behavior the pull request is about."
            case .tests: "Tests: check they exercise the changes above."
            case .docs: "Documentation and other files."
            }
        }

        public var title: String {
            switch self {
            case .contract: "Contracts"
            case .sensitive: "Sensitive code"
            case .change: "Changes"
            case .tests: "Tests"
            case .docs: "Docs and other"
            }
        }
    }

    public struct Stop: Codable, Equatable, Sendable, Identifiable {
        public var path: String
        public var kind: Kind
        public var added: Int
        public var removed: Int
        /// The first changed line, to open the diff at.
        public var first: Anchor?
        public var flags: [Flag]

        public var id: String { path }
    }

    /// The commit the guide was made at.
    public var head: String
    public var stops: [Stop]
    /// Changed files the diff did not carry (binary, past a cut, renamed
    /// without changes): not covered by the guide.
    public var uncovered: [String]
    /// Changed code files under a folder where no test changed: a question
    /// to ask, not a gap found.
    public var untestedFolders: [String]
    /// The diff was cut: later files may be missing.
    public var truncated: Bool
    /// The reader's notes, by stop. Kept when the guide is made again.
    public var notes: [String: String] = [:]
    /// Stops the reader marked read.
    public var read: Set<String> = []

    /// The most stops a guide lists; the rest are listed as remaining.
    public static let maxStops = 60

    /// Whether the pull request moved on since the guide was made.
    public func isStale(head current: String) -> Bool { !current.isEmpty && current != head }

    /// The guide for `files` as the diff has them, at `head`. `listed` is
    /// every path the pull request changes, to name the ones not covered.
    /// `previous` keeps its notes and read marks for stops still there.
    public static func build(files: [GitFileChange], listed: [String], head: String, truncated: Bool,
                             previous: ReviewGuide? = nil) -> ReviewGuide {
        let carried = files.filter { !$0.isBinary && !$0.rows.isEmpty }
        var stops = carried.map { file -> Stop in
            Stop(path: file.path, kind: kind(of: file.path), added: file.added, removed: file.removed,
                 first: firstChange(in: file), flags: flags(in: file))
        }
        let order = Dictionary(uniqueKeysWithValues: Kind.allCases.enumerated().map { ($1, $0) })
        stops.sort { a, b in
            if a.kind != b.kind { return order[a.kind, default: 0] < order[b.kind, default: 0] }
            // Flagged files first, then the larger changes.
            if a.flags.isEmpty != b.flags.isEmpty { return !a.flags.isEmpty }
            if a.added + a.removed != b.added + b.removed { return a.added + a.removed > b.added + b.removed }
            return a.path < b.path
        }
        var uncovered = Set(listed).subtracting(carried.map(\.path))
        if stops.count > maxStops {
            uncovered.formUnion(stops[maxStops...].map(\.path))
            stops.removeLast(stops.count - maxStops)
        }
        // Tests written beside the code, as Rust's are, count too.
        let inlineTests = Set(carried.filter(addsTests).map(\.path))
        var testedFolders: Set<String> = []
        var codeFolders: Set<String> = []
        for stop in stops {
            if stop.kind == .tests || inlineTests.contains(stop.path) { testedFolders.insert(folder(of: stop.path)) }
            if stop.kind == .change || stop.kind == .sensitive { codeFolders.insert(folder(of: stop.path)) }
        }
        let untested = codeFolders.subtracting(testedFolders)
        let paths = Set(stops.map(\.path))
        return ReviewGuide(head: head, stops: stops, uncovered: uncovered.sorted(), untestedFolders: untested.sorted(),
                           truncated: truncated,
                           notes: previous?.notes.filter { paths.contains($0.key) } ?? [:],
                           read: previous?.read.intersection(paths) ?? [])
    }

    /// Which kind of stop a path is, from its name.
    public static func kind(of path: String) -> Kind {
        let lower = path.lowercased()
        let name = (lower as NSString).lastPathComponent
        if lower.contains("test") || lower.contains("spec") || lower.hasPrefix("e2e") { return .tests }
        if lower.hasSuffix(".md") || lower.hasPrefix("docs/") || lower.hasSuffix(".txt") || lower.hasSuffix(".png") {
            return .docs
        }
        let contracts = ["cargo.toml", "package.swift", "package.json", "info.plist", ".entitlements", "schema",
                         "migration", "protocol", "proto", "/api", "acptypes", ".github/workflows"]
        if contracts.contains(where: { lower.contains($0) }) || name.hasSuffix(".toml") || name.hasSuffix(".yml")
            || name.hasSuffix(".yaml") { return .contract }
        let sensitive = ["auth", "sandbox", "permission", "secret", "token", "credential", "keychain", "crypto",
                         "approval", "security", "trash", "cleanup", "delete", "remote"]
        if sensitive.contains(where: { lower.contains($0) }) { return .sensitive }
        return .change
    }

    /// Patterns on added lines worth a look, each at its line, and file-wide
    /// ones: a large change, a file deleted, tests removed.
    static func flags(in file: GitFileChange) -> [Flag] {
        let patterns: [(String, String)] = [
            ("unsafe", "Uses unsafe code"),
            (".unwrap()", "Unwraps a value that may be missing"),
            ("try!", "Force-tries a call"),
            ("as!", "Force-casts a value"),
            ("expect(", "Panics when a value is missing"),
            ("todo", "Leaves a TODO"),
            ("fixme", "Leaves a FIXME"),
            ("sleep(", "Waits a fixed time"),
            ("--force", "Forces a command"),
            ("rm -rf", "Removes files recursively"),
            ("removeitem", "Deletes files"),
            ("remove_dir_all", "Deletes a folder tree"),
            ("password", "Mentions a password"),
            ("api_key", "Mentions an API key"),
            ("http://", "Uses plain HTTP"),
            ("#[ignore]", "Skips a test"),
            ("xctskip", "Skips a test"),
        ]
        var flags: [Flag] = []
        var seen: Set<String> = []
        for case .line(let line) in file.rows where line.kind == .added {
            let lower = line.text.lowercased()
            for (needle, message) in patterns where lower.contains(needle) && !seen.contains(message) {
                seen.insert(message)
                flags.append(Flag(message: message, anchor: line.newNumber.map {
                    Anchor(path: file.path, line: $0, text: line.text.trimmingCharacters(in: .whitespaces))
                }))
            }
        }
        if file.status == .deleted {
            flags.append(Flag(message: "Deletes the file", anchor: nil))
        } else if kind(of: file.path) == .tests, file.removed > file.added * 2, file.removed >= 10 {
            flags.append(Flag(message: "Removes more test lines than it adds", anchor: nil))
        }
        if file.added + file.removed >= 300 {
            flags.append(Flag(message: "Large change: \(file.added + file.removed) lines", anchor: nil))
        }
        return flags
    }

    static func firstChange(in file: GitFileChange) -> Anchor? {
        for case .line(let line) in file.rows where line.kind != .context {
            if let number = line.number {
                return Anchor(path: file.path, line: number, text: line.text.trimmingCharacters(in: .whitespaces),
                              removed: line.kind == .removed)
            }
        }
        return nil
    }

    static func folder(of path: String) -> String {
        // A test folder answers for the code folder beside it: `Tests/KitTests`
        // for `Sources/Kit`, `tests/` for `src/`.
        let parts = path.split(separator: "/").dropLast()
            .filter { !["tests", "test", "Tests", "src", "Sources"].contains($0) }
            .map { $0.hasSuffix("Tests") && $0.count > 5 ? String($0.dropLast(5)) : String($0) }
        return parts.prefix(2).joined(separator: "/")
    }

    /// The file's added lines declare a test.
    static func addsTests(_ file: GitFileChange) -> Bool {
        file.rows.contains { row in
            guard case .line(let line) = row, line.kind == .added else { return false }
            return line.text.contains("#[test]") || line.text.contains("#[tokio::test]") || line.text.contains("func test")
        }
    }

    /// A message asking the agent about one stop, with its evidence, for the
    /// reader to send.
    public func question(about stop: Stop, number: Int) -> String {
        var lines = ["About pull request #\(number) at commit \(String(head.prefix(7))), file \(stop.path) (+\(stop.added) −\(stop.removed))."]
        if let first = stop.first { lines.append("It first changes \(first.label): `\(first.text)`") }
        for flag in stop.flags {
            lines.append("- \(flag.message)\(flag.anchor.map { " at \($0.label): `\($0.text)`" } ?? "")")
        }
        lines.append("")
        lines.append("Read the change without checking it out or editing anything, and tell me what it does and what could go wrong. Cite file and line for each claim.")
        return lines.joined(separator: "\n")
    }
}

/// Guides on disk, one per pull request, beside the review drafts.
public struct ReviewGuideStore: Sendable {
    public let directory: URL

    public init(directory: URL) {
        self.directory = directory
    }

    private func file(_ repository: String, _ number: Int) -> URL {
        directory.appendingPathComponent("\(repository.replacingOccurrences(of: "/", with: "__"))__\(number).json")
    }

    public func load(repository: String, number: Int) -> ReviewGuide? {
        guard let data = try? Data(contentsOf: file(repository, number)) else { return nil }
        return try? JSONDecoder().decode(ReviewGuide.self, from: data)
    }

    public func save(_ guide: ReviewGuide, repository: String, number: Int) throws {
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        try JSONEncoder().encode(guide).write(to: file(repository, number), options: .atomic)
    }
}
