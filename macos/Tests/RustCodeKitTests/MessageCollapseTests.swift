import XCTest
@testable import RustCodeKit

final class MessageCollapseTests: XCTestCase {
    private func lines(_ count: Int) -> String {
        (0..<count).map { "line \($0)" }.joined(separator: "\n")
    }

    func testMessagesUpToTheLimitAreNeverMeasured() {
        XCTAssertFalse(MessageCollapse.mayBeLong(""))
        XCTAssertFalse(MessageCollapse.mayBeLong(lines(MessageCollapse.maxLines)))
        XCTAssertTrue(MessageCollapse.mayBeLong(lines(MessageCollapse.maxLines + 1)))
    }

    func testOneHugeLineCountsTheLinesItWrapsTo() {
        let columns = MessageCollapse.approxColumns
        // Twelve lines' worth wraps to exactly twelve; one character more to thirteen.
        XCTAssertFalse(MessageCollapse.mayBeLong(String(repeating: "x", count: columns * MessageCollapse.maxLines)))
        XCTAssertTrue(MessageCollapse.mayBeLong(String(repeating: "x", count: columns * MessageCollapse.maxLines + 1)))
        XCTAssertTrue(MessageCollapse.mayBeLong(String(repeating: "word ", count: 5_000)))
    }

    func testBlankLinesCount() {
        XCTAssertTrue(MessageCollapse.mayBeLong(String(repeating: "\n", count: MessageCollapse.maxLines)))
    }

    func testFoldsOnlyWhenFoldingSavesRoom() {
        let line = 18.0
        let folded = MessageCollapse.foldedHeight(lineHeight: line)
        XCTAssertEqual(folded, Double(MessageCollapse.maxLines) * line)
        // Twelve lines and the control's height draw whole; any more fold.
        let limit = folded + MessageCollapse.controlHeight
        XCTAssertEqual(MessageCollapse.foldHeight(lineHeight: line), limit)
        XCTAssertFalse(MessageCollapse.isLong(height: folded, lineHeight: line))
        XCTAssertFalse(MessageCollapse.isLong(height: limit, lineHeight: line))
        XCTAssertTrue(MessageCollapse.isLong(height: limit + 1, lineHeight: line))
        XCTAssertTrue(MessageCollapse.isLong(height: 100 * line, lineHeight: line))
    }

    func testOnlyUserMessagesFold() {
        XCTAssertTrue(MessageCollapse.folds(.user("hi")))
        XCTAssertFalse(MessageCollapse.folds(.assistant(lines(40))))
        XCTAssertFalse(MessageCollapse.folds(.thinking("hi")))
        XCTAssertFalse(MessageCollapse.folds(.command(input: "/x", output: lines(40))))
    }

    func testReplayedMessagesStartFoldedAndOpenPerChat() {
        var transcript = Transcript()
        transcript.apply(.userText(lines(40)))
        transcript.apply(.agentText(lines(40)))
        let ids = transcript.items.map(\.id)
        var expanded = ExpandedMessages()
        // Nothing is open in a chat just loaded.
        XCTAssertTrue(expanded.ids(in: "a").isEmpty)

        expanded.toggle(ids[1], in: "a")
        XCTAssertEqual(expanded.ids(in: "a"), [ids[1]])
        XCTAssertFalse(expanded.contains(ids[1], in: "b"))
        XCTAssertFalse(expanded.contains(ids[1], in: nil))

        expanded.toggle(ids[1], in: "a")
        XCTAssertTrue(expanded.ids(in: "a").isEmpty)
    }

    func testExpandKeepsAnOpenMessageOpen() {
        var expanded = ExpandedMessages()
        XCTAssertTrue(expanded.expand(3, in: nil))
        XCTAssertFalse(expanded.expand(3, in: nil))
        XCTAssertTrue(expanded.contains(3, in: nil))
    }
}
