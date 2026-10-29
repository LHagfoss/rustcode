import Foundation
import XCTest
@testable import RustCodeTerminal

/// #2594: ⌘W closes one terminal per press, and the panel only with the last.
@MainActor
final class TerminalPanelCloseTests: XCTestCase {
    private var folders: [URL] = []

    override func tearDown() async throws {
        for folder in folders { try? FileManager.default.removeItem(at: folder) }
        folders = []
    }

    private func project() throws -> String {
        let base = URL(fileURLWithPath: NSTemporaryDirectory()).resolvingSymlinksInPath()
        let folder = base.appendingPathComponent("rc-panel-close-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        folders.append(folder)
        return folder.path
    }

    private func manager() throws -> TerminalSessionManager {
        let terminals = TerminalSessionManager {
            TerminalLaunch(executable: "/bin/sh", execName: "sh", environment: [
                "PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "TERM": "xterm-256color", "PS1": "$ ",
                "HOME": NSTemporaryDirectory(), "LANG": "en_US.UTF-8",
            ])
        }
        terminals.setProject(try project())
        return terminals
    }

    /// One ⌘W press as the panel handles it: the step, then the close and
    /// whether the panel stays. Returns whether the panel is shown after.
    private func press(_ terminals: TerminalSessionManager, shown: Bool, repeat isKeyRepeat: Bool = false) -> Bool {
        switch TerminalPanelClose.step(panelShown: shown, selected: terminals.selectedSession?.id, isKeyRepeat: isKeyRepeat) {
        case .nothing: return shown
        case .hidePanel: return false
        case .close(let id):
            terminals.close(id, wait: true)
            return TerminalPanelClose.keepsPanel(remaining: terminals.projectSessions.count)
        }
    }

    func testClosingTheLastTerminalClosesThePanel() throws {
        let terminals = try manager()
        let only = try XCTUnwrap(terminals.newSession())
        XCTAssertEqual(TerminalPanelClose.step(panelShown: true, selected: only.id, isKeyRepeat: false), .close(only.id))
        XCTAssertFalse(press(terminals, shown: true))
        XCTAssertTrue(terminals.projectSessions.isEmpty)
        XCTAssertNil(terminals.selectedSession)
    }

    func testClosingOneOfManyKeepsThePanelWithANeighbourSelected() throws {
        let terminals = try manager()
        let first = try XCTUnwrap(terminals.newSession())
        let second = try XCTUnwrap(terminals.newSession())
        let third = try XCTUnwrap(terminals.newSession())
        terminals.select(second.id)
        defer { terminals.terminateAll(grace: 1) }

        XCTAssertTrue(press(terminals, shown: true))
        XCTAssertEqual(terminals.projectSessions.map(\.id), [first.id, third.id])
        XCTAssertEqual(terminals.selectedSession?.id, third.id)
    }

    /// Holding ⌘W sends auto-repeats; they used to close every terminal and
    /// then the panel.
    func testAHeldKeyClosesOnlyOneTerminal() throws {
        let terminals = try manager()
        let first = try XCTUnwrap(terminals.newSession())
        _ = try XCTUnwrap(terminals.newSession())
        defer { terminals.terminateAll(grace: 1) }

        var shown = press(terminals, shown: true)
        for _ in 0..<3 { shown = press(terminals, shown: shown, repeat: true) }
        XCTAssertTrue(shown)
        XCTAssertEqual(terminals.projectSessions.map(\.id), [first.id])
        XCTAssertEqual(TerminalPanelClose.step(panelShown: true, selected: first.id, isKeyRepeat: true), .nothing)
    }

    func testOpeningDuringACloseKeepsThePanelAndSelectsTheNewTerminal() throws {
        let terminals = try manager()
        _ = try XCTUnwrap(terminals.newSession())
        defer { terminals.terminateAll(grace: 1) }

        // A terminal opened right before the close: the panel stays with it.
        let opened = try XCTUnwrap(terminals.newSession())
        terminals.select(terminals.projectSessions[0].id)
        XCTAssertTrue(press(terminals, shown: true))
        XCTAssertEqual(terminals.projectSessions.map(\.id), [opened.id])
        XCTAssertEqual(terminals.selectedSession?.id, opened.id)

        // Closing the last, then opening right after: a fresh terminal is
        // selected and a second press closes it and the panel again.
        XCTAssertFalse(press(terminals, shown: true))
        let reopened = try XCTUnwrap(terminals.newSession())
        XCTAssertEqual(terminals.selectedSession?.id, reopened.id)
        XCTAssertFalse(press(terminals, shown: true))
        XCTAssertTrue(terminals.projectSessions.isEmpty)
    }

    func testHiddenPanelIgnoresTheKeyAndAnEmptyOneHides() {
        let id = UUID()
        XCTAssertEqual(TerminalPanelClose.step(panelShown: false, selected: id, isKeyRepeat: false), .nothing)
        XCTAssertEqual(TerminalPanelClose.step(panelShown: false, selected: nil, isKeyRepeat: false), .nothing)
        XCTAssertEqual(TerminalPanelClose.step(panelShown: true, selected: nil, isKeyRepeat: false), .hidePanel)
        XCTAssertEqual(TerminalPanelClose.step(panelShown: true, selected: nil, isKeyRepeat: true), .nothing)
        XCTAssertTrue(TerminalPanelClose.keepsPanel(remaining: 1))
        XCTAssertFalse(TerminalPanelClose.keepsPanel(remaining: 0))
    }
}
