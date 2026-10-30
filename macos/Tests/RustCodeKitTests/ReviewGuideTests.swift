import XCTest
@testable import RustCodeKit

final class ReviewGuideTests: XCTestCase {
    private func file(_ path: String, _ lines: [DiffLine], status: GitFileChange.Status = .modified) -> GitFileChange {
        GitFileChange(path: path, status: status, rows: lines.map { .line($0) },
                      added: lines.filter { $0.kind == .added }.count, removed: lines.filter { $0.kind == .removed }.count)
    }

    private var files: [GitFileChange] {
        [
            file("rustcode/engine/tests/watch.rs", [DiffLine(kind: .added, text: "#[test] fn x() {}", newNumber: 3)]),
            file("rustcode/engine/src/pull_watch.rs", [
                DiffLine(kind: .context, text: "use x;", oldNumber: 1, newNumber: 1),
                DiffLine(kind: .added, text: "let v = value.unwrap();", newNumber: 2),
            ]),
            file("Cargo.toml", [DiffLine(kind: .removed, text: "old = 1", oldNumber: 9)]),
            file("macos/Sources/RustCode/Sandbox.swift", [DiffLine(kind: .added, text: "allow()", newNumber: 5)]),
            file("docs/watch.md", [DiffLine(kind: .added, text: "Watching", newNumber: 1)]),
        ]
    }

    func testStopsGoContractsFirstAndPointAtRealLines() throws {
        let guide = ReviewGuide.build(files: files, listed: files.map(\.path) + ["logo.png"], head: "abc1234", truncated: false)
        XCTAssertEqual(guide.stops.map(\.kind), [.contract, .sensitive, .change, .tests, .docs])
        let cargo = try XCTUnwrap(guide.stops.first)
        XCTAssertEqual(cargo.first, ReviewGuide.Anchor(path: "Cargo.toml", line: 9, text: "old = 1", removed: true))
        let change = guide.stops[2]
        XCTAssertEqual(change.first?.line, 2)
        XCTAssertEqual(change.flags.first?.message, "Unwraps a value that may be missing")
        XCTAssertEqual(change.flags.first?.anchor?.label, "rustcode/engine/src/pull_watch.rs:2")
        XCTAssertEqual(guide.uncovered, ["logo.png"], "a file the diff did not carry is not claimed")
        // The sandbox code's folder has no test change; the engine's does.
        XCTAssertEqual(guide.untestedFolders, ["macos/RustCode"])
        // Swift's test target and Rust's inline tests count for their code.
        let more = files + [
            file("macos/Tests/RustCodeTests/SandboxTests.swift", [DiffLine(kind: .added, text: "func testA() {}", newNumber: 1)]),
            file("rustcode/tui/src/run.rs", [DiffLine(kind: .added, text: "#[test]", newNumber: 9)]),
        ]
        XCTAssertEqual(ReviewGuide.build(files: more, listed: [], head: "a", truncated: false).untestedFolders, [])
    }

    func testNotesSurviveARebuildAndAMovedHeadIsStale() {
        var guide = ReviewGuide.build(files: files, listed: [], head: "a", truncated: false)
        guide.notes["Cargo.toml"] = "Check the bump"
        guide.notes["gone.rs"] = "old"
        guide.read = ["Cargo.toml"]
        XCTAssertFalse(guide.isStale(head: "a"))
        XCTAssertTrue(guide.isStale(head: "b"))
        let again = ReviewGuide.build(files: files, listed: [], head: "b", truncated: true, previous: guide)
        XCTAssertEqual(again.notes, ["Cargo.toml": "Check the bump"])
        XCTAssertEqual(again.read, ["Cargo.toml"])
        XCTAssertTrue(again.truncated)
    }

    func testALargeDiffIsCappedAndTheRestListed() {
        let many = (0..<(ReviewGuide.maxStops + 5)).map { file("src/f\($0).rs", [DiffLine(kind: .added, text: "x", newNumber: 1)]) }
        let guide = ReviewGuide.build(files: many, listed: many.map(\.path), head: "a", truncated: false)
        XCTAssertEqual(guide.stops.count, ReviewGuide.maxStops)
        XCTAssertEqual(guide.uncovered.count, 5)
    }

    func testAQuestionCitesTheEvidenceAndAsksNotToEdit() {
        let guide = ReviewGuide.build(files: files, listed: [], head: "abc1234ff", truncated: false)
        let question = guide.question(about: guide.stops[2], number: 7)
        XCTAssertTrue(question.contains("#7 at commit abc1234"))
        XCTAssertTrue(question.contains("pull_watch.rs:2"))
        XCTAssertTrue(question.contains("without checking it out or editing"))
    }

    func testGuidesAreKeptOnDisk() throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: dir) }
        let store = ReviewGuideStore(directory: dir)
        XCTAssertNil(store.load(repository: "o/r", number: 1))
        var guide = ReviewGuide.build(files: files, listed: [], head: "a", truncated: false)
        guide.notes["Cargo.toml"] = "n"
        try store.save(guide, repository: "o/r", number: 1)
        XCTAssertEqual(store.load(repository: "o/r", number: 1), guide)
    }
}
