import Foundation

/// How much of a long chat the transcript lays out (#2190).
///
/// A chat opens with its newest `initialCount` items; older ones join a page
/// at a time as the reader scrolls near the top of what is shown. Even
/// lazily, every item in the chat's stack cost the scroll view bookkeeping on
/// each scrolled frame and each streamed chunk: a 3,000-item chat scrolled at
/// 30 ms a frame where a 300-item one took 19. Items only ever join the
/// window, at its top, so nothing the reader has seen leaves it; output
/// streaming in joins at the bottom as usual. Find and Home widen it to reach
/// what they go to.
public struct TranscriptWindow: Equatable, Sendable {
    /// Items a chat opens with.
    public static let initialCount = 150
    /// Items each step back adds.
    public static let pageCount = 100
    /// Items kept above a find match brought into the window, so it does
    /// not land on the window's top edge and load more at once.
    public static let revealMargin = 10

    /// The chat the window belongs to (`Transcript.identity`) and the index
    /// of its first item shown.
    private var identity: UUID?
    private var start = 0

    public init() {}

    /// The index of the first item shown of a chat with `count` items. A
    /// chat the window has not seen yet opens on its newest items.
    public func firstShown(identity: UUID, count: Int) -> Int {
        guard identity == self.identity else { return Self.opening(count) }
        return min(start, count)
    }

    static func opening(_ count: Int) -> Int { max(count - initialCount, 0) }

    /// Fix the window of a chat as it is first shown, so items arriving
    /// later grow it at the bottom rather than slide it down.
    public mutating func open(identity: UUID, count: Int) {
        guard identity != self.identity else { return }
        self.identity = identity
        start = Self.opening(count)
    }

    /// Older items exist above the window.
    public func hasEarlier(identity: UUID, count: Int) -> Bool {
        firstShown(identity: identity, count: count) > 0
    }

    /// Add the page before the window. Returns whether anything was added.
    @discardableResult
    public mutating func showEarlier(identity: UUID, count: Int) -> Bool {
        let first = firstShown(identity: identity, count: count)
        guard first > 0 else { return false }
        self.identity = identity
        start = max(first - Self.pageCount, 0)
        return true
    }

    /// Make sure the item at `index` is shown. Returns whether the window grew.
    @discardableResult
    public mutating func reveal(_ index: Int, identity: UUID, count: Int) -> Bool {
        let first = firstShown(identity: identity, count: count)
        self.identity = identity
        start = first
        guard index < first else { return false }
        start = max(index - Self.revealMargin, 0)
        return true
    }

    /// Show the whole chat, for Home. Returns whether the window grew.
    @discardableResult
    public mutating func showAll(identity: UUID, count: Int) -> Bool {
        reveal(0, identity: identity, count: count)
    }
}
