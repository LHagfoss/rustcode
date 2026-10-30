import Foundation
import XCTest
@testable import RustCodeKit

final class CodeReferenceTests: XCTestCase {
    private func reference(_ rows: [DiffLine], intent: CodeReference.Intent = .ask) -> CodeReference? {
        CodeReference.from(rows: rows, intent: intent, path: "src/a.swift", oldPath: nil,
                           scope: "uncommitted changes", baseCommit: "abc1234")
    }

    func testAddedAndUnchangedLinesReferTheNewSide() throws {
        let ref = try XCTUnwrap(reference([
            DiffLine(kind: .context, text: "let a = 1", oldNumber: 9, newNumber: 10),
            DiffLine(kind: .added, text: "let b = 2", newNumber: 11),
        ]))
        XCTAssertEqual(ref.side, .new)
        XCTAssertEqual(ref.lines, 10...11)
        XCTAssertEqual(ref.label, "src/a.swift:10-11")
    }

    func testRemovedLinesReferTheOldSideAndMixedSidesAreRefused() throws {
        let removed = try XCTUnwrap(reference([DiffLine(kind: .removed, text: "old()", oldNumber: 4)]))
        XCTAssertEqual(removed.side, .old)
        XCTAssertEqual(removed.label, "src/a.swift:4 (removed)")
        XCTAssertEqual(removed.locate(in: nil), .unchanged, "removed lines are history")
        XCTAssertNil(reference([DiffLine(kind: .removed, text: "x", oldNumber: 4),
                                DiffLine(kind: .added, text: "y", newNumber: 4)]))
        XCTAssertNil(reference([]))
    }

    func testLocateFindsUnchangedMovedChangedAndDeleted() throws {
        let ref = try XCTUnwrap(reference([DiffLine(kind: .added, text: "b", newNumber: 2),
                                           DiffLine(kind: .added, text: "c", newNumber: 3)]))
        XCTAssertEqual(ref.locate(in: "a\nb\nc\nd"), .unchanged)
        XCTAssertEqual(ref.locate(in: "x\ny\na\nb\nc"), .moved(4...5))
        XCTAssertEqual(ref.locate(in: "a\nB\nc"), .changed)
        XCTAssertEqual(ref.locate(in: nil), .deleted)
    }

    func testThePromptNamesTheIntentRangeAndScopeAndSaysWhatChanged() throws {
        let ref = try XCTUnwrap(reference([DiffLine(kind: .added, text: "b", newNumber: 2)], intent: .change))
        let unchanged = ref.prompt(location: .unchanged)
        XCTAssertTrue(unchanged.hasPrefix("Change requested for `src/a.swift` lines 2-2 (uncommitted changes against abc1234):"))
        XCTAssertTrue(unchanged.contains("```\nb\n```"))
        XCTAssertTrue(ref.prompt(location: .moved(7...7)).contains("lines 7-7"))
        XCTAssertTrue(ref.prompt(location: .moved(7...7)).contains("they were at 2-2"))
        XCTAssertTrue(ref.prompt(location: .deleted).contains("has been deleted"))
        // An excerpt holding a fence gets a longer one.
        let fenced = try XCTUnwrap(reference([DiffLine(kind: .added, text: "```", newNumber: 1)]))
        XCTAssertTrue(fenced.prompt(location: .unchanged).contains("````\n```\n````"))
    }

    func testThePromptReadsTheFileOnDiskToSeeWhetherTheLinesMoved() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("coderef-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: root.appendingPathComponent("src"), withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let ref = try XCTUnwrap(CodeReference.from(rows: [DiffLine(kind: .added, text: "b", newNumber: 2)], intent: .ask,
                                                   root: root.path, path: "src/a.swift", oldPath: nil,
                                                   scope: "uncommitted changes", baseCommit: nil))
        try "x\ny\nb\n".write(to: root.appendingPathComponent("src/a.swift"), atomically: true, encoding: .utf8)
        XCTAssertTrue(ref.prompt().contains("lines 3-3"))
        try FileManager.default.removeItem(at: root.appendingPathComponent("src/a.swift"))
        XCTAssertTrue(ref.prompt().contains("has been deleted"))
    }
}
