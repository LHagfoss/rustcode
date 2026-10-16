import XCTest
@testable import RustCodeKit

final class SessionSearchTests: XCTestCase {
    func testParsesEngineJSON() throws {
        let json = """
        [{"id":"a1","title":"Fix parser","snippet":"…the parser now…","matches":3,"when":"12:00","cwd":"/work/app"},
         {"id":"b2","title":"Legacy","snippet":"parser","matches":1,"when":"","cwd":null}]
        """
        let matches = try SessionContentSearch.parse(Data(json.utf8))
        XCTAssertEqual(matches.map(\.id), ["a1", "b2"])
        XCTAssertEqual(matches[0].snippet, "…the parser now…")
        XCTAssertEqual(matches[0].matches, 3)
        XCTAssertEqual(matches[0].cwd, "/work/app")
        XCTAssertNil(matches[1].cwd)
        XCTAssertEqual(try SessionContentSearch.parse(Data("[]".utf8)), [])
        XCTAssertThrowsError(try SessionContentSearch.parse(Data("No saved sessions".utf8)))
    }

    func testShortQueriesOnlyFilterTitles() {
        XCTAssertNil(SessionContentSearch.query(for: " a "))
        XCTAssertEqual(SessionContentSearch.query(for: " ab "), "ab")
    }

    func testArgumentsFollowTheProjectScope() {
        XCTAssertEqual(SessionContentSearch.arguments(query: "x y", allProjects: false, limit: 5),
                       ["sessions", "search", "x y", "--limit", "5", "--json"])
        XCTAssertEqual(SessionContentSearch.arguments(query: "x", allProjects: true).suffix(1), ["--all"])
    }

    func testTitleMatchesComeFirstThenContentMatchesInEngineOrder() {
        let shown = [
            SessionSummary(id: "1", cwd: "/w/app", title: "Parser work", updatedAt: nil),
            SessionSummary(id: "2", cwd: "/w/app", title: "Other", updatedAt: nil),
            SessionSummary(id: "3", cwd: "/w/app", title: "Third", updatedAt: nil),
        ]
        let content = ["3", "1", "9", "2"].map {
            SessionContentMatch(id: $0, title: nil, snippet: "parser", matches: 1, cwd: nil)
        }
        let merged = SessionContentSearch.merge(shown: shown, filter: "parser", contentMatches: content)
        // "1" matches by title, so it is not repeated; "9" is outside the shown list.
        XCTAssertEqual(merged.map(\.id), ["1", "3", "2"])
        XCTAssertEqual(SessionContentSearch.merge(shown: shown, filter: "parser", contentMatches: []).map(\.id), ["1"])
    }
}
