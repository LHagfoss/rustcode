import Foundation

/// The sidebar's chats, read from `session/list` a page at a time (#2189).
///
/// Reading every page up front took over a second with a thousand chats, and
/// ran again after every turn. The sidebar now starts with one page and asks
/// for the next as the reader scrolls near the end. A refresh reads again
/// from the start, but as far as the pages already shown, so no row the
/// reader saw disappears. The engine pages in a stable order, by when each
/// chat started, so a cursor stays good while chats are updated.
public struct SessionPages: Equatable, Sendable {
    /// The chats listed so far, in the engine's order, each once.
    public private(set) var listed: [SessionSummary] = []
    /// Where the next page starts; nil once the listing is complete.
    public private(set) var cursor: String?
    /// The listing has been started at all.
    public private(set) var isStarted = false

    /// Fetch the next page once a row this close to the end appears.
    public static let fetchMoreThreshold = 10
    /// The engine's page size (`ACP_SESSION_PAGE_SIZE`), for planning only:
    /// a page may come back shorter, as rows are filtered after the cap.
    public static let pageSize = 50
    /// Pages a refresh reads at most, so a vast store cannot stall it.
    public static let maximumRefreshPages = 40

    public init() {}

    public var hasMore: Bool { cursor != nil }

    /// Start the listing again from its first page.
    public mutating func reset() {
        listed = []
        cursor = nil
        isStarted = false
    }

    /// A page arrived. Rows already listed are skipped. Returns how many rows
    /// were new.
    @discardableResult
    public mutating func append(_ page: [SessionSummary], next: String?) -> Int {
        var known = Set(listed.map(\.id))
        let fresh = page.filter { known.insert($0.id).inserted }
        listed += fresh
        cursor = next
        isStarted = true
        return fresh.count
    }

    /// Whether a refresh that has read `self` so far should read another page
    /// to show at least `keep` rows, as many as were shown before it.
    public func refreshWantsMore(keeping keep: Int, pagesRead: Int) -> Bool {
        !isStarted || (hasMore && pagesRead < Self.maximumRefreshPages && listed.count < keep)
    }

    /// A chat left the store, such as one moved to the Trash.
    public mutating func remove(_ id: String) {
        listed.removeAll { $0.id == id }
    }

    /// Whether showing the row at `index` of `count` rows should fetch more.
    public static func isNearEnd(_ index: Int, of count: Int) -> Bool {
        index >= count - fetchMoreThreshold
    }

    /// Pinned chats whose page has not been read yet, from what was saved
    /// of them, so they stay at the top whichever pages are in. Only chats
    /// in `scope` (a project path; nil for every project) and not listed.
    /// Once every page is read, a pinned chat not among them is not in this
    /// listing at all.
    public func unlistedPinned(_ saved: [SessionSummary], pinned: Set<String>, scope: String?) -> [SessionSummary] {
        guard hasMore else { return [] }
        let known = Set(listed.map(\.id))
        return saved.filter { pinned.contains($0.id) && !known.contains($0.id) && (scope == nil || $0.cwd == scope) }
    }
}
