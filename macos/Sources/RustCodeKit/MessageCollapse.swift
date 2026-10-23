import Foundation

/// Long chat messages fold to their first lines behind Show more (#2303).
///
/// A user message or a finished reply taller than `maxLines` lines, and the
/// Show more control, shows its first `maxLines` lines under a fade. Only
/// the drawing changes: copy, the message menu, find and VoiceOver still get
/// the whole text. A reply never folds while it is written.
public enum MessageCollapse {
    /// The lines a folded message shows.
    public static let maxLines = 12
    /// Narrower than the chat ever is, in characters: a message that fits in
    /// `maxLines` lines this narrow fits in the chat, and is not measured.
    public static let approxColumns = 30
    /// The height of the Show more control under a folded message.
    public static let controlHeight: Double = 22

    /// Whether a message of this kind folds when it is long: what the reader
    /// wrote and finished replies, never a reply still being written.
    public static func folds(_ content: TranscriptItem.Content, isStreaming: Bool) -> Bool {
        switch content {
        case .user: true
        case .assistant: !isStreaming
        case .thinking, .tool, .command: false
        }
    }

    /// Whether `text` may run past `maxLines` lines: each line wraps every
    /// `approxColumns` characters. A quick test before the chat measures the
    /// message; false means it is short however the chat wraps it.
    public static func mayBeLong(_ text: String, maxLines: Int = maxLines, approxColumns: Int = approxColumns) -> Bool {
        let columns = max(approxColumns, 1)
        var lines = 0
        for line in text.split(separator: "\n", omittingEmptySubsequences: false) {
            lines += max(1, (line.count + columns - 1) / columns)
            if lines > maxLines { return true }
        }
        return false
    }

    /// The height a folded message shows.
    public static func foldedHeight(lineHeight: Double) -> Double {
        Double(maxLines) * lineHeight
    }

    /// The tallest a message is drawn whole. Folding one only a little
    /// taller would hide less than the control takes.
    public static func foldHeight(lineHeight: Double) -> Double {
        foldedHeight(lineHeight: lineHeight) + controlHeight
    }

    /// A message of this height, laid out in full, folds.
    public static func isLong(height: Double, lineHeight: Double) -> Bool {
        height > foldHeight(lineHeight: lineHeight)
    }
}

/// The long messages the reader opened, per chat (#2303). Kept by the app's
/// model rather than by the rows, so a message stays open when its row is
/// drawn again: the chat rebuilds rows and draws far ones as stand-ins.
public struct ExpandedMessages: Equatable, Sendable {
    private var open: [String: Set<Int>] = [:]

    public init() {}

    /// The open messages of `session`; nil is a new chat not saved yet.
    public func ids(in session: String?) -> Set<Int> {
        open[session ?? ""] ?? []
    }

    public func contains(_ id: Int, in session: String?) -> Bool {
        ids(in: session).contains(id)
    }

    /// Opens a folded message, or folds an open one.
    public mutating func toggle(_ id: Int, in session: String?) {
        if open[session ?? "", default: []].remove(id) == nil { open[session ?? "", default: []].insert(id) }
    }

    /// Opens `id`, as a reply watched while it was written and a find match
    /// are. Returns whether it was folded.
    @discardableResult
    public mutating func expand(_ id: Int, in session: String?) -> Bool {
        open[session ?? "", default: []].insert(id).inserted
    }
}
