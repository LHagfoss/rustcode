import AppKit
import Combine
import RustCodeKit
import RustCodeTerminal
import SwiftTerm
import SwiftUI

/// The workspace terminal panel: the project's shells and the views that
/// show them.
///
/// The terminal is the user's own shell, not an agent tool. It never touches
/// the engine: what is typed goes straight to the PTY, without the engine's
/// sandbox or permission prompts, and nothing it prints reaches the agent.
/// The agent's shell commands still go through the engine and its approvals.
/// The preview is separate too; a local link clicked here only asks it to
/// load that URL.
@MainActor
final class TerminalController: ObservableObject {
    let manager: TerminalSessionManager
    @Published private(set) var isShown = false
    /// Bumped to move keyboard focus to the selected terminal.
    @Published private(set) var focusRequest = 0
    /// Shows a local URL clicked in a terminal in the localhost preview.
    var openInPreview: ((URL) -> Void)?

    private var views: [TerminalSession.ID: SessionTerminalView] = [:]
    private var managerChanges: AnyCancellable?
    private var quitObserver: NSObjectProtocol?

    init(manager: TerminalSessionManager? = nil) {
        let manager = manager ?? TerminalSessionManager()
        self.manager = manager
        managerChanges = manager.objectWillChange.sink { [weak self] _ in self?.objectWillChange.send() }
        // Quitting ends every shell and what runs in it, so no dev server or
        // watcher outlives the app.
        quitObserver = NotificationCenter.default.addObserver(
            forName: NSApplication.willTerminateNotification, object: nil, queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated { self?.manager.terminateAll() }
        }
    }

    /// Show another project's terminals. The previous project's shells keep
    /// running and stay with that project.
    func setWorkspace(_ path: String) {
        manager.setProject(path)
    }

    /// ⇧⌘T: show or hide the panel. Hiding never stops a shell.
    func toggle() {
        if isShown { hide() } else { open() }
    }

    /// `/terminal`: show the panel and focus the project's terminal, opening
    /// one if it has none.
    func open() {
        withAnimation(.snappy(duration: 0.25)) { isShown = true }
        manager.ensureSession()
        focusRequest += 1
    }

    /// A `! command` from the composer: shown in the panel and typed into
    /// the open project's selected terminal. Focus stays in the composer.
    func run(_ command: String) -> Result<TerminalSession, InlineCommandError> {
        if !isShown { withAnimation(.snappy(duration: 0.25)) { isShown = true } }
        return manager.run(command)
    }

    func hide() {
        withAnimation(.snappy(duration: 0.25)) { isShown = false }
    }

    func newSession() {
        if !isShown { withAnimation(.snappy(duration: 0.25)) { isShown = true } }
        manager.newSession()
        focusRequest += 1
    }

    func select(_ session: TerminalSession) {
        manager.select(session.id)
        focusRequest += 1
    }

    func relaunch(_ session: TerminalSession) {
        // Leave the last program's colors and modes behind.
        views[session.id]?.feed(text: "\u{1b}[0m\r\n")
        manager.relaunch(session.id)
        focusRequest += 1
    }

    /// The tab's close button. Asks first while something other than the
    /// shell is running in it, as Terminal does.
    func requestClose(_ session: TerminalSession) {
        if session.hasRunningChildren {
            let alert = NSAlert()
            alert.messageText = "Close this terminal?"
            alert.informativeText = "Processes are still running in it. Closing the terminal ends them."
            alert.addButton(withTitle: "Close")
            alert.addButton(withTitle: "Cancel")
            guard alert.runModal() == .alertFirstButtonReturn else { return }
        }
        close(session)
    }

    func close(_ session: TerminalSession) {
        views.removeValue(forKey: session.id)?.removeFromSuperview()
        manager.close(session.id)
    }

    /// The emulator showing a session. It lives as long as the session, so
    /// its screen and scrollback survive hiding the panel and switching
    /// projects.
    func view(for session: TerminalSession) -> SessionTerminalView {
        if let view = views[session.id] { return view }
        let view = SessionTerminalView(session: session) { [weak self] url in self?.openLink(url) }
        views[session.id] = view
        return view
    }

    /// Local pages open in the preview, other web pages in the browser;
    /// nothing else printed in a terminal is opened.
    private func openLink(_ url: URL) {
        if PreviewURL.isLocal(url), let openInPreview {
            openInPreview(url)
        } else if let scheme = url.scheme?.lowercased(), scheme == "http" || scheme == "https" {
            NSWorkspace.shared.open(url)
        }
    }
}

/// A SwiftTerm emulator bound to one session: keystrokes go to its PTY, its
/// output is drawn here, and the view's size is the PTY's.
final class SessionTerminalView: TerminalView, TerminalViewDelegate {
    let session: TerminalSession
    private let openLink: (URL) -> Void

    init(session: TerminalSession, openLink: @escaping (URL) -> Void) {
        self.session = session
        self.openLink = openLink
        super.init(frame: NSRect(x: 0, y: 0, width: 640, height: 240),
                   font: NSFont.monospacedSystemFont(ofSize: 12, weight: .regular),
                   options: TerminalLimits.emulatorOptions(columns: session.columns, rows: session.rows))
        terminalDelegate = self
        // Option types characters (|, @, [ ] on many layouts) instead of
        // acting as Meta.
        optionAsMetaKey = false
        nativeBackgroundColor = .textBackgroundColor
        nativeForegroundColor = .textColor
        setAccessibilityLabel("Terminal in \((session.project as NSString).lastPathComponent)")
        session.output = { [weak self] bytes in self?.feed(byteArray: bytes) }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }

    func sizeChanged(source: TerminalView, newCols: Int, newRows: Int) {
        session.resize(columns: newCols, rows: newRows)
    }

    func setTerminalTitle(source: TerminalView, title: String) {
        session.title = title
    }

    func hostCurrentDirectoryUpdate(source: TerminalView, directory: String?) {
        session.reportedDirectory = directory.flatMap { URL(string: $0)?.path ?? $0 }
    }

    func send(source: TerminalView, data: ArraySlice<UInt8>) {
        session.send(data)
    }

    func scrolled(source: TerminalView, position: Double) {}

    func rangeChanged(source: TerminalView, startY: Int, endY: Int) {}

    func requestOpenLink(source: TerminalView, link: String, params: [String: String]) {
        if let url = URL(string: link) { openLink(url) }
    }

    /// OSC 52 from a program in the terminal, as iTerm and Terminal allow.
    func clipboardCopy(source: TerminalView, content: Data) {
        guard let text = String(data: content, encoding: .utf8) else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
    }
}

/// View > Show Terminal (⇧⌘T) and New Terminal Tab.
struct TerminalMenuItems: View {
    @ObservedObject var terminal: TerminalController

    var body: some View {
        Button(terminal.isShown ? "Hide Terminal" : "Show Terminal") { terminal.toggle() }
            .keyboardShortcut("t", modifiers: [.command, .shift])
        Button("New Terminal Tab") { terminal.newSession() }
            .disabled(!terminal.manager.canOpenSession)
    }
}
