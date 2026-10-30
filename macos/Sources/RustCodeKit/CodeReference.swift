import Foundation

/// Lines picked in the Changes panel and attached to a message (#2643), so
/// the agent is pointed at exact code instead of a pasted snippet. Private to
/// the conversation: never a pull request comment, and never sent on its own.
public struct CodeReference: Identifiable, Equatable, Sendable {
    public enum Intent: String, Equatable, Sendable {
        /// A question about the lines.
        case ask
        /// A change the agent should make to them.
        case change
    }

    /// Which side of the diff the lines were picked on.
    public enum Side: String, Equatable, Sendable {
        /// Lines as they are now (added or unchanged).
        case new
        /// Lines the change removed, as they were in the base.
        case old
    }

    public var id = UUID()
    public var intent: Intent
    /// The repository root `path` is relative to.
    public var root: String
    /// Relative to the repository root, as the file is named now.
    public var path: String
    /// The file's old name, when it was renamed.
    public var oldPath: String?
    public var side: Side
    /// First and last line number on `side`, inclusive.
    public var lines: ClosedRange<Int>
    /// What was compared: "uncommitted changes" or "the branch since <base>".
    public var scope: String
    /// Short hash of the base commit, when known.
    public var baseCommit: String?
    /// The picked lines as they read when picked, so the reference still
    /// says what was meant after the file changes.
    public var excerpt: [String]

    public init(intent: Intent, root: String = "", path: String, oldPath: String? = nil, side: Side, lines: ClosedRange<Int>,
                scope: String, baseCommit: String?, excerpt: [String]) {
        self.intent = intent
        self.root = root
        self.path = path
        self.oldPath = oldPath
        self.side = side
        self.lines = lines
        self.scope = scope
        self.baseCommit = baseCommit
        self.excerpt = excerpt
    }

    /// `path:12-18`, the chip's label.
    public var label: String {
        let range = lines.lowerBound == lines.upperBound ? "\(lines.lowerBound)" : "\(lines.lowerBound)-\(lines.upperBound)"
        return "\(path):\(range)" + (side == .old ? " (removed)" : "")
    }

    /// A reference to the picked rows of a diff, or nil when the rows mix
    /// sides (removed and added lines together have no single range) or hold
    /// no line. Unchanged lines count as the new side.
    public static func from(rows: [DiffLine], intent: Intent, root: String = "", path: String, oldPath: String?,
                            scope: String, baseCommit: String?) -> CodeReference? {
        guard !rows.isEmpty else { return nil }
        let side: Side = rows.allSatisfy { $0.kind == .removed } ? .old : .new
        if side == .new, rows.contains(where: { $0.kind == .removed }) { return nil }
        let numbers = rows.compactMap { side == .old ? $0.oldNumber : $0.newNumber }
        guard numbers.count == rows.count, let first = numbers.min(), let last = numbers.max() else { return nil }
        return CodeReference(intent: intent, root: root, path: path, oldPath: oldPath, side: side, lines: first...last,
                             scope: scope, baseCommit: baseCommit, excerpt: rows.map(\.text))
    }

    /// Where the excerpt is in `file` now.
    public enum Location: Equatable, Sendable {
        /// Still at the picked lines.
        case unchanged
        /// Found whole at other lines.
        case moved(ClosedRange<Int>)
        /// Not in the file any more; the excerpt says what was picked.
        case changed
        /// The file is gone.
        case deleted
    }

    /// Check the excerpt against the file as it is now (`nil`: deleted).
    /// Lines removed by the change are history and always `unchanged`.
    public func locate(in file: String?) -> Location {
        guard side == .new else { return .unchanged }
        guard let file else { return .deleted }
        let current = file.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
        let start = lines.lowerBound - 1
        if start >= 0, start + excerpt.count <= current.count, Array(current[start..<start + excerpt.count]) == excerpt {
            return .unchanged
        }
        guard !excerpt.isEmpty, excerpt.count <= current.count else { return .changed }
        // The nearest whole match to where it was.
        let matches = (0...(current.count - excerpt.count)).filter { Array(current[$0..<$0 + excerpt.count]) == excerpt }
        guard let nearest = matches.min(by: { abs($0 - start) < abs($1 - start) }) else { return .changed }
        return .moved((nearest + 1)...(nearest + excerpt.count))
    }

    /// The reference as the message carries it, checked against the file on
    /// disk now.
    public func prompt() -> String {
        let url = URL(fileURLWithPath: root).appendingPathComponent(path)
        return prompt(location: locate(in: try? String(contentsOf: url, encoding: .utf8)))
    }

    /// The reference as the message carries it, after checking it against
    /// the file (`location`).
    public func prompt(location: Location) -> String {
        var range = lines
        var note: String?
        switch location {
        case .unchanged: break
        case .moved(let now):
            range = now
            note = "These lines moved since they were picked (they were at \(lines.lowerBound)-\(lines.upperBound))."
        case .changed:
            note = "The file changed since these lines were picked; the excerpt is what was picked."
        case .deleted:
            note = "The file has been deleted since these lines were picked."
        }
        let where_ = side == .old
            ? "lines \(range.lowerBound)-\(range.upperBound) removed from `\(oldPath ?? path)`"
            : "`\(path)` lines \(range.lowerBound)-\(range.upperBound)"
        let base = baseCommit.map { " against \($0)" } ?? ""
        let heading = switch intent {
        case .ask: "Question about \(where_) (\(scope)\(base)):"
        case .change: "Change requested for \(where_) (\(scope)\(base)):"
        }
        let fence = excerpt.contains { $0.contains("```") } ? "````" : "```"
        var text = heading + "\n" + fence + "\n" + excerpt.joined(separator: "\n") + "\n" + fence
        if let note { text += "\n" + note }
        return text
    }
}
