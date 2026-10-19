import Foundation

/// The workspace terminals, grouped by project.
///
/// Every session is bound to the project it was opened in. Only the open
/// project's sessions are listed and can be selected, so a project switch
/// never sends input to, or starts a shell in, another project's folder;
/// the other project's shells keep running until closed. Hiding the panel
/// changes nothing here: only `close` and `terminateAll` end a shell.
@MainActor
public final class TerminalSessionManager: ObservableObject {
    /// All sessions, of every project, in the order they were opened.
    @Published public private(set) var sessions: [TerminalSession] = []
    /// The open project, standardized.
    @Published public private(set) var project: String?
    @Published private var selections: [String: TerminalSession.ID] = [:]

    private let makeLaunch: () -> TerminalLaunch
    private let sessionLimit: Int
    private var opened: [String: Int] = [:]

    public init(sessionsPerProject: Int = TerminalLimits.sessionsPerProject,
                launch: @escaping () -> TerminalLaunch = { TerminalLaunch.loginShell() }) {
        self.makeLaunch = launch
        self.sessionLimit = sessionsPerProject
    }

    public nonisolated static func standardize(_ path: String) -> String {
        let standardized = URL(fileURLWithPath: path, isDirectory: true).standardizedFileURL.path
        return standardized.count > 1 && standardized.hasSuffix("/") ? String(standardized.dropLast()) : standardized
    }

    /// Show another project's terminals. Nothing is started or stopped.
    public func setProject(_ path: String) {
        let path = Self.standardize(path)
        if path != project { project = path }
    }

    /// The open project's sessions.
    public var projectSessions: [TerminalSession] {
        guard let project else { return [] }
        return sessions.filter { $0.project == project }
    }

    /// Shells still running in projects other than the open one.
    public var runningElsewhere: [TerminalSession] {
        sessions.filter { $0.project != project && $0.isRunning }
    }

    public var selectedSession: TerminalSession? {
        let list = projectSessions
        guard let project else { return nil }
        return list.first { $0.id == selections[project] } ?? list.last
    }

    public var canOpenSession: Bool {
        project != nil && projectSessions.count < sessionLimit
    }

    /// Open a terminal in the open project and select it; nil at the limit.
    @discardableResult
    public func newSession() -> TerminalSession? {
        guard let project, canOpenSession else { return nil }
        let ordinal = (opened[project] ?? 0) + 1
        opened[project] = ordinal
        let session = TerminalSession(project: project, ordinal: ordinal, launch: makeLaunch())
        sessions.append(session)
        selections[project] = session.id
        session.start()
        return session
    }

    /// The project's selected terminal, opening one if it has none.
    @discardableResult
    public func ensureSession() -> TerminalSession? {
        selectedSession ?? newSession()
    }

    /// Select one of the open project's sessions; others are refused.
    public func select(_ id: TerminalSession.ID) {
        guard let project, projectSessions.contains(where: { $0.id == id }) else { return }
        selections[project] = id
    }

    /// Restart an exited shell of the open project.
    public func relaunch(_ id: TerminalSession.ID) {
        projectSessions.first { $0.id == id }?.relaunch()
    }

    /// End a session's shell and processes and remove it. With `wait`, returns
    /// once they are gone.
    public func close(_ id: TerminalSession.ID, wait: Bool = false) {
        guard let index = sessions.firstIndex(where: { $0.id == id }) else { return }
        let session = sessions[index]
        let siblings = sessions.filter { $0.project == session.project }
        let position = siblings.firstIndex { $0.id == id } ?? 0
        sessions.remove(at: index)
        session.output = nil
        session.terminate(wait: wait)
        if selections[session.project] == id {
            let rest = sessions.filter { $0.project == session.project }
            selections[session.project] = rest.isEmpty ? nil : rest[min(position, rest.count - 1)].id
        }
    }

    /// The app is quitting: end every shell and its processes, waiting at
    /// most `grace` in all before killing what is left.
    public func terminateAll(grace: TimeInterval = TerminalLimits.terminationGrace) {
        let deadline = Date().addingTimeInterval(grace)
        let terminations = sessions.compactMap { $0.beginTermination() }
        for termination in terminations { termination.finish(by: deadline) }
    }
}
