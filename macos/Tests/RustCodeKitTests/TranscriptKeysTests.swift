import XCTest
@testable import RustCodeKit

final class TranscriptKeysTests: XCTestCase {
    private typealias Key = TranscriptKeys.KeyCode

    func testPageAndJumpKeysMapToChatActions() {
        XCTAssertEqual(TranscriptKeys.action(keyCode: Key.pageUp, modified: false, focus: .chat), .pageUp)
        XCTAssertEqual(TranscriptKeys.action(keyCode: Key.pageDown, modified: false, focus: .chat), .pageDown)
        XCTAssertEqual(TranscriptKeys.action(keyCode: Key.home, modified: false, focus: .chat), .top)
        XCTAssertEqual(TranscriptKeys.action(keyCode: Key.end, modified: false, focus: .chat), .end)
    }

    /// The composer keeps focus while reading: the keys still scroll the chat.
    func testKeysReachTheChatFromTheComposer() {
        XCTAssertEqual(TranscriptKeys.action(keyCode: Key.pageDown, modified: false, focus: .textField), .pageDown)
        XCTAssertEqual(TranscriptKeys.action(keyCode: Key.end, modified: false, focus: .textField), .end)
    }

    func testOtherKeysAreLeftAlone() {
        // Arrows, Return and letters belong to whoever has focus.
        for code: UInt16 in [123, 124, 125, 126, 36, 0, 49] {
            XCTAssertNil(TranscriptKeys.action(keyCode: code, modified: false, focus: .chat), "key \(code)")
        }
    }

    /// Shift-Page Down selects by page in the composer; ⌘ and ⌥ variants are
    /// the system's.
    func testModifiedKeysAreLeftAlone() {
        for code in [Key.pageUp, Key.pageDown, Key.home, Key.end] {
            XCTAssertNil(TranscriptKeys.action(keyCode: code, modified: true, focus: .textField))
            XCTAssertNil(TranscriptKeys.action(keyCode: code, modified: true, focus: .chat))
        }
    }

    /// The terminal pages its scrollback, the preview its page, and a sheet
    /// over the chat its own content.
    func testKeysStayWithTerminalPreviewAndPanels() {
        for focus: TranscriptKeys.Focus in [.terminal, .web, .panel] {
            for code in [Key.pageUp, Key.pageDown, Key.home, Key.end] {
                XCTAssertNil(TranscriptKeys.action(keyCode: code, modified: false, focus: focus), "\(focus) \(code)")
            }
        }
    }

    func testAPageIsTheVisibleHeightLessTheOverlap() {
        XCTAssertEqual(TranscriptKeys.pageDistance(visible: 600), 560)
        // A tiny chat still moves.
        XCTAssertEqual(TranscriptKeys.pageDistance(visible: 50), TranscriptKeys.pageOverlap)
        XCTAssertEqual(TranscriptKeys.pageDistance(visible: 0), TranscriptKeys.pageOverlap)
    }

    /// The last page down lands on the end and follows output from there.
    func testPageDownNearTheEndGoesToTheEnd() {
        XCTAssertEqual(TranscriptKeys.resolved(.pageDown, distanceFromBottom: 300, visible: 600), .end)
        XCTAssertEqual(TranscriptKeys.resolved(.pageDown, distanceFromBottom: 560 + ChatFollow.threshold, visible: 600), .end)
        XCTAssertEqual(TranscriptKeys.resolved(.pageDown, distanceFromBottom: 2_000, visible: 600), .pageDown)
        XCTAssertEqual(TranscriptKeys.resolved(.pageDown, distanceFromBottom: nil, visible: 600), .pageDown)
    }

    func testOnlyPageDownIsResolved() {
        for action: TranscriptKeys.Action in [.pageUp, .top, .end] {
            XCTAssertEqual(TranscriptKeys.resolved(action, distanceFromBottom: 0, visible: 600), action)
        }
    }
}
