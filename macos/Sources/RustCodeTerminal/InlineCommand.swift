import Foundation

/// A chat composer message that starts with `!`: a shell command the user
/// sends to the open project's terminal instead of to the agent.
///
/// Only the user's own composer is read this way. The agent never issues
/// these, and nothing printed in the terminal is sent back to it.
///
/// - `! ls` and `!ls` run `ls`.
/// - `!!` sends the rest as an ordinary chat message that starts with one
///   `!`: `!!important` sends `!important`.
/// - A lone `!` or a command spanning several lines is refused, so a
///   pasted script never runs line by line by accident.
public enum InlineCommand: Equatable, Sendable {
    /// Not a terminal command: send the text to the agent as it is.
    case message(String)
    /// Run this one-line command in the terminal.
    case command(String)
    case invalid(String)

    public static func parse(_ text: String) -> InlineCommand {
        guard text.hasPrefix("!") else { return .message(text) }
        if text.hasPrefix("!!") { return .message(String(text.dropFirst())) }
        let command = text.dropFirst().trimmingCharacters(in: .whitespacesAndNewlines)
        if command.isEmpty { return .invalid("Type a shell command after !, such as ! ls.") }
        if command.contains(where: \.isNewline) {
            return .invalid("A terminal command is one line. Start the message with !! to send it to the agent instead.")
        }
        return .command(command)
    }
}

/// Why a `!` command was not sent to a terminal.
public enum InlineCommandError: Error, Equatable, CustomStringConvertible {
    /// No project is open, or it has as many terminals as it may.
    case noTerminal
    /// The shell could not be started again.
    case shellUnavailable(String)
    /// A program such as a dev server or an editor holds the terminal.
    case busy(String)

    public var description: String {
        switch self {
        case .noTerminal:
            return "No terminal could be opened for this project."
        case .shellUnavailable(let terminal):
            return "The shell in \(terminal) could not be started."
        case .busy(let terminal):
            return "A program is running in \(terminal). Finish it or open a new terminal tab, then try again."
        }
    }
}

extension TerminalSessionManager {
    /// Run a one-line command in the open project's selected terminal, as if
    /// typed there, so it sees that shell's folder, environment and history.
    ///
    /// With no terminal one is opened, and an exited shell is started again.
    /// A terminal running a program is refused rather than typed into. A
    /// line the user had half typed in the shell is cleared first (⌃U, which
    /// zsh and bash keep for ⌃Y). Another project's terminals are never used.
    @discardableResult
    public func run(_ command: String) -> Result<TerminalSession, InlineCommandError> {
        guard let session = ensureSession() else { return .failure(.noTerminal) }
        if !session.isRunning {
            relaunch(session.id)
            guard session.isRunning else { return .failure(.shellUnavailable(session.displayTitle)) }
        }
        if session.hasForegroundProgram { return .failure(.busy(session.displayTitle)) }
        session.send(text: "\u{15}" + command + "\r")
        return .success(session)
    }
}
