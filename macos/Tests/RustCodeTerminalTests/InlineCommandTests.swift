import Foundation
import XCTest
@testable import RustCodeTerminal

final class InlineCommandParseTests: XCTestCase {
    func testBangRunsTheRestAsACommand() {
        XCTAssertEqual(InlineCommand.parse("! ls"), .command("ls"))
        XCTAssertEqual(InlineCommand.parse("!ls -la"), .command("ls -la"))
        XCTAssertEqual(InlineCommand.parse("!  git status  "), .command("git status"))
    }

    func testOtherMessagesGoToTheAgent() {
        XCTAssertEqual(InlineCommand.parse("ls"), .message("ls"))
        XCTAssertEqual(InlineCommand.parse("run ! ls"), .message("run ! ls"))
    }

    func testDoubleBangSendsOneBangToTheAgent() {
        XCTAssertEqual(InlineCommand.parse("!!important"), .message("!important"))
        XCTAssertEqual(InlineCommand.parse("!! ls"), .message("! ls"))
    }

    func testEmptyAndMultilineCommandsAreRefused() {
        guard case .invalid = InlineCommand.parse("!") else { return XCTFail("lone !") }
        guard case .invalid = InlineCommand.parse("!   ") else { return XCTFail("blank") }
        guard case .invalid = InlineCommand.parse("! ls\nrm -rf x") else { return XCTFail("two lines") }
    }
}

/// These run real shells (`/bin/sh`) on real PTYs.
@MainActor
final class InlineCommandRoutingTests: XCTestCase {
    private var folders: [URL] = []

    override func tearDown() async throws {
        for folder in folders { try? FileManager.default.removeItem(at: folder) }
        folders = []
    }

    private func makeFolder(_ name: String) throws -> String {
        let base = URL(fileURLWithPath: NSTemporaryDirectory()).resolvingSymlinksInPath()
        let folder = base.appendingPathComponent("rc-inline-\(name)-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        folders.append(folder)
        return folder.path
    }

    private func manager(limit: Int = TerminalLimits.sessionsPerProject) -> TerminalSessionManager {
        TerminalSessionManager(sessionsPerProject: limit) {
            TerminalLaunch(executable: "/bin/sh", execName: "sh", environment: [
                "PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "TERM": "xterm-256color", "PS1": "$ ",
                "HOME": NSTemporaryDirectory(), "LANG": "en_US.UTF-8",
            ])
        }
    }

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

    func testOpensATerminalWhenTheProjectHasNone() async throws {
        let manager = manager()
        manager.setProject(try makeFolder("open"))
        let session = try manager.run("echo HELLO$((1 + 1))").get()
        defer { manager.close(session.id, wait: true) }
        XCTAssertEqual(manager.projectSessions.map(\.id), [session.id])
        let output = capture(session)
        try await waitUntil("command output") { output.text.contains("HELLO2") }
    }

    func testRunsInTheSelectedTerminalKeepingItsState() async throws {
        let manager = manager()
        manager.setProject(try makeFolder("state"))
        let first = try XCTUnwrap(manager.newSession())
        let second = try XCTUnwrap(manager.newSession())
        defer { manager.close(first.id, wait: true); manager.close(second.id, wait: true) }
        manager.select(first.id)
        let output = capture(first)
        first.send(text: "MARK=kept\n")
        XCTAssertEqual(try manager.run("echo \"$MARK$((2 + 2))\"").get().id, first.id)
        try await waitUntil("the shell's own variable") { output.text.contains("kept4") }
    }

    func testNeverUsesAnotherProjectsTerminal() async throws {
        let manager = manager()
        manager.setProject(try makeFolder("a"))
        let a = try XCTUnwrap(manager.newSession())
        manager.setProject(try makeFolder("b"))
        let b = try manager.run("true").get()
        defer { manager.close(a.id, wait: true); manager.close(b.id, wait: true) }
        XCTAssertNotEqual(a.id, b.id)
        XCTAssertEqual(b.project, manager.project)
    }

    func testRestartsAnExitedShell() async throws {
        let manager = manager()
        manager.setProject(try makeFolder("exited"))
        let session = try XCTUnwrap(manager.newSession())
        defer { manager.close(session.id, wait: true) }
        session.send(text: "exit 3\n")
        try await waitUntil("the shell to exit") { !session.isRunning }
        XCTAssertEqual(try manager.run("true").get().id, session.id)
        XCTAssertTrue(session.isRunning)
        XCTAssertEqual(session.launchCount, 2)
    }

    func testRefusesATerminalHeldByAProgram() async throws {
        let manager = manager()
        manager.setProject(try makeFolder("busy"))
        let session = try XCTUnwrap(manager.newSession())
        defer { manager.close(session.id, wait: true) }
        session.send(text: "sleep 30\n")
        try await waitUntil("sleep to hold the terminal") { session.hasForegroundProgram }
        XCTAssertEqual(manager.run("ls").failure, .busy(session.displayTitle))
    }

    func testNoTerminalAtTheLimit() throws {
        let manager = manager(limit: 0)
        manager.setProject(try makeFolder("limit"))
        XCTAssertEqual(manager.run("ls").failure, .noTerminal)
    }
}

private extension Result {
    var failure: Failure? {
        if case .failure(let error) = self { return error }
        return nil
    }
}
