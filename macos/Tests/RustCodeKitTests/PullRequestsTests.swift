import XCTest
@testable import RustCodeKit

final class PullRequestsTests: XCTestCase {
    private let pull: JSONValue = [
        "number": 2701, "title": "feat(engine): clean", "author": "LHagfoss", "state": "open", "draft": false,
        "checks": ["passed": 3, "failed": 1, "pending": 2, "skipped": 0],
        "headRef": "feat/x", "baseRef": "main", "headSha": "da4593ba4f64323068e3d1f7d390d2f0a4aab68a",
        "updatedAt": "2026-10-30T14:13:48Z", "url": "https://github.com/o/r/pull/2701",
        "additions": 473, "deletions": 0, "changedFiles": 5,
    ]

    func testAListDecodesItsPullsAndWhetherThereAreMore() throws {
        let list = PullRequestList(result: ["list": ["provider": "github", "pulls": [pull, ["title": "no number"]], "hasMore": true]])
        guard case .loaded(let pulls, let hasMore) = list else { return XCTFail("\(list)") }
        XCTAssertEqual(pulls.count, 1)
        XCTAssertTrue(hasMore)
        let first = try XCTUnwrap(pulls.first)
        XCTAssertEqual(first.number, 2701)
        XCTAssertEqual(first.shortSHA, "da4593b")
        XCTAssertEqual(first.checks.label, "1 failing")
        XCTAssertNil(first.reviewLabel)
        XCTAssertNotNil(first.updatedAt)
        XCTAssertTrue(first.threadDraft.contains("at commit da4593b"))
        XCTAssertTrue(first.threadDraft.contains("without checking it out"))
    }

    func testAnErrorSaysWhatToDo() {
        let list = PullRequestList(result: ["error": ["code": "not_connected", "message": "gh auth login"]])
        guard case .failed(let error) = list else { return XCTFail("\(list)") }
        XCTAssertEqual(error.title, "GitHub is not connected")
        XCTAssertTrue(error.opensConnections)
        XCTAssertFalse(PullRequestError(code: "not_a_repository", message: "").opensConnections)
    }

    func testADetailCarriesChecksFilesAndActivity() throws {
        var fields = pull.objectValue ?? [:]
        fields["body"] = "Why."
        fields["checkRuns"] = [["name": "test", "status": "failed", "url": "https://ci/1"]]
        fields["files"] = [["path": "a.rs", "additions": 3, "deletions": 1]]
        fields["activity"] = [["author": "rev", "kind": "approved", "at": "2026-10-30T15:00:00Z", "body": "ok"]]
        let json = JSONValue.object(fields)
        let detail = try PullRequestDetail.from(result: ["pull": json]).get()
        XCTAssertEqual(detail.body, "Why.")
        XCTAssertEqual(detail.checks.first?.status, "failed")
        XCTAssertEqual(detail.files.first?.additions, 3)
        XCTAssertEqual(detail.activity.first?.kind, "approved")
        XCTAssertThrowsError(try PullRequestDetail.from(result: ["error": ["code": "timed_out", "message": ""]]).get())
    }

    func testRequestsCarryTheFolderAndLeaveOutAnEmptySearch() {
        let params = ACPRequests.pulls(cwd: URL(fileURLWithPath: "/r"), state: "open", search: "  ", limit: 30)
        XCTAssertEqual(params["cwd"]?.stringValue, "/r")
        XCTAssertNil(params["search"])
        XCTAssertEqual(ACPRequests.pulls(cwd: URL(fileURLWithPath: "/r"), state: "all", search: "fix", limit: 30)["search"]?.stringValue, "fix")
    }
}
