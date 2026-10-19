import Darwin
import Foundation
import SwiftTerm
import XCTest
@testable import RustCodeTerminal

/// These run real shells (`/bin/sh`) on real PTYs.
@MainActor
final class TerminalSessionTests: XCTestCase {
    private var folders: [URL] = []

    override func tearDown() async throws {
        for folder in folders { try? FileManager.default.removeItem(at: folder) }
        folders = []
    }

    private func makeFolder(_ name: String) throws -> String {
        let base = URL(fileURLWithPath: NSTemporaryDirectory()).resolvingSymlinksInPath()
        let folder = base.appendingPathComponent("rc-terminal-\(name)-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        folders.append(folder)
        return folder.path
    }

    private static func shell() -> TerminalLaunch {
        TerminalLaunch(executable: "/bin/sh", execName: "sh", environment: [
            "PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "TERM": "xterm-256color", "PS1": "$ ",
            "HOME": NSTemporaryDirectory(), "LANG": "en_US.UTF-8",
        ])
    }

    private func manager(limit: Int = TerminalLimits.sessionsPerProject) -> TerminalSessionManager {
        TerminalSessionManager(sessionsPerProject: limit, launch: Self.shell)
    }

    /// Collects a session's output as text.
    private final class Capture {
        var text = ""
    }

    private func capture(_ session: TerminalSession) -> Capture {
        let capture = Capture()
        session.output = { capture.text += String(decoding: $0, as: UTF8.self) }
        return capture
    }

    private func waitUntil(_ what: String, timeout: TimeInterval = 10, _ condition: () -> Bool) async throws {
        let deadline = Date().addingTimeInterval(timeout)
        while !condition() {
            if Date() > deadline { XCTFail("Timed out waiting for \(what)"); throw CancellationError() }
            try await Task.sleep(for: .milliseconds(20))
        }
    }

    /// Runs `command` and waits for its output to end with a marker that
    /// only appears once the shell has executed it, not in the echoed input.
    private func run(_ command: String, in session: TerminalSession, _ capture: Capture) async throws -> String {
        let marker = "END\(Int.random(in: 1000...9999))"
        let start = capture.text.endIndex
        session.send(text: "\(command)\necho \(marker.prefix(3))$((\(marker.dropFirst(3)) + 0))\n")
        try await waitUntil("output of \(command)") {
            capture.text[start...].components(separatedBy: marker).count > 1
        }
        return String(capture.text[start...])
    }

    private func pid(after label: String, in text: String) -> pid_t? {
        guard let range = text.range(of: "\(label)=(\\d+)", options: .regularExpression) else { return nil }
        return pid_t(text[range].dropFirst(label.count + 1))
    }

    // MARK: - Lifecycle

    func testShellStartsInTheProjectFolder() async throws {
        let folder = try makeFolder("start")
        let terminals = manager()
        terminals.setProject(folder)
        let session = try XCTUnwrap(terminals.newSession())
        let output = capture(session)
        XCTAssertTrue(session.isRunning)
        XCTAssertEqual(session.project, folder)
        XCTAssertEqual(session.launchCount, 1)
        XCTAssertFalse(session.hasRunningChildren)
        let printed = try await run("pwd", in: session, output)
        XCTAssertTrue(printed.contains(folder), printed)
        terminals.close(session.id, wait: true)
    }

    func testExitedShellReportsItsStatusAndRelaunches() async throws {
        let folder = try makeFolder("relaunch")
        let terminals = manager()
        terminals.setProject(folder)
        let session = try XCTUnwrap(terminals.newSession())
        let output = capture(session)
        let firstPid = try XCTUnwrap(session.pid)
        session.send(text: "exit 3\n")
        try await waitUntil("exit") { session.status == .exited(code: 3) }
        XCTAssertTrue(session.canRelaunch)
        XCTAssertFalse(ProcessTree.isAlive(firstPid))
        // Input to an exited shell goes nowhere.
        session.send(text: "echo ignored\n")

        terminals.relaunch(session.id)
        XCTAssertTrue(session.isRunning)
        XCTAssertEqual(session.launchCount, 2)
        XCTAssertNotEqual(session.pid, firstPid)
        let printed = try await run("pwd", in: session, output)
        XCTAssertTrue(printed.contains(folder), printed)
        XCTAssertEqual(terminals.projectSessions.count, 1, "relaunching reuses the session")
        terminals.close(session.id, wait: true)
    }

    func testCloseEndsTheShellAndItsProcesses() async throws {
        let folder = try makeFolder("close")
        let terminals = manager()
        terminals.setProject(folder)
        let session = try XCTUnwrap(terminals.newSession())
        let output = capture(session)
        let background = try await run("sleep 300 & echo BG=$!", in: session, output)
        let job = try XCTUnwrap(pid(after: "BG", in: background), background)
        // A foreground program holding the terminal, as a dev server does.
        session.send(text: "sleep 301\n")
        let shell = try XCTUnwrap(session.pid)
        try await waitUntil("foreground job") { ProcessTree.descendants(of: shell).count >= 2 }
        let tree = ProcessTree.descendants(of: shell)
        XCTAssertTrue(tree.contains(job))
        XCTAssertTrue(session.hasRunningChildren, "closing it asks first")

        terminals.close(session.id, wait: true)
        XCTAssertEqual(session.status, .terminated)
        XCTAssertFalse(session.canRelaunch)
        XCTAssertTrue(terminals.projectSessions.isEmpty)
        try await waitUntil("processes to end") {
            !([shell] + tree).contains(where: ProcessTree.isAlive)
        }
    }

    func testHidingOutputDoesNotStopTheShell() async throws {
        // The panel closing detaches the view; the shell keeps running and
        // what it prints meanwhile is replayed.
        let folder = try makeFolder("detach")
        let terminals = manager()
        terminals.setProject(folder)
        let session = try XCTUnwrap(terminals.newSession())
        session.send(text: "echo HELD$((40 + 2))\n")
        try await waitUntil("detached output") { session.detachedOutputCount > 0 }
        try await Task.sleep(for: .milliseconds(200))
        XCTAssertTrue(session.isRunning)
        let output = capture(session)
        XCTAssertTrue(output.text.contains("HELD42"), output.text)
        XCTAssertEqual(session.detachedOutputCount, 0)
        terminals.close(session.id, wait: true)
    }

    func testDetachedOutputIsBounded() async throws {
        let folder = try makeFolder("bounded")
        let session = TerminalSession(project: folder, ordinal: 1, launch: Self.shell(), detachedOutputLimit: 1024)
        session.start()
        session.send(text: "head -c 20000 /dev/zero | tr '\\0' x; echo\n")
        try await waitUntil("output") { session.detachedOutputCount == 1024 }
        try await Task.sleep(for: .milliseconds(300))
        XCTAssertEqual(session.detachedOutputCount, 1024)
        session.terminate(wait: true)
    }

    func testMissingProjectFolderIsNotReplacedByAnother() throws {
        let terminals = manager()
        terminals.setProject("/nonexistent/rustcode-terminal-test")
        let session = try XCTUnwrap(terminals.newSession())
        guard case .failed(let message) = session.status else {
            return XCTFail("expected a failure, got \(session.status)")
        }
        XCTAssertTrue(message.contains("/nonexistent/rustcode-terminal-test"))
        XCTAssertNil(session.pid)
    }

    func testSessionsPerProjectAreLimited() throws {
        let folder = try makeFolder("limit")
        let terminals = manager(limit: 2)
        terminals.setProject(folder)
        XCTAssertNotNil(terminals.newSession())
        XCTAssertNotNil(terminals.newSession())
        XCTAssertFalse(terminals.canOpenSession)
        XCTAssertNil(terminals.newSession())
        XCTAssertEqual(terminals.projectSessions.map(\.displayTitle), ["Terminal", "Terminal 2"])
        terminals.terminateAll(grace: 1)
    }

    func testClosingSelectsANeighbour() throws {
        let folder = try makeFolder("select")
        let terminals = manager()
        terminals.setProject(folder)
        let first = try XCTUnwrap(terminals.newSession())
        let second = try XCTUnwrap(terminals.newSession())
        let third = try XCTUnwrap(terminals.newSession())
        terminals.select(second.id)
        terminals.close(second.id, wait: true)
        XCTAssertEqual(terminals.selectedSession?.id, third.id)
        terminals.close(third.id, wait: true)
        XCTAssertEqual(terminals.selectedSession?.id, first.id)
        terminals.close(first.id, wait: true)
        XCTAssertNil(terminals.selectedSession)
    }

    // MARK: - Projects

    func testProjectSwitchKeepsTerminalsInTheirOwnProject() async throws {
        let projectA = try makeFolder("a")
        let projectB = try makeFolder("b")
        let terminals = manager()
        terminals.setProject(projectA)
        let a = try XCTUnwrap(terminals.newSession())

        terminals.setProject(projectB + "/")
        XCTAssertEqual(terminals.project, projectB)
        XCTAssertTrue(terminals.projectSessions.isEmpty)
        XCTAssertNil(terminals.selectedSession, "another project's terminal is never shown")
        terminals.select(a.id)
        XCTAssertNil(terminals.selectedSession, "or selected")
        XCTAssertEqual(terminals.runningElsewhere.map(\.id), [a.id])
        XCTAssertTrue(a.isRunning, "switching projects does not stop a shell")

        let b = try XCTUnwrap(terminals.ensureSession())
        XCTAssertEqual(b.project, projectB)
        let output = capture(b)
        let printed = try await run("pwd", in: b, output)
        XCTAssertTrue(printed.contains(projectB), printed)
        XCTAssertFalse(printed.contains(projectA), printed)

        terminals.setProject(projectA)
        XCTAssertEqual(terminals.selectedSession?.id, a.id)
        XCTAssertEqual(terminals.projectSessions.map(\.id), [a.id])
        terminals.terminateAll(grace: 1)
    }

    // MARK: - Size

    func testResizeReachesThePTY() async throws {
        let folder = try makeFolder("resize")
        let session = TerminalSession(project: folder, ordinal: 1, launch: Self.shell())
        session.resize(columns: 91, rows: 29)
        session.start()
        let output = capture(session)
        var printed = try await run("stty size", in: session, output)
        XCTAssertTrue(printed.contains("29 91"), "starts at the view's size: \(printed)")

        session.resize(columns: 132, rows: 41)
        printed = try await run("stty size", in: session, output)
        XCTAssertTrue(printed.contains("41 132"), printed)

        session.resize(columns: 0, rows: 10)
        XCTAssertEqual(session.columns, 132, "an empty size is ignored")
        session.terminate(wait: true)
    }

    // MARK: - Quit

    func testTerminateAllEndsEveryProjectsShells() async throws {
        let projectA = try makeFolder("quit-a")
        let projectB = try makeFolder("quit-b")
        let terminals = manager()
        var pids: [pid_t] = []
        for project in [projectA, projectB] {
            terminals.setProject(project)
            let session = try XCTUnwrap(terminals.newSession())
            let output = capture(session)
            let printed = try await run("sleep 300 & echo BG=$!", in: session, output)
            pids.append(try XCTUnwrap(session.pid))
            pids.append(try XCTUnwrap(pid(after: "BG", in: printed), printed))
        }
        let started = Date()
        terminals.terminateAll(grace: 1)
        XCTAssertLessThan(Date().timeIntervalSince(started), 3)
        XCTAssertTrue(terminals.sessions.allSatisfy { $0.status == .terminated })
        try await waitUntil("processes to end") { !pids.contains(where: ProcessTree.isAlive) }
    }

    func testAShellIgnoringHangupIsKilled() async throws {
        let folder = try makeFolder("stubborn")
        let session = TerminalSession(project: folder, ordinal: 1, launch: Self.shell())
        session.start()
        let output = capture(session)
        _ = try await run("trap '' HUP TERM", in: session, output)
        let shell = try XCTUnwrap(session.pid)
        session.terminate(grace: 0.3, wait: true)
        XCTAssertFalse(ProcessTree.isAlive(shell))
    }

    // MARK: - Limits

    func testScrollbackIsBounded() {
        final class Host: TerminalDelegate {
            func send(source: Terminal, data: ArraySlice<UInt8>) {}
        }
        let host = Host()
        let terminal = Terminal(delegate: host, options: TerminalLimits.emulatorOptions(columns: 80, rows: 24))
        XCTAssertEqual(terminal.options.scrollback, TerminalLimits.scrollbackLines)
        let lines = TerminalLimits.scrollbackLines * 2
        var text = ""
        for line in 0..<lines { text += "line \(line)\r\n" }
        terminal.feed(text: text)
        XCTAssertLessThanOrEqual(terminal.buffer.yDisp, TerminalLimits.scrollbackLines)
        XCTAssertGreaterThan(terminal.buffer.totalLinesTrimmed, 0, "old lines are dropped")
    }

    func testExitCodeDecodesWaitStatus() {
        XCTAssertEqual(TerminalSession.exitCode(rawStatus: 0), 0)
        XCTAssertEqual(TerminalSession.exitCode(rawStatus: 3 << 8), 3)
        XCTAssertEqual(TerminalSession.exitCode(rawStatus: SIGKILL), 128 + SIGKILL)
    }
}
