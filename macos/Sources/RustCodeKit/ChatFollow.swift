import Foundation

/// Whether the chat keeps its newest output in view, and whether the Latest
/// button shows. The view feeds it the scroll geometry it measures and the
/// reader's own scroll input; everything else is decided here.
///
/// The button follows the measured distance from the end, not recent input:
/// it shows only while the chat is let go and meaningfully short of its end,
/// so reaching the end, however that happens, hides it.
public struct ChatFollow: Equatable, Sendable {
    /// How close to the end still counts as being at the newest output.
    public static let threshold: Double = 24

    /// What the chat's scroll view measures.
    public struct Geometry: Equatable, Sendable {
        /// The content offset; smaller is further up the chat.
        public var offset: Double
        public var containerHeight: Double
        /// How much further the chat can scroll down.
        public var distanceFromBottom: Double
        /// The content is taller than the chat, so it scrolls at all.
        public var canScroll: Bool
        /// The scroll view's content insets, under the title bar and composer.
        public var insetTop: Double
        public var insetBottom: Double

        public init(offset: Double, containerHeight: Double, distanceFromBottom: Double, canScroll: Bool,
                    insetTop: Double = 0, insetBottom: Double = 0) {
            self.offset = offset
            self.containerHeight = containerHeight
            self.distanceFromBottom = distanceFromBottom
            self.canScroll = canScroll
            self.insetTop = insetTop
            self.insetBottom = insetBottom
        }

        public var isAtEnd: Bool { !canScroll || distanceFromBottom <= ChatFollow.threshold }
    }

    /// The chat stays anchored to its end as output grows.
    public private(set) var follows = true
    /// The last measured geometry; nil until the chat is first laid out.
    public private(set) var geometry: Geometry?

    public init() {}

    /// Latest shows only when it has somewhere to go.
    public var showsLatest: Bool { !follows && !(geometry?.isAtEnd ?? true) }

    /// The scroll geometry changed. `readerScrolling` is whether the reader's
    /// own wheel, trackpad or scroller input is in progress. Returns true when
    /// the chat should scroll to its end: it follows but sits short of it,
    /// because output grew or rows below took their real height late.
    /// `towardEnd` is whether the reader's latest input points down the chat.
    public mutating func update(_ new: Geometry, readerScrolling: Bool, towardEnd: Bool = false) -> Bool {
        let old = geometry
        geometry = new
        // A resize moves the offset without anyone scrolling.
        let sameSize = old.map { new.containerHeight == $0.containerHeight } ?? false
        let movedUp = sameSize && new.offset < old!.offset - 0.5
        let movedDown = sameSize && new.offset > old!.offset + 0.5
        if readerScrolling {
            // The reader moving the chat up lets go, also in the first frames
            // still within the threshold, so output cannot pull it back down.
            // Moving up from a scroll down is the bounce at the end, not the
            // reader leaving it; letting go there fought the pull to the end
            // while a reply streamed.
            if movedUp, !towardEnd {
                follows = false
                return false
            }
            // While the reader scrolls, only their own way down to the end
            // holds on again. Output arriving between their scroll events
            // changes the geometry too, and while the end was still held it
            // even moves the chat down; taking that for reaching the end
            // pulled a reader just leaving the end back to it on every
            // token, so the chat fought the scroll.
            if new.isAtEnd, (movedDown && towardEnd) || follows { follows = true }
            return false
        }
        // At the end means following, however the chat got there.
        if new.isAtEnd {
            follows = true
            return false
        }
        return follows
    }

    /// A wheel or trackpad scroll toward the top, over the chat. Lets go at
    /// once, so output arriving in the same frame cannot pull the chat back.
    public mutating func readerScrolledUp() {
        if geometry?.canScroll ?? false { follows = false }
    }

    /// A scroll came to rest, after momentum or the bounce at the end.
    public mutating func scrollSettled() {
        if geometry?.isAtEnd ?? true { follows = true }
    }

    /// The transcript changed. A chat left resting at its end follows again,
    /// so output never piles up below a reader who is already at the bottom.
    /// Returns whether the chat should scroll to its end.
    public mutating func transcriptChanged(readerScrolling: Bool) -> Bool {
        if !follows, !readerScrolling, geometry?.isAtEnd ?? true { follows = true }
        return follows
    }

    /// Latest was pressed, a message was sent, or another chat opened.
    public mutating func jumpToLatest() {
        follows = true
    }

    /// The chat moved somewhere else on purpose, such as a find match.
    public mutating func letGo() {
        follows = false
    }
}

/// Where a reply still being written splits into text that has settled and
/// the tail that is still fading in.
public enum StreamingSplit {
    /// The number of leading characters to show as settled, plain text: up to
    /// the last line break at or before `settled`. The tail starts after that
    /// break, so the two pieces stacked read as the whole text. Zero when
    /// there is no such break.
    ///
    /// The fading tail is drawn by a custom text renderer into one layer, and
    /// a layer much taller than the window draws nothing; drawing the whole
    /// of a long reply that way blanked the chat while it streamed.
    public static func settledLength(of characters: some BidirectionalCollection<Character>, settled: Int) -> Int {
        let limit = min(max(settled, 0), characters.count)
        guard limit > 0 else { return 0 }
        var index = characters.index(characters.startIndex, offsetBy: limit)
        var offset = limit
        // The break that ends a line, not one of a run of blank lines after it.
        while index > characters.startIndex {
            let previous = characters.index(before: index)
            offset -= 1
            if characters[previous] == "\n" || characters[previous] == "\r\n" {
                if previous == characters.startIndex { return 0 }
                let before = characters.index(before: previous)
                if characters[before] != "\n", characters[before] != "\r\n" { return offset }
            }
            index = previous
        }
        return 0
    }
}
