import XCTest
@testable import RustCodeKit

final class SessionPagesTests: XCTestCase {
    private func chats(_ range: Range<Int>, cwd: String = "/p") -> [SessionSummary] {
        range.map { SessionSummary(id: "s\($0)", cwd: cwd, title: "Chat \($0)") }
    }

    func testPagesAppendInOrder() {
        var pages = SessionPages()
        XCTAssertFalse(pages.isStarted)
        XCTAssertEqual(pages.append(chats(0..<50), next: "s49"), 50)
        XCTAssertTrue(pages.hasMore)
        XCTAssertEqual(pages.append(chats(50..<80), next: nil), 30)
        XCTAssertFalse(pages.hasMore)
        XCTAssertEqual(pages.listed.map(\.id), (0..<80).map { "s\($0)" })
    }

    /// A chat listed twice, such as one added while paging, keeps one row.
    func testDuplicateRowsAreSkipped() {
        var pages = SessionPages()
        pages.append(chats(0..<50), next: "s49")
        XCTAssertEqual(pages.append(chats(45..<60), next: "s59"), 10)
        XCTAssertEqual(pages.listed.count, 60)
        XCTAssertEqual(Set(pages.listed.map(\.id)).count, 60)
        // A page of only known rows still moves the cursor on.
        XCTAssertEqual(pages.append(chats(50..<60), next: "s69"), 0)
        XCTAssertEqual(pages.cursor, "s69")
    }

    /// The first refresh reads one page; later ones read as far as was shown.
    func testRefreshReadsAsFarAsTheSidebarHad() {
        var pages = SessionPages()
        XCTAssertTrue(pages.refreshWantsMore(keeping: 0, pagesRead: 0))
        pages.append(chats(0..<50), next: "s49")
        XCTAssertFalse(pages.refreshWantsMore(keeping: 0, pagesRead: 1), "initial page only")
        XCTAssertTrue(pages.refreshWantsMore(keeping: 120, pagesRead: 1))
        pages.append(chats(50..<100), next: "s99")
        XCTAssertTrue(pages.refreshWantsMore(keeping: 120, pagesRead: 2))
        pages.append(chats(100..<150), next: "s149")
        XCTAssertFalse(pages.refreshWantsMore(keeping: 120, pagesRead: 3))
    }

    func testRefreshStopsAtTheEndAndAtTheBound() {
        var pages = SessionPages()
        pages.append(chats(0..<10), next: nil)
        XCTAssertFalse(pages.refreshWantsMore(keeping: 500, pagesRead: 1))
        var endless = SessionPages()
        endless.append(chats(0..<1), next: "s0")
        XCTAssertFalse(endless.refreshWantsMore(keeping: 10_000, pagesRead: SessionPages.maximumRefreshPages))
    }

    func testResetStartsOver() {
        var pages = SessionPages()
        pages.append(chats(0..<50), next: "s49")
        pages.reset()
        XCTAssertEqual(pages, SessionPages())
        XCTAssertTrue(pages.refreshWantsMore(keeping: 0, pagesRead: 0))
    }

    func testRemoveDropsATrashedChat() {
        var pages = SessionPages()
        pages.append(chats(0..<3), next: nil)
        pages.remove("s1")
        XCTAssertEqual(pages.listed.map(\.id), ["s0", "s2"])
    }

    func testNearEndUsesTheThreshold() {
        let threshold = SessionPages.fetchMoreThreshold
        XCTAssertFalse(SessionPages.isNearEnd(100 - threshold - 1, of: 100))
        XCTAssertTrue(SessionPages.isNearEnd(100 - threshold, of: 100))
        XCTAssertTrue(SessionPages.isNearEnd(99, of: 100))
        // A list shorter than the threshold is near its end from the start.
        XCTAssertTrue(SessionPages.isNearEnd(0, of: 3))
    }

    /// A pinned chat on a page not read yet still shows, from what was saved.
    func testUnreadPinnedChatsComeFromTheSavedRows() {
        var pages = SessionPages()
        pages.append(chats(0..<50), next: "s49")
        let saved = [SessionSummary(id: "s900", cwd: "/p", title: "Old pin"),
                     SessionSummary(id: "s901", cwd: "/other", title: "Other project"),
                     SessionSummary(id: "s3", cwd: "/p", title: "Listed pin"),
                     SessionSummary(id: "s902", cwd: "/p", title: "No longer pinned")]
        let pinned: Set = ["s900", "s901", "s3"]
        XCTAssertEqual(pages.unlistedPinned(saved, pinned: pinned, scope: nil).map(\.id), ["s900", "s901"])
        XCTAssertEqual(pages.unlistedPinned(saved, pinned: pinned, scope: "/p").map(\.id), ["s900"])
    }

    /// With every page read, a pinned chat not listed is not in this scope.
    func testCompleteListingAddsNoUnlistedPins() {
        var pages = SessionPages()
        pages.append(chats(0..<5), next: nil)
        let saved = [SessionSummary(id: "s900", cwd: "/p")]
        XCTAssertEqual(pages.unlistedPinned(saved, pinned: ["s900"], scope: "/p"), [])
    }

    func testSummariesRoundTripForThePinnedCache() throws {
        let summary = SessionSummary(id: "s1", cwd: "/p", title: "T", updatedAt: Date(timeIntervalSince1970: 1_000), model: "a/b")
        let data = try JSONEncoder().encode([summary])
        XCTAssertEqual(try JSONDecoder().decode([SessionSummary].self, from: data), [summary])
    }

    /// A thousand chats read page by page come out whole, once each, in order.
    func testLargeHistoryPagesWithoutGapsOrDuplicates() {
        let all = chats(0..<1_132)
        var pages = SessionPages()
        var start = 0
        repeat {
            let end = min(start + SessionPages.pageSize, all.count)
            // Overlap each page by a row, as a chat added meanwhile would.
            let page = Array(all[max(start - 1, 0)..<end])
            pages.append(page, next: end < all.count ? all[end - 1].id : nil)
            start = end
        } while pages.hasMore
        XCTAssertEqual(pages.listed, all)
    }
}
