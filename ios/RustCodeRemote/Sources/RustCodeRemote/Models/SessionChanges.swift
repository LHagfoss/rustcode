import Foundation

/// A session's uncommitted changes as the host read them for review (`get_changes`). A
/// snapshot: `capturedAt` says how fresh it is, and nothing here was changed to read it.
public struct SessionChanges: Sendable, Equatable {
    public struct File: Sendable, Equatable, Identifiable {
        public enum Status: String, Sendable, Equatable {
            case added, modified, deleted, renamed, untracked, other
        }

        public var path: String
        public var oldPath: String?
        public var status: Status
        public var additions: Int
        public var deletions: Int
        public var isBinary: Bool
        /// The unified diff, absent for a binary file or when the host left it out.
        public var diff: String?
        /// The diff was cut, or left out, to keep the answer bounded.
        public var diffTruncated: Bool

        public var id: String { path }

        public init(path: String, oldPath: String? = nil, status: Status, additions: Int, deletions: Int,
                    isBinary: Bool = false, diff: String? = nil, diffTruncated: Bool = false) {
            self.path = path
            self.oldPath = oldPath
            self.status = status
            self.additions = additions
            self.deletions = deletions
            self.isBinary = isBinary
            self.diff = diff
            self.diffTruncated = diffTruncated
        }

        /// The diff's lines with their numbers in the new file, for display and selection.
        public var lines: [DiffLine] { DiffLine.parse(diff ?? "") }
    }

    /// One line of a diff, numbered as the file is now (or was, for a removed line).
    public struct DiffLine: Sendable, Equatable, Identifiable {
        public enum Kind: Sendable, Equatable { case hunk, context, added, removed }

        public var id: Int
        public var kind: Kind
        public var text: String
        public var number: Int?

        static func parse(_ diff: String) -> [DiffLine] {
            var out: [DiffLine] = []
            var old = 0
            var new = 0
            var started = false
            for line in diff.split(separator: "\n", omittingEmptySubsequences: false) {
                let text = String(line)
                if text.hasPrefix("@@") {
                    started = true
                    // "@@ -a,b +c,d @@": the numbers the hunk starts at.
                    let parts = text.split(separator: " ")
                    old = parts.count > 1 ? Int(parts[1].dropFirst().split(separator: ",")[0]) ?? 0 : 0
                    new = parts.count > 2 ? Int(parts[2].dropFirst().split(separator: ",")[0]) ?? 0 : 0
                    out.append(DiffLine(id: out.count, kind: .hunk, text: text, number: nil))
                    continue
                }
                guard started, !text.hasPrefix("\\") else { continue }
                if text.hasPrefix("+") {
                    out.append(DiffLine(id: out.count, kind: .added, text: String(text.dropFirst()), number: new))
                    new += 1
                } else if text.hasPrefix("-") {
                    out.append(DiffLine(id: out.count, kind: .removed, text: String(text.dropFirst()), number: old))
                    old += 1
                } else if text.hasPrefix(" ") {
                    out.append(DiffLine(id: out.count, kind: .context, text: String(text.dropFirst()), number: new))
                    old += 1
                    new += 1
                }
            }
            return out
        }
    }

    public var capturedAt: Date?
    public var workspace: String
    /// False when there was nothing to read; `reason` says why.
    public var isAvailable: Bool
    public var reason: String?
    public var head: String?
    public var branch: String?
    public var files: [File]
    /// Changed files the host did not list.
    public var omittedFiles: Int

    public init(capturedAt: Date?, workspace: String, isAvailable: Bool, reason: String? = nil, head: String? = nil,
                branch: String? = nil, files: [File], omittedFiles: Int = 0) {
        self.capturedAt = capturedAt
        self.workspace = workspace
        self.isAvailable = isAvailable
        self.reason = reason
        self.head = head
        self.branch = branch
        self.files = files
        self.omittedFiles = omittedFiles
    }

    public var additions: Int { files.reduce(0) { $0 + $1.additions } }
    public var deletions: Int { files.reduce(0) { $0 + $1.deletions } }

    /// How old the snapshot is, in words, as of `now`.
    public func freshness(now: Date = Date()) -> String {
        guard let capturedAt else { return "Read just now" }
        let seconds = Int(now.timeIntervalSince(capturedAt))
        if seconds < 60 { return "Read just now" }
        if seconds < 3600 { return "Read \(seconds / 60) min ago" }
        return "Read \(seconds / 3600) h ago"
    }

    /// Whether the snapshot is old enough that it is worth reading again.
    public func isStale(now: Date = Date()) -> Bool {
        guard let capturedAt else { return false }
        return now.timeIntervalSince(capturedAt) > 300
    }

    /// A follow-up about `lines` of `file`, quoted with their numbers, for the composer. It is
    /// a draft: nothing is sent until the user sends it.
    public static func followUp(file: File, lines: [DiffLine]) -> String {
        let numbered = lines.filter { $0.kind != .hunk }
        let first = numbered.compactMap(\.number).min()
        let last = numbered.compactMap(\.number).max()
        let range = first.map { first in last.map { $0 == first ? "line \(first)" : "lines \(first)–\($0)" } ?? "line \(first)" } ?? "the change"
        let quoted = numbered.map { line in
            let mark = line.kind == .added ? "+" : line.kind == .removed ? "-" : " "
            return "\(mark) \(line.text)"
        }.joined(separator: "\n")
        return "About \(file.path), \(range):\n```diff\n\(quoted)\n```\n"
    }
}
