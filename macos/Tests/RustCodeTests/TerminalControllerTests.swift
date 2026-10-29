import XCTest
import RustCodeTerminal
@testable import RustCode

/// ⌘W in the terminal panel.
@MainActor
final class TerminalControllerTests: XCTestCase {
    private static func shell() -> TerminalLaunch {
        TerminalLaunch(executable: "/bin/sh", execName: "sh", environment: [
            "PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "TERM": "xterm-256color", "PS1": "$ ",
            "HOME": NSTemporaryDirectory(), "LANG": "en_US.UTF-8",
        ])
    }

    /// #2252: ⌘W closes the selected tab, the panel goes with the last one,
    /// and with the panel hidden ⌘W does nothing.
    func testCommandWClosesTheSelectedTabThenThePanel() {
        let terminal = TerminalController(manager: TerminalSessionManager(launch: Self.shell))
        defer { terminal.manager.terminateAll() }
        terminal.setWorkspace(NSTemporaryDirectory())
        terminal.open()
        terminal.newSession()
        XCTAssertEqual(terminal.manager.projectSessions.count, 2)
        let first = terminal.manager.projectSessions[0]

        XCTAssertTrue(terminal.closeSelectedTab())
        XCTAssertTrue(terminal.isShown)
        XCTAssertEqual(terminal.manager.projectSessions.map(\.id), [first.id])
        XCTAssertEqual(terminal.manager.selectedSession?.id, first.id)

        XCTAssertTrue(terminal.closeSelectedTab())
        XCTAssertFalse(terminal.isShown)
        XCTAssertTrue(terminal.manager.projectSessions.isEmpty)

        XCTAssertFalse(terminal.closeSelectedTab())
    }

    /// #2594: a held ⌘W's auto-repeats close nothing more, so a press meant
    /// for one terminal no longer takes the others and the panel with it.
    func testHeldCommandWClosesOneTabAndKeepsThePanel() {
        let terminal = TerminalController(manager: TerminalSessionManager(launch: Self.shell))
        defer { terminal.manager.terminateAll() }
        terminal.setWorkspace(NSTemporaryDirectory())
        terminal.open()
        terminal.newSession()
        let first = terminal.manager.projectSessions[0]

        XCTAssertTrue(terminal.closeSelectedTab(isKeyRepeat: false))
        XCTAssertTrue(terminal.closeSelectedTab(isKeyRepeat: true))
        XCTAssertTrue(terminal.closeSelectedTab(isKeyRepeat: true))
        XCTAssertTrue(terminal.isShown)
        XCTAssertEqual(terminal.manager.projectSessions.map(\.id), [first.id])

        // Opening right after the last closes shows the panel again.
        XCTAssertTrue(terminal.closeSelectedTab(isKeyRepeat: false))
        XCTAssertFalse(terminal.isShown)
        terminal.newSession()
        XCTAssertTrue(terminal.isShown)
        XCTAssertEqual(terminal.manager.projectSessions.count, 1)
    }
}
