import Foundation
import XCTest
@testable import RustCodeTerminal

final class TerminalLaunchTests: XCTestCase {
    private let home = URL(fileURLWithPath: "/Users/someone")

    func testUsesTheUsersShellAsALoginShell() {
        let launch = TerminalLaunch.loginShell(environment: ["SHELL": "/opt/homebrew/bin/fish"], homeDirectory: home,
                                               accountShell: "/bin/zsh", isExecutable: { _ in true })
        XCTAssertEqual(launch.executable, "/opt/homebrew/bin/fish")
        XCTAssertEqual(launch.execName, "-fish")
        XCTAssertEqual(launch.environment["SHELL"], "/opt/homebrew/bin/fish")
    }

    func testFallsBackToTheAccountShellThenZsh() {
        let account = TerminalLaunch.loginShell(environment: ["SHELL": "/missing/bash"], homeDirectory: home,
                                                accountShell: "/bin/bash", isExecutable: { $0 != "/missing/bash" })
        XCTAssertEqual(account.executable, "/bin/bash")
        let fallback = TerminalLaunch.loginShell(environment: [:], homeDirectory: home, accountShell: nil,
                                                 isExecutable: { $0 == "/bin/zsh" })
        XCTAssertEqual(fallback.executable, "/bin/zsh")
        XCTAssertEqual(fallback.execName, "-zsh")
    }

    func testEnvironmentMatchesTheEnginesPlusTerminalSettings() {
        let environment = TerminalLaunch.terminalEnvironment(
            base: ["PATH": "/usr/bin:/bin", "PWD": "/", "SHLVL": "1", "OPENAI_API_KEY": "kept", "TERM": "dumb"],
            homeDirectory: home, shell: "/bin/zsh")
        let path = environment["PATH"]?.split(separator: ":").map(String.init) ?? []
        XCTAssertEqual(Array(path.prefix(2)), ["/usr/bin", "/bin"], "the app's PATH comes first, as for the engine")
        XCTAssertTrue(path.contains("/opt/homebrew/bin"))
        XCTAssertTrue(path.contains("/Users/someone/.cargo/bin"))
        XCTAssertEqual(environment["TERM"], "xterm-256color")
        XCTAssertEqual(environment["COLORTERM"], "truecolor")
        XCTAssertEqual(environment["TERM_PROGRAM"], "RustCode")
        XCTAssertEqual(environment["LANG"], "en_US.UTF-8", "a Finder launch has no locale")
        XCTAssertEqual(environment["HOME"], "/Users/someone")
        XCTAssertEqual(environment["OPENAI_API_KEY"], "kept")
        XCTAssertNil(environment["PWD"])
        XCTAssertNil(environment["SHLVL"])
    }

    func testKeepsTheUsersLocale() {
        let environment = TerminalLaunch.terminalEnvironment(base: ["LC_ALL": "nb_NO.UTF-8"], homeDirectory: home,
                                                            shell: "/bin/zsh")
        XCTAssertNil(environment["LANG"])
        XCTAssertEqual(environment["LC_ALL"], "nb_NO.UTF-8")
    }

    func testEnvironmentListSetsTheWorkingDirectory() {
        let launch = TerminalLaunch(executable: "/bin/sh", environment: ["B": "2", "A": "1", "PWD": "/"])
        XCTAssertEqual(launch.environmentList(currentDirectory: "/tmp/project"), ["A=1", "B=2", "PWD=/tmp/project"])
    }

    func testProjectPathsAreStandardized() {
        XCTAssertEqual(TerminalSessionManager.standardize("/tmp/a/../b/"), "/tmp/b")
        XCTAssertEqual(TerminalSessionManager.standardize("/"), "/")
    }
}
