import Foundation

/// What the Changes panel compares the working tree with (#2642).
public enum ChangesScope: Equatable, Hashable, Sendable {
    /// The last commit (`HEAD`) against the files on disk: staged, unstaged
    /// and untracked changes.
    case workingTree
    /// Where the branch left `base` (their merge base) against the files on
    /// disk: the branch's commits and anything not committed yet.
    case branch(base: String)
}

/// One file in the Changes panel.
public struct GitFileChange: Equatable, Sendable, Identifiable {
    public enum Status: String, Equatable, Sendable {
        case added, deleted, modified, renamed, copied, untracked
    }

    /// Relative to the repository root, as it is now (the new name of a rename).
    public var path: String
    /// The old name of a rename or copy.
    public var oldPath: String?
    public var status: Status
    public var isBinary: Bool
    public var rows: [DiffRow]
    public var added: Int
    public var removed: Int
    /// Diff lines left out to keep the panel bounded.
    public var omittedLines: Int

    public var id: String { path }

    public init(path: String, oldPath: String? = nil, status: Status, isBinary: Bool = false, rows: [DiffRow] = [],
                added: Int = 0, removed: Int = 0, omittedLines: Int = 0) {
        self.path = path
        self.oldPath = oldPath
        self.status = status
        self.isBinary = isBinary
        self.rows = rows
        self.added = added
        self.removed = removed
        self.omittedLines = omittedLines
    }

    /// The rows as the transcript's diff view draws them.
    public var fileChange: FileChange {
        FileChange(path: path, rows: rows, added: added, removed: removed,
                   isNewFile: status == .added || status == .untracked)
    }
}

/// Everything the panel shows for one load. A load never hides what it left
/// out: `omittedFiles`, `truncated` and each file's `omittedLines` say so.
public struct ChangesSnapshot: Equatable, Sendable {
    public var root: String
    public var scope: ChangesScope
    /// The current branch, nil when HEAD is detached.
    public var branch: String?
    /// Short hash of the commit compared against: HEAD, or the merge base.
    public var baseCommit: String?
    public var files: [GitFileChange]
    /// Untracked files beyond the bound, not listed.
    public var omittedFiles: Int
    /// The diff was cut at the size bound.
    public var truncated: Bool
    public var loadedAt: Date

    public init(root: String, scope: ChangesScope, branch: String?, baseCommit: String?, files: [GitFileChange],
                omittedFiles: Int = 0, truncated: Bool = false, loadedAt: Date = Date()) {
        self.root = root
        self.scope = scope
        self.branch = branch
        self.baseCommit = baseCommit
        self.files = files
        self.omittedFiles = omittedFiles
        self.truncated = truncated
        self.loadedAt = loadedAt
    }

    public var added: Int { files.reduce(0) { $0 + $1.added } }
    public var removed: Int { files.reduce(0) { $0 + $1.removed } }
}

public enum ChangesError: Error, Equatable, Sendable {
    /// The folder is not inside a Git repository.
    case notARepository
    /// The base branch or commit does not exist here.
    case unknownBase(String)
    /// Git failed; its own message.
    case git(String)

    public var message: String {
        switch self {
        case .notARepository: "This project is not a Git repository, so there are no changes to compare."
        case .unknownBase(let base): "“\(base)” is not a branch or commit in this repository."
        case .git(let message): message.isEmpty ? "Git could not compare the changes." : message
        }
    }
}

/// A `git diff` in unified format, read into files (#2642).
public enum UnifiedDiff {
    /// Lines kept per file; the rest are counted in `omittedLines`.
    public static let maxLinesPerFile = 3_000

    public static func parse(_ text: String, maxLinesPerFile: Int = maxLinesPerFile) -> [GitFileChange] {
        var files: [GitFileChange] = []
        var current: GitFileChange?
        var oldLine = 0
        var newLine = 0
        var previousHunkEnd: Int?
        var kept = 0

        func finish() {
            if let file = current { files.append(file) }
            current = nil
        }

        for raw in text.split(separator: "\n", omittingEmptySubsequences: false) {
            let line = String(raw)
            if line.hasPrefix("diff --git ") {
                finish()
                let (old, new) = gitPaths(line)
                current = GitFileChange(path: new ?? old ?? "", oldPath: nil, status: .modified)
                previousHunkEnd = nil
                kept = 0
                continue
            }
            guard var file = current else { continue }
            defer { current = file }
            if line.hasPrefix("@@") {
                guard let header = hunkHeader(line) else { continue }
                if let end = previousHunkEnd, header.oldStart > end {
                    file.rows.append(.gap(header.oldStart - end))
                } else if previousHunkEnd == nil, header.oldStart > 1 {
                    file.rows.append(.gap(header.oldStart - 1))
                }
                oldLine = header.oldStart
                newLine = header.newStart
                previousHunkEnd = header.oldStart + header.oldCount
                continue
            }
            if previousHunkEnd == nil {
                // Extended header lines, before the first hunk.
                if line.hasPrefix("new file mode") { file.status = .added }
                else if line.hasPrefix("deleted file mode") { file.status = .deleted }
                else if line.hasPrefix("rename from ") {
                    file.status = .renamed
                    file.oldPath = String(line.dropFirst("rename from ".count))
                } else if line.hasPrefix("rename to ") {
                    file.path = String(line.dropFirst("rename to ".count))
                } else if line.hasPrefix("copy from ") {
                    file.status = .copied
                    file.oldPath = String(line.dropFirst("copy from ".count))
                } else if line.hasPrefix("copy to ") {
                    file.path = String(line.dropFirst("copy to ".count))
                } else if line.hasPrefix("Binary files ") || line == "GIT binary patch" {
                    file.isBinary = true
                } else if line.hasPrefix("+++ "), let path = headerPath(line) {
                    file.path = path
                }
                continue
            }
            let kind: DiffLine.Kind
            switch line.first {
            case "+": kind = .added
            case "-": kind = .removed
            case " ": kind = .context
            default: continue // "\ No newline at end of file", or the trailing empty line
            }
            switch kind {
            case .added: file.added += 1
            case .removed: file.removed += 1
            case .context: break
            }
            if kept < maxLinesPerFile {
                file.rows.append(.line(DiffLine(kind: kind, text: String(line.dropFirst()),
                                                oldNumber: kind == .added ? nil : oldLine,
                                                newNumber: kind == .removed ? nil : newLine)))
                kept += 1
            } else {
                file.omittedLines += 1
            }
            if kind != .added { oldLine += 1 }
            if kind != .removed { newLine += 1 }
        }
        finish()
        return files
    }

    /// `@@ -12,3 +12,4 @@ …` as its starts and the old side's length.
    static func hunkHeader(_ line: String) -> (oldStart: Int, oldCount: Int, newStart: Int)? {
        let parts = line.split(separator: " ")
        guard parts.count >= 3, parts[1].hasPrefix("-"), parts[2].hasPrefix("+") else { return nil }
        func range(_ part: Substring) -> (Int, Int)? {
            let numbers = part.dropFirst().split(separator: ",")
            guard let start = numbers.first.flatMap({ Int($0) }) else { return nil }
            return (start, numbers.count > 1 ? Int(numbers[1]) ?? 1 : 1)
        }
        guard let old = range(parts[1]), let new = range(parts[2]) else { return nil }
        // An empty side starts at 0; its first line is 1.
        return (max(old.0, 1), old.1, max(new.0, 1))
    }

    /// The path in a `--- a/x` or `+++ b/x` line; nil for `/dev/null`.
    static func headerPath(_ line: String) -> String? {
        let path = String(line.dropFirst(4))
        if path == "/dev/null" { return nil }
        return path.hasPrefix("a/") || path.hasPrefix("b/") ? String(path.dropFirst(2)) : path
    }

    /// Both paths of `diff --git a/x b/x`. With spaces in a path the split is
    /// ambiguous; the `+++` or `rename to` line that follows corrects it.
    static func gitPaths(_ line: String) -> (String?, String?) {
        let rest = line.dropFirst("diff --git ".count)
        guard rest.hasPrefix("a/"), let split = rest.range(of: " b/", options: .backwards) else { return (nil, nil) }
        return (String(rest[rest.index(rest.startIndex, offsetBy: 2)..<split.lowerBound]),
                String(rest[split.upperBound...]))
    }
}

/// Reads a repository's changes with the `git` command line, read-only: it
/// never stages, checks out, stashes or writes anything (#2642).
public struct GitChangesLoader: Sendable {
    /// Bytes of diff read before the rest is cut, and untracked files listed.
    public static let maxDiffBytes = 4 << 20
    public static let maxUntracked = 300
    static let maxUntrackedBytes = 256 << 10
    static let emptyTree = "4b825dc642cb6eb9a060e54bf8d69288fbee4904"

    public var git: String

    public init(git: String = "/usr/bin/git") {
        self.git = git
    }

    public func load(folder: String, scope: ChangesScope) async throws -> ChangesSnapshot {
        guard let root = try? run(["rev-parse", "--show-toplevel"], in: folder).trimmed, !root.isEmpty else {
            throw ChangesError.notARepository
        }
        let branch = (try? run(["symbolic-ref", "--quiet", "--short", "HEAD"], in: root))?.trimmed
        let head = (try? run(["rev-parse", "--verify", "--quiet", "HEAD"], in: root))?.trimmed
        let base: String
        switch scope {
        case .workingTree:
            base = head?.isEmpty == false ? head! : Self.emptyTree
        case .branch(let name):
            guard (try? run(["rev-parse", "--verify", "--quiet", name + "^{commit}"], in: root)) != nil else {
                throw ChangesError.unknownBase(name)
            }
            guard let head, !head.isEmpty else { throw ChangesError.unknownBase(name) }
            base = try run(["merge-base", name, head], in: root).trimmed
        }
        let (diff, truncated) = try runBounded(
            ["-c", "core.quotePath=false", "diff", "--no-color", "--no-ext-diff", "-M", "--src-prefix=a/", "--dst-prefix=b/", base],
            in: root, limit: Self.maxDiffBytes)
        var files = UnifiedDiff.parse(diff)
        let untracked = (try run(["-c", "core.quotePath=false", "ls-files", "--others", "--exclude-standard", "-z"], in: root))
            .split(separator: "\0").map(String.init)
        for path in untracked.prefix(Self.maxUntracked) {
            files.append(Self.untrackedFile(path, root: root))
        }
        files.sort { $0.path < $1.path }
        return ChangesSnapshot(root: root, scope: scope, branch: branch?.isEmpty == false ? branch : nil,
                               baseCommit: base == Self.emptyTree ? nil : String(base.prefix(8)), files: files,
                               omittedFiles: max(untracked.count - Self.maxUntracked, 0), truncated: truncated)
    }

    /// Branches a base can be picked from, local first, then remote.
    public func branches(folder: String) -> [String] {
        let output = (try? run(["for-each-ref", "--format=%(refname:short)", "refs/heads", "refs/remotes"], in: folder)) ?? ""
        return output.split(separator: "\n").map(String.init).filter { !$0.hasSuffix("/HEAD") }
    }

    /// The branch the repository treats as its default: `origin/HEAD`'s
    /// target, else `main` or `master` when one exists.
    public func defaultBase(folder: String) -> String? {
        if let remote = (try? run(["symbolic-ref", "--quiet", "--short", "refs/remotes/origin/HEAD"], in: folder))?.trimmed,
           !remote.isEmpty {
            return remote
        }
        let names = branches(folder: folder)
        return ["main", "master", "origin/main", "origin/master"].first(where: names.contains)
    }

    /// An untracked file as all added lines, or binary or too big to show.
    static func untrackedFile(_ path: String, root: String) -> GitFileChange {
        let url = URL(fileURLWithPath: root).appendingPathComponent(path)
        guard let handle = try? FileHandle(forReadingFrom: url) else {
            return GitFileChange(path: path, status: .untracked)
        }
        defer { try? handle.close() }
        let data = (try? handle.read(upToCount: maxUntrackedBytes + 1)) ?? Data()
        if data.contains(0) { return GitFileChange(path: path, status: .untracked, isBinary: true) }
        let text = String(decoding: data.prefix(maxUntrackedBytes), as: UTF8.self)
        var lines = text.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
        if lines.last == "" { lines.removeLast() }
        let kept = lines.prefix(UnifiedDiff.maxLinesPerFile)
        let rows = kept.enumerated().map { DiffRow.line(DiffLine(kind: .added, text: $1, newNumber: $0 + 1)) }
        let omitted = lines.count - kept.count + (data.count > maxUntrackedBytes ? 1 : 0)
        return GitFileChange(path: path, status: .untracked, rows: rows, added: lines.count, omittedLines: omitted)
    }

    func run(_ arguments: [String], in folder: String) throws -> String {
        let (output, _) = try runBounded(arguments, in: folder, limit: 16 << 20)
        return output
    }

    /// Run git with `GIT_OPTIONAL_LOCKS=0`, so reading never takes the index
    /// lock from a commit running beside it, and stop reading at `limit`.
    func runBounded(_ arguments: [String], in folder: String, limit: Int) throws -> (String, truncated: Bool) {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: git)
        process.arguments = arguments
        process.currentDirectoryURL = URL(fileURLWithPath: folder)
        var environment = ProcessInfo.processInfo.environment
        environment["GIT_OPTIONAL_LOCKS"] = "0"
        environment["GIT_TERMINAL_PROMPT"] = "0"
        environment["LC_ALL"] = "C"
        process.environment = environment
        let out = Pipe()
        let err = Pipe()
        process.standardOutput = out
        process.standardError = err
        process.standardInput = FileHandle.nullDevice
        try process.run()
        var data = Data()
        var truncated = false
        while let chunk = try out.fileHandleForReading.read(upToCount: 64 << 10), !chunk.isEmpty {
            if data.count + chunk.count > limit {
                data.append(chunk.prefix(limit - data.count))
                truncated = true
                process.terminate()
                break
            }
            data.append(chunk)
        }
        let errors = String(decoding: err.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
        process.waitUntilExit()
        if !truncated, process.terminationStatus != 0 {
            throw ChangesError.git(errors.trimmed)
        }
        if truncated, let cut = data.lastIndex(of: UInt8(ascii: "\n")) {
            data = data.prefix(through: cut)
        }
        return (String(decoding: data, as: UTF8.self), truncated)
    }
}

private extension String {
    var trimmed: String { trimmingCharacters(in: .whitespacesAndNewlines) }
}
