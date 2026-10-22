import Foundation

/// One ACP `diff` item of a tool call: a file's text before and after an edit.
/// The engine sends one per hunk, with the hunk's first line numbers in
/// `_meta.rustcode`; other agents may send whole files.
public struct FileDiff: Equatable, Sendable {
    public var path: String
    /// Nil for a file the edit created.
    public var oldText: String?
    public var newText: String
    public var oldStart: Int
    public var newStart: Int

    public init(path: String, oldText: String?, newText: String, oldStart: Int = 1, newStart: Int = 1) {
        self.path = path
        self.oldText = oldText
        self.newText = newText
        self.oldStart = oldStart
        self.newStart = newStart
    }

    public init?(json: JSONValue) {
        guard json["type"]?.stringValue == "diff", let path = json["path"]?.stringValue,
              let newText = json["newText"]?.stringValue else { return nil }
        let meta = json["_meta"]?["rustcode"]
        self.init(
            path: path,
            oldText: json["oldText"]?.stringValue,
            newText: newText,
            oldStart: meta?["oldStart"]?.intValue ?? 1,
            newStart: meta?["newStart"]?.intValue ?? 1)
    }

    /// The `diff` items of a tool call's `content` array.
    public static func parse(content: JSONValue?) -> [FileDiff] {
        (content?.arrayValue ?? []).compactMap(FileDiff.init(json:))
    }
}

/// One line of a unified diff, numbered in the file it belongs to.
public struct DiffLine: Equatable, Sendable {
    public enum Kind: Equatable, Sendable { case context, added, removed }

    public var kind: Kind
    public var text: String
    public var oldNumber: Int?
    public var newNumber: Int?

    public init(kind: Kind, text: String, oldNumber: Int? = nil, newNumber: Int? = nil) {
        self.kind = kind
        self.text = text
        self.oldNumber = oldNumber
        self.newNumber = newNumber
    }

    /// The number the gutter shows, as GitHub's single column does: where a
    /// removed line was in the old file, else where the line is in the new
    /// one, so the numbers follow the file as it is now.
    public var number: Int? { kind == .removed ? oldNumber : newNumber }

    /// This line's gutter cell, right-aligned to `digits`.
    public func gutter(digits: Int) -> String {
        let text = number.map(String.init) ?? ""
        return String(repeating: " ", count: max(digits - text.count, 0)) + text
    }
}

/// A row of a rendered diff: a line, or unchanged lines left out (`nil` when
/// how many is unknown).
public enum DiffRow: Equatable, Sendable {
    case line(DiffLine)
    case gap(Int?)
}

/// Every change a tool call made to one file, as the card draws it.
public struct FileChange: Equatable, Sendable, Identifiable {
    public var path: String
    public var rows: [DiffRow]
    public var added: Int
    public var removed: Int
    public var isNewFile: Bool

    public var id: String { path }

    /// The width of the gutter: the widest number in the change, so the
    /// code does not shift between rows.
    public var gutterDigits: Int {
        rows.reduce(2) { result, row in
            guard case .line(let line) = row else { return result }
            return max(result, String(line.number ?? 0).count)
        }
    }

    /// Unchanged lines kept around each change.
    static let context = 3

    /// The diffs of one call, merged per file in the order they first appear.
    public static func group(_ diffs: [FileDiff]) -> [FileChange] {
        var order: [String] = []
        var byPath: [String: [FileDiff]] = [:]
        for diff in diffs {
            if byPath[diff.path] == nil { order.append(diff.path) }
            byPath[diff.path, default: []].append(diff)
        }
        return order.map { FileChange(path: $0, diffs: byPath[$0] ?? []) }
    }

    init(path: String, diffs: [FileDiff]) {
        self.path = path
        isNewFile = diffs.allSatisfy { $0.oldText == nil }
        var rows: [DiffRow] = []
        var previousOldEnd: Int?
        for diff in diffs.sorted(by: { $0.oldStart < $1.oldStart }) {
            let lines = LineDiff.lines(
                old: LineDiff.split(diff.oldText ?? ""), new: LineDiff.split(diff.newText),
                oldStart: diff.oldStart, newStart: diff.newStart)
            let hunkRows = Self.folded(lines)
            if let previousOldEnd, !hunkRows.isEmpty {
                let skipped = diff.oldStart - previousOldEnd
                if skipped > 0 { rows.append(.gap(skipped)) }
            }
            for row in hunkRows {
                if case .gap(let more) = row, case .gap(let count)? = rows.last {
                    rows[rows.count - 1] = .gap(count.flatMap { a in more.map { a + $0 } })
                } else {
                    rows.append(row)
                }
            }
            previousOldEnd = diff.oldStart + lines.filter { $0.kind != .added }.count
        }
        self.rows = rows
        added = rows.reduce(0) { if case .line(let line) = $1, line.kind == .added { $0 + 1 } else { $0 } }
        removed = rows.reduce(0) { if case .line(let line) = $1, line.kind == .removed { $0 + 1 } else { $0 } }
    }

    /// Long runs of unchanged lines shrink to the few beside a change.
    static func folded(_ lines: [DiffLine]) -> [DiffRow] {
        guard lines.contains(where: { $0.kind != .context }) else { return [] }
        var rows: [DiffRow] = []
        var index = 0
        while index < lines.count {
            guard lines[index].kind == .context else {
                rows.append(.line(lines[index]))
                index += 1
                continue
            }
            var end = index
            while end < lines.count, lines[end].kind == .context { end += 1 }
            let run = Array(lines[index..<end])
            let keepBefore = index == 0 ? 0 : context
            let keepAfter = end == lines.count ? 0 : context
            if run.count > keepBefore + keepAfter + 1 {
                rows += run.prefix(keepBefore).map(DiffRow.line)
                rows.append(.gap(run.count - keepBefore - keepAfter))
                rows += run.suffix(keepAfter).map(DiffRow.line)
            } else {
                rows += run.map(DiffRow.line)
            }
            index = end
        }
        return rows
    }

    /// The path under `root` without its prefix, else under the home folder
    /// as `~/…`, else as is.
    public func displayPath(relativeTo root: String?) -> String {
        if let root, !root.isEmpty {
            let base = root.hasSuffix("/") ? root : root + "/"
            if path.hasPrefix(base) { return String(path.dropFirst(base.count)) }
        }
        let home = NSHomeDirectory() + "/"
        if path.hasPrefix(home) { return "~/" + path.dropFirst(home.count) }
        return path
    }
}

/// A line-level diff: the longest common subsequence of the lines once their
/// shared head and tail are set aside. Inputs too large to compare cheaply
/// show as all removed then all added.
public enum LineDiff {
    /// Above this many line pairs the middle is not compared.
    static let maxCells = 4_000_000

    public static func split(_ text: String) -> [String] {
        guard !text.isEmpty else { return [] }
        var lines = text.components(separatedBy: "\n")
        if lines.last == "" { lines.removeLast() }
        return lines
    }

    public static func lines(old: [String], new: [String], oldStart: Int = 1, newStart: Int = 1) -> [DiffLine] {
        var head = 0
        while head < old.count, head < new.count, old[head] == new[head] { head += 1 }
        var tail = 0
        while tail < old.count - head, tail < new.count - head,
              old[old.count - 1 - tail] == new[new.count - 1 - tail] { tail += 1 }
        let oldMiddle = Array(old[head..<(old.count - tail)])
        let newMiddle = Array(new[head..<(new.count - tail)])

        var kinds: [DiffLine.Kind] = Array(repeating: .context, count: head)
        kinds += grouped(middle(oldMiddle, newMiddle), new: newMiddle)
        kinds += Array(repeating: .context, count: tail)

        var result: [DiffLine] = []
        result.reserveCapacity(kinds.count)
        var oldIndex = 0, newIndex = 0
        for kind in kinds {
            switch kind {
            case .context:
                result.append(DiffLine(kind: .context, text: new[newIndex],
                                       oldNumber: oldStart + oldIndex, newNumber: newStart + newIndex))
                oldIndex += 1
                newIndex += 1
            case .removed:
                result.append(DiffLine(kind: .removed, text: old[oldIndex], oldNumber: oldStart + oldIndex))
                oldIndex += 1
            case .added:
                result.append(DiffLine(kind: .added, text: new[newIndex], newNumber: newStart + newIndex))
                newIndex += 1
            }
        }
        return result
    }

    /// The edit script with a change that only a blank line or a lone brace
    /// split in two read as one: such a line matched between edits on both
    /// sides, at most as long as either, is replaced like its neighbours, and
    /// each changed run lists its removals before its additions. Without
    /// this, a `}` or blank line kept as context left a removal stranded after
    /// it, away from the additions that replaced it. Real context, and any
    /// run longer than the edits around it, stays.
    static func grouped(_ kinds: [DiffLine.Kind], new: [String]) -> [DiffLine.Kind] {
        // The script as runs: a context run with the new lines it covers, or
        // a changed run's counts.
        enum Run { case context(Range<Int>), change(removed: Int, added: Int) }
        var runs: [Run] = []
        var newIndex = 0
        for kind in kinds {
            switch (kind, runs.last) {
            case (.context, .context(let range)?):
                runs[runs.count - 1] = .context(range.lowerBound..<(range.upperBound + 1))
            case (.context, _):
                runs.append(.context(newIndex..<(newIndex + 1)))
            case (.removed, .change(let removed, let added)?):
                runs[runs.count - 1] = .change(removed: removed + 1, added: added)
            case (.added, .change(let removed, let added)?):
                runs[runs.count - 1] = .change(removed: removed, added: added + 1)
            case (.removed, _):
                runs.append(.change(removed: 1, added: 0))
            case (.added, _):
                runs.append(.change(removed: 0, added: 1))
            }
            if kind != .removed { newIndex += 1 }
        }
        var merged: [Run] = []
        var index = 0
        while index < runs.count {
            if case .context(let range) = runs[index], index > 0, index + 1 < runs.count,
               case .change(let removedBefore, let addedBefore)? = merged.last,
               case .change(let removedAfter, let addedAfter) = runs[index + 1],
               range.count <= max(removedBefore, addedBefore), range.count <= max(removedAfter, addedAfter),
               new[range].allSatisfy(isFiller) {
                merged[merged.count - 1] = .change(removed: removedBefore + range.count + removedAfter,
                                                   added: addedBefore + range.count + addedAfter)
                index += 2
                continue
            }
            merged.append(runs[index])
            index += 1
        }
        return merged.flatMap { run -> [DiffLine.Kind] in
            switch run {
            case .context(let range): Array(repeating: .context, count: range.count)
            case .change(let removed, let added):
                Array(repeating: .removed, count: removed) + Array(repeating: .added, count: added)
            }
        }
    }

    /// A line that says nothing on its own: blank, or only brackets and
    /// separators such as `}` or `});`.
    private static func isFiller(_ line: String) -> Bool {
        line.allSatisfy { $0.isWhitespace || "{}()[];,".contains($0) }
    }

    /// The edit script for the differing middle: removals before additions
    /// within each changed run, as a unified diff reads.
    private static func middle(_ old: [String], _ new: [String]) -> [DiffLine.Kind] {
        let n = old.count, m = new.count
        if n == 0 || m == 0 || n * m > maxCells {
            return Array(repeating: .removed, count: n) + Array(repeating: .added, count: m)
        }
        // lcs[i][j]: common lines of old[i...] and new[j...].
        let width = m + 1
        var lcs = [Int32](repeating: 0, count: (n + 1) * width)
        for i in stride(from: n - 1, through: 0, by: -1) {
            for j in stride(from: m - 1, through: 0, by: -1) {
                lcs[i * width + j] = old[i] == new[j]
                    ? lcs[(i + 1) * width + j + 1] + 1
                    : max(lcs[(i + 1) * width + j], lcs[i * width + j + 1])
            }
        }
        var kinds: [DiffLine.Kind] = []
        var i = 0, j = 0
        while i < n, j < m {
            if old[i] == new[j] {
                kinds.append(.context)
                i += 1
                j += 1
            } else if lcs[(i + 1) * width + j] >= lcs[i * width + j + 1] {
                kinds.append(.removed)
                i += 1
            } else {
                kinds.append(.added)
                j += 1
            }
        }
        kinds += Array(repeating: .removed, count: n - i)
        kinds += Array(repeating: .added, count: m - j)
        return kinds
    }
}
