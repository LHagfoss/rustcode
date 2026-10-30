import XCTest
@testable import RustCodeKit

final class ReviewDraftsTests: XCTestCase {
    func testALineCommentIsAnchoredOnItsSide() throws {
        let added = DiffLine(kind: .added, text: "let x = 1", oldNumber: nil, newNumber: 12)
        let removed = DiffLine(kind: .removed, text: "let y = 2", oldNumber: 7, newNumber: nil)
        let right = try XCTUnwrap(ReviewDrafts.Comment(path: "a.rs", line: added, body: "why?", commit: "abc"))
        XCTAssertEqual(right.side, .right)
        XCTAssertEqual(right.line, 12)
        let left = try XCTUnwrap(ReviewDrafts.Comment(path: "a.rs", line: removed, body: "gone?", commit: "abc"))
        XCTAssertEqual(left.side, .left)
        XCTAssertEqual(left.line, 7)
        XCTAssertNil(ReviewDrafts.Comment(path: "a.rs", line: DiffLine(kind: .context, text: ""), body: "", commit: "abc"))
    }

    func testProblemsStaleCommentsAndTheRequest() throws {
        var drafts = ReviewDrafts(repository: "o/r", number: 5)
        XCTAssertNotNil(drafts.problem, "nothing to post")
        drafts.comments.append(.init(path: "a.rs", line: 3, side: .right, lineText: "x", body: "hm", commit: "old"))
        XCTAssertNil(drafts.problem)
        drafts.event = .requestChanges
        XCTAssertNotNil(drafts.problem, "a change request needs a summary")
        drafts.summary = "Please rename."
        XCTAssertNil(drafts.problem)
        XCTAssertEqual(drafts.stale(head: "new").count, 1)
        XCTAssertTrue(drafts.stale(head: "old").isEmpty)
        let request = drafts.request(cwd: URL(fileURLWithPath: "/r"), head: "new")
        XCTAssertEqual(request["repository"]?.stringValue, "o/r")
        XCTAssertEqual(request["commitId"]?.stringValue, "new")
        XCTAssertEqual(request["event"]?.stringValue, "REQUEST_CHANGES")
        XCTAssertEqual(request["comments"]?.arrayValue?.first?["side"]?.stringValue, "RIGHT")
        XCTAssertEqual(ReviewDrafts.repository(of: URL(string: "https://github.com/LHagfoss/rustcode/pull/2701")), "LHagfoss/rustcode")
    }

    func testSubmissionOutcomes() {
        XCTAssertEqual(ReviewSubmission(result: ["review": ["url": "https://x/1"]]), .posted(URL(string: "https://x/1")))
        XCTAssertEqual(ReviewSubmission(result: ["error": ["code": "uncertain", "message": "m"]]), .uncertain("m"))
        XCTAssertEqual(ReviewSubmission(result: ["error": ["code": "failed", "message": "422"]]), .failed("422"))
    }

    func testDraftsAreKeptOnDiskAndRemovedWhenEmpty() throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: dir) }
        let store = ReviewDraftStore(directory: dir)
        var drafts = store.load(repository: "o/r", number: 5)
        XCTAssertTrue(drafts.isEmpty)
        drafts.summary = "Looks good"
        try store.save(drafts)
        XCTAssertEqual(store.load(repository: "o/r", number: 5).summary, "Looks good")
        drafts.summary = ""
        try store.save(drafts)
        XCTAssertTrue(try FileManager.default.contentsOfDirectory(atPath: dir.path).isEmpty)
    }
}
