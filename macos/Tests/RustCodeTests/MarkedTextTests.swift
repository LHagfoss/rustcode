import AppKit
import XCTest
@testable import RustCode

/// #2214: a dead key's accent, such as ` on a Norwegian keyboard, still
/// composing when ⏎ sends.
@MainActor
final class MarkedTextTests: XCTestCase {
    private func composing(_ typed: String, marked: String) -> NSTextView {
        let editor = NSTextView(frame: NSRect(x: 0, y: 0, width: 200, height: 40))
        editor.string = typed
        editor.setSelectedRange(NSRange(location: typed.utf16.count, length: 0))
        editor.setMarkedText(marked, selectedRange: NSRange(location: marked.utf16.count, length: 0),
                             replacementRange: NSRange(location: NSNotFound, length: 0))
        return editor
    }

    func testAMarkedBacktickIsCommittedAsTyped() {
        let editor = composing("run `ls", marked: "`")
        XCTAssertTrue(editor.hasMarkedText())
        XCTAssertTrue(MarkedText.commit(in: editor))
        XCTAssertFalse(editor.hasMarkedText())
        XCTAssertEqual(editor.string, "run `ls`")
    }

    /// The field's binding hears the commit, so the draft sent includes it.
    func testTheCommitIsAnnouncedAsAnEdit() {
        let editor = composing("hello", marked: "`")
        let changed = expectation(forNotification: NSText.didChangeNotification, object: editor)
        MarkedText.commit(in: editor)
        wait(for: [changed], timeout: 1)
    }

    func testNothingComposingIsLeftAlone() {
        let editor = NSTextView()
        editor.string = "hello`"
        XCTAssertFalse(MarkedText.commit(in: editor))
        XCTAssertEqual(editor.string, "hello`")
        XCTAssertFalse(MarkedText.commit(in: nil))
        XCTAssertFalse(MarkedText.commit(in: NSView()))
    }

    /// The field being typed in is found in whichever window holds it.
    func testCommitsTheFieldComposingInAWindow() {
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 300, height: 100),
                              styleMask: [.titled], backing: .buffered, defer: true)
        window.isReleasedWhenClosed = false
        let editor = composing("hello", marked: "`")
        window.contentView?.addSubview(editor)
        XCTAssertTrue(window.makeFirstResponder(editor))
        XCTAssertTrue(editor.hasMarkedText())
        XCTAssertTrue(MarkedText.commit(in: window.firstResponder))
        XCTAssertEqual(editor.string, "hello`")
        XCTAssertFalse(MarkedText.commitInWindows(), "nothing is left composing")
        window.close()
    }
}
