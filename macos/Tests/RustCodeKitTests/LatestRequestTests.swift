import XCTest
@testable import RustCodeKit

final class LatestRequestTests: XCTestCase {
    /// All → Project (#2163): the All listing started first but answers last,
    /// and must not overwrite the Project listing.
    func testAnEarlierRefreshFinishingLastIsStale() {
        var requests = LatestRequest()
        let all = requests.begin()
        let project = requests.begin()
        XCTAssertTrue(requests.isCurrent(project))
        XCTAssertFalse(requests.isCurrent(all))
    }

    func testRapidTogglesLeaveOnlyTheLastCurrent() {
        var requests = LatestRequest()
        let started = (0..<5).map { _ in requests.begin() }
        XCTAssertEqual(started.filter(requests.isCurrent), [started.last!])
    }

    /// A slow refresh stays stale even after the newer one has published.
    func testOverlappingRefreshesPublishOnlyTheNewest() async {
        actor Sidebar { var shown: [String] = []; func show(_ list: [String]) { shown = list } }
        let sidebar = Sidebar()
        var requests = LatestRequest()
        let all = requests.begin()
        let project = requests.begin()
        let snapshot = requests
        // Project answers first, then the slower All listing.
        if snapshot.isCurrent(project) { await sidebar.show(["here"]) }
        if snapshot.isCurrent(all) { await sidebar.show(["here", "elsewhere"]) }
        let shown = await sidebar.shown
        XCTAssertEqual(shown, ["here"])
    }
}
