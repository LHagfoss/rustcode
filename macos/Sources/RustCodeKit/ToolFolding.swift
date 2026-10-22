import Foundation

/// How much of a turn's tool calls the chat shows, as the terminal's
/// `verbosity` setting does.
public enum ToolVerbosity: String, CaseIterable, Identifiable, Sendable {
    /// A run of calls folds into one `N tool calls` row; while tools run,
    /// only the current call shows.
    case low
    /// Every call is its own closed card.
    case high
    /// Like `low`, but file edits stay out of the runs with their diffs open.
    case edits

    public var id: String { rawValue }
    public static let key = "toolVerbosity"

    public var title: String {
        switch self {
        case .low: "Low"
        case .high: "High"
        case .edits: "Edits"
        }
    }

    /// Runs of calls fold (low and edits).
    public var folds: Bool { self != .high }
    /// File edits start open on their diff.
    public var editsOpen: Bool { self == .edits }

    /// The `verbosity = "…"` line of the terminal's `config.toml`, so the
    /// app starts where the terminal is set.
    public static func configured(in config: String) -> ToolVerbosity? {
        for line in config.split(separator: "\n") {
            let parts = line.split(separator: "=", maxSplits: 1)
            guard parts.count == 2, parts[0].trimmingCharacters(in: .whitespaces) == "verbosity" else { continue }
            let value = parts[1].trimmingCharacters(in: .whitespaces).trimmingCharacters(in: CharacterSet(charactersIn: "\""))
            return ToolVerbosity(rawValue: value)
        }
        return nil
    }
}

/// A folded run of tool calls, drawn on the row of its first call.
public struct ToolRunFold: Equatable, Sendable {
    /// The id of the run's first call, which names the run.
    public var run: Int
    public var count: Int
    public var failed: Int
    public var isOpen: Bool
}

/// Which tool cards a chat shows at a verbosity: rows stay one per item, so
/// a folded run hides its calls and its first row carries the summary.
public struct ToolFolding: Equatable, Sendable {
    /// Calls inside a closed run.
    public private(set) var hidden: Set<Int> = []
    /// The summary each run's first row draws.
    public private(set) var folds: [Int: ToolRunFold] = [:]

    public init() {}

    /// `open` holds the runs the reader opened. While `running`, the run at
    /// the end of the chat folds everything but its current call.
    public init(items: [TranscriptItem], verbosity: ToolVerbosity, open: Set<Int>, running: Bool) {
        guard verbosity.folds else { return }
        var run: [ToolItem] = []
        var ids: [Int] = []
        func close(atEnd: Bool) {
            defer { run = []; ids = [] }
            let live = atEnd && running
            let count = live ? run.count - 1 : run.count
            guard count >= (live ? 1 : 2), let first = ids.first else { return }
            let isOpen = open.contains(first)
            folds[first] = ToolRunFold(run: first, count: count,
                                       failed: run.prefix(count).filter { $0.status == .failed }.count,
                                       isOpen: isOpen)
            if !isOpen { hidden.formUnion(ids.prefix(count).dropFirst()) }
        }
        for item in items {
            guard case .tool(let tool) = item.content else {
                close(atEnd: false)
                continue
            }
            if verbosity.editsOpen && !tool.changes.isEmpty {
                close(atEnd: false)
                continue
            }
            run.append(tool)
            ids.append(item.id)
        }
        close(atEnd: true)
    }

    /// The summary on `item`'s row, if it starts a folded run.
    public func fold(for item: Int) -> ToolRunFold? { folds[item] }
}
