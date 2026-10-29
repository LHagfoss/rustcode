import Foundation

/// A text selection in the chat, held in the transcript's terms rather than
/// in any one text view's (#2440). Each finished run of prose is its own
/// text view, and the chat draws only the rows near the screen, so a view
/// can undraw while its text is selected. Kept here, the selection outlives
/// the view: a row drawn again restores its highlight from it, and a
/// selection that spans runs or messages can say what each one shows.
///
/// A place in the text is an item of the transcript, a run of prose within
/// it, and a UTF-16 offset into that run's string, as `NSRange` counts.
public struct TranscriptSelection: Equatable, Sendable {
    /// One run of prose: an item and the run's index within it. Runs are
    /// ordered as the chat shows them, item ids growing down the chat.
    public struct Run: Hashable, Comparable, Sendable {
        public var item: Int
        public var segment: Int

        public init(item: Int, segment: Int) {
            self.item = item
            self.segment = segment
        }

        public static func < (lhs: Run, rhs: Run) -> Bool {
            (lhs.item, lhs.segment) < (rhs.item, rhs.segment)
        }
    }

    /// A place between two UTF-16 code units of a run.
    public struct Position: Hashable, Comparable, Sendable {
        public var run: Run
        public var offset: Int

        public init(item: Int, segment: Int, offset: Int) {
            run = Run(item: item, segment: segment)
            self.offset = offset
        }

        public init(_ run: Run, offset: Int) {
            self.run = run
            self.offset = offset
        }

        public static func < (lhs: Position, rhs: Position) -> Bool {
            lhs.run != rhs.run ? lhs.run < rhs.run : lhs.offset < rhs.offset
        }
    }

    /// Where the selection began, and where it now ends. A drag upward has
    /// its focus before its anchor.
    public private(set) var anchor: Position?
    public private(set) var focus: Position?

    public init() {}

    public init(from anchor: Position, to focus: Position) {
        self.anchor = anchor
        self.focus = focus
    }

    /// Nothing is selected: no selection, or one that has not moved yet.
    public var isEmpty: Bool {
        guard let anchor, let focus else { return true }
        return anchor == focus
    }

    /// The selection from its first place to its last, whichever way it was made.
    public var bounds: (start: Position, end: Position)? {
        guard let anchor, let focus, anchor != focus else { return nil }
        return anchor < focus ? (anchor, focus) : (focus, anchor)
    }

    /// A press: a selection starts there, empty until it is extended.
    public mutating func begin(at position: Position) {
        anchor = position
        focus = position
    }

    /// A drag: the selection now ends at `position`. Without an anchor the
    /// drag starts there.
    public mutating func extend(to position: Position) {
        if anchor == nil { anchor = position }
        focus = position
    }

    public mutating func select(from anchor: Position, to focus: Position) {
        self.anchor = anchor
        self.focus = focus
    }

    public mutating func clear() {
        anchor = nil
        focus = nil
    }

    /// Whether any of `run` is selected.
    public func contains(_ run: Run) -> Bool {
        guard let bounds else { return false }
        return bounds.start.run <= run && run <= bounds.end.run
    }

    /// The part of a run of `length` UTF-16 units that shows selected, or
    /// nil when none does. A run inside the selection shows whole; the runs
    /// at its ends from or up to the selection's place, held to the run's
    /// length should its text have become shorter.
    public func range(in run: Run, length: Int) -> Range<Int>? {
        guard let bounds, contains(run) else { return nil }
        let lower = run == bounds.start.run ? min(max(bounds.start.offset, 0), length) : 0
        let upper = run == bounds.end.run ? min(max(bounds.end.offset, 0), length) : length
        return lower < upper ? lower..<upper : nil
    }

    /// What a copy puts on the pasteboard: the selected part of each run, in
    /// the chat's order, a blank line between them. `runs` are the runs the
    /// selection may cover with their text, in any order; runs it does not
    /// reach are left out.
    public func text(of runs: [(run: Run, text: String)]) -> String {
        runs.sorted { $0.run < $1.run }
            .compactMap { run, text -> String? in
                let units = Array(text.utf16)
                guard let range = range(in: run, length: units.count) else { return nil }
                return String(decoding: units[range], as: UTF16.self)
            }
            .joined(separator: "\n\n")
    }
}
