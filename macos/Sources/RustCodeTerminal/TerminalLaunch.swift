import Darwin
import Foundation
import RustCodeKit

/// How a workspace terminal's shell starts: the user's own login shell, with
/// the environment the app gives the engine.
///
/// The shell runs as the user, directly on a PTY. It is not an agent tool: it
/// does not go through the engine, its sandbox or its permission prompts, and
/// nothing typed or printed in it is sent to the agent.
public struct TerminalLaunch: Equatable, Sendable {
    public var executable: String
    public var arguments: [String]
    /// `argv[0]`. A leading `-` starts a login shell, as Terminal.app does,
    /// so the user's profile sets up the same `PATH` and keys as their own
    /// terminal.
    public var execName: String?
    public var environment: [String: String]

    public init(executable: String, arguments: [String] = [], execName: String? = nil, environment: [String: String]) {
        self.executable = executable
        self.arguments = arguments
        self.execName = execName
        self.environment = environment
    }

    /// The user's login shell: `$SHELL`, else the account's shell, else zsh.
    public static func loginShell(
        environment: [String: String] = ProcessInfo.processInfo.environment,
        homeDirectory: URL = FileManager.default.homeDirectoryForCurrentUser,
        accountShell: String? = TerminalLaunch.accountShell(),
        isExecutable: (String) -> Bool = EngineLocator.isExecutableFile
    ) -> TerminalLaunch {
        let shell = [environment["SHELL"], accountShell, "/bin/zsh"]
            .compactMap { $0 }
            .first { $0.hasPrefix("/") && isExecutable($0) } ?? "/bin/sh"
        return TerminalLaunch(
            executable: shell,
            execName: "-" + (shell as NSString).lastPathComponent,
            environment: terminalEnvironment(base: environment, homeDirectory: homeDirectory, shell: shell))
    }

    /// The engine's environment (`EngineLocator.engineEnvironment`, which adds
    /// the usual tool directories a Finder-launched app lacks) plus what a
    /// terminal emulator sets, without the app's own launch details.
    public static func terminalEnvironment(base: [String: String], homeDirectory: URL, shell: String) -> [String: String] {
        var environment = EngineLocator(userSetting: nil, environment: base, homeDirectory: homeDirectory)
            .engineEnvironment()
        // Describe how the app itself was started, not this shell.
        for key in ["PWD", "OLDPWD", "SHLVL", "_", "TERM_SESSION_ID", "ITERM_SESSION_ID",
                    "TERM_PROGRAM_VERSION", "LC_TERMINAL", "LC_TERMINAL_VERSION"] {
            environment[key] = nil
        }
        environment["TERM"] = "xterm-256color"
        environment["COLORTERM"] = "truecolor"
        environment["TERM_PROGRAM"] = "RustCode"
        environment["SHELL"] = shell
        environment["HOME"] = environment["HOME"] ?? homeDirectory.path
        // A Finder-launched app has no locale, and without one shells and
        // editors fall back to ASCII.
        let locale = ["LC_ALL", "LC_CTYPE", "LANG"].compactMap { environment[$0] }.first { !$0.isEmpty }
        if locale == nil { environment["LANG"] = "en_US.UTF-8" }
        return environment
    }

    /// `KEY=value` pairs for `execve`, with `PWD` set to where the shell starts.
    public func environmentList(currentDirectory: String) -> [String] {
        var environment = self.environment
        environment["PWD"] = currentDirectory
        return environment.keys.sorted().map { "\($0)=\(environment[$0]!)" }
    }

    /// The login shell in the user's account record.
    public static func accountShell() -> String? {
        guard let entry = getpwuid(getuid()), let shell = entry.pointee.pw_shell else { return nil }
        let path = String(cString: shell)
        return path.isEmpty ? nil : path
    }
}
