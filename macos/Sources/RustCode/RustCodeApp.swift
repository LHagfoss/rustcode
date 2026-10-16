import AppKit
import SwiftUI
import RustCodeKit

@main
struct RustCodeApp: App {
    @StateObject private var model = DesktopModel()

    init() {
        // A write to the engine's stdin after it exits must surface as an
        // error, not kill the app with the default SIGPIPE action.
        signal(SIGPIPE, SIG_IGN)
        AppearanceSetting.apply(UserDefaults.standard.string(forKey: AppearanceSetting.key))
    }

    var body: some Scene {
        WindowGroup {
            ContentView(model: model)
                .frame(minWidth: 850, minHeight: 620)
                .task { await model.start() }
                .task { await model.updater.checkOnLaunch() }
                .onDisappear { model.stop() }
        }
        .windowStyle(.hiddenTitleBar)
        .commands {
            CommandGroup(replacing: .newItem) {
                Button("New Chat") { Task { await model.newSession() } }
                    .keyboardShortcut("n", modifiers: .command)
            }
            CommandGroup(after: .appInfo) {
                Button("Check for Updates…") {
                    model.settingsSection = .about
                    model.showsSettings = true
                    Task { await model.updater.check() }
                }
                Button("Choose Project…") { model.chooseWorkspace() }
                    .keyboardShortcut("o", modifiers: .command)
            }
            // Settings open as a sheet in the main window, not a window of their own.
            CommandGroup(replacing: .appSettings) {
                Button("Settings…") { model.settingsSection = .general; model.showsSettings = true }
                    .keyboardShortcut(",", modifiers: .command)
            }
            CommandGroup(after: .textEditing) {
                Button("Find in Chat…") { model.showsFind = true }
                    .keyboardShortcut("f", modifiers: .command)
                Button("Find Next") { model.findNext() }
                    .keyboardShortcut("g", modifiers: .command)
                Button("Find Previous") { model.findNext(-1) }
                    .keyboardShortcut("g", modifiers: [.command, .shift])
            }
            CommandGroup(before: .sidebar) {
                Button(model.showsSidebar ? "Hide Sidebar" : "Show Sidebar") { model.toggleSidebar() }
                    .keyboardShortcut("s", modifiers: [.command, .control])
            }
        }
    }
}

/// The panel open above the composer.
enum ComposerPanel { case add, model }

@MainActor
final class DesktopModel: ObservableObject {
    let updater = Updater()
    @Published var composerPanel: ComposerPanel?
    /// Slash commands the engine advertised, and the highlighted suggestion.
    @Published var commands: [SlashCommand] = []
    @Published var commandSelection = 0
    /// The Settings section to show when the sheet opens.
    @Published var settingsSection: SettingsSection = .general
    @Published var transcript = Transcript()
    @Published var sessions: [SessionSummary] = []
    @Published var selectedSession: String?
    @Published var workspace: URL {
        didSet { UserDefaults.standard.set(workspace.path, forKey: "workspacePath") }
    }
    @Published var draft = ""
    @Published var showsSidebar = true
    @Published var showsSettings = false
    /// ⌘F: the find bar over the chat, its text and which match is current.
    @Published var showsFind = false
    @Published var findQuery = "" { didSet { findIndex = 0 } }
    @Published var findIndex = 0
    /// The sidebar lists chats from every project, or only the open one.
    @Published var showsAllProjects = UserDefaults.standard.object(forKey: "showsAllProjects") as? Bool ?? true {
        didSet {
            UserDefaults.standard.set(showsAllProjects, forKey: "showsAllProjects")
            Task { await refreshSessions() }
        }
    }
    /// Set by /history to put the cursor in the chat search.
    @Published var focusSessionSearch = false
    /// Filters the sidebar's chats by title.
    @Published var sessionFilter = ""
    /// Files and folders added with + or by dropping them on the composer. The
    /// engine takes text prompts, so they are sent as paths for the agent to read.
    @Published var attachments: [URL] = []
    @Published var busy = false
    @Published var error: String?
    /// Every unanswered permission or question request, oldest first. The
    /// engine blocks until each one is answered, so none may be dropped.
    @Published var permissionQueue: [PermissionRequest] = []
    @Published var models = ModelOptions.empty
    @Published var selectedModel: String?
    /// The engine binary in use and where it was found, shown in Settings.
    @Published private(set) var engineLocation: EngineLocation?
    private var engine: EngineProcess?
    /// The session whose prompt turn is running; Stop cancels this one even
    /// after the user switches to another session.
    @Published private(set) var turnSession: String?
    private var inboundTask: Task<Void, Never>?
    private var capabilities: AgentInfo?

    init() {
        let savedPath = UserDefaults.standard.string(forKey: "workspacePath")
        // An app opened from Finder or the Dock starts in `/`, which is no project.
        let launchPath = FileManager.default.currentDirectoryPath
        workspace = savedPath.map { URL(fileURLWithPath: $0, isDirectory: true) }
            ?? (launchPath == "/" ? FileManager.default.homeDirectoryForCurrentUser
                : URL(fileURLWithPath: launchPath, isDirectory: true))
    }

    func start() async {
        guard engine == nil else { return }
        do {
            let location = try EngineLocator(userSetting: UserDefaults.standard.string(forKey: "enginePath")).locate()
            engineLocation = location
            var launched: EngineProcess?
            let process = EngineProcess(executable: location.url, workingDirectory: workspace) { [weak self] exit in
                Task { @MainActor in
                    // A late exit of an engine replaced by a project switch
                    // must not touch the state of the one now running.
                    guard let self, let launched, self.engine === launched else { return }
                    self.busy = false
                    self.turnSession = nil
                    guard !exit.requested else { return }
                    // Drop the dead engine so the next `start()` launches a
                    // new one; its waiting requests can no longer be answered.
                    self.inboundTask?.cancel()
                    self.inboundTask = nil
                    self.engine = nil
                    self.capabilities = nil
                    self.permissionQueue = []
                    self.error = "RustCode engine exited (status \(exit.status))."
                }
            }
            launched = process
            try process.start()
            engine = process
            inboundTask = Task { [weak self] in
                for await inbound in process.connection.inbound {
                    await self?.handle(inbound)
                }
            }
            let result = try await process.connection.request(ACPMethod.initialize, params: ACPRequests.initialize(appVersion: "0.1.0"), timeout: .seconds(10))
            capabilities = try AgentInfo(initializeResult: result)
            try await process.connection.notify("initialized", params: [:])
            await refreshSessions()
            if selectedSession == nil { await newSession() }
        } catch { self.error = String(describing: error) }
    }

    func chooseWorkspace() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = false
        if panel.runModal() == .OK, let url = panel.url {
            workspace = url
            Task { await restartForWorkspace() }
        }
    }

    var findMatches: [Int] { transcript.matches(findQuery) }

    /// The transcript item the find bar is on, if any matched.
    var currentMatch: Int? {
        let matches = findMatches
        return matches.isEmpty ? nil : matches[min(findIndex, matches.count - 1)]
    }

    /// Step through the matches, wrapping at either end.
    func findNext(_ step: Int = 1) {
        let count = findMatches.count
        guard count > 0 else { return }
        showsFind = true
        findIndex = ((findIndex + step) % count + count) % count
    }

    var visibleSessions: [SessionSummary] {
        let filter = sessionFilter.trimmingCharacters(in: .whitespaces)
        guard !filter.isEmpty else { return sessions }
        return sessions.filter {
            "\($0.title ?? "New chat") \($0.projectName)".range(of: filter, options: [.caseInsensitive, .diacriticInsensitive]) != nil
        }
    }

    /// Commands matching the draft while its first word is being typed.
    var commandSuggestions: [SlashCommand] { SlashCommand.suggestions(for: draft, in: commands) }

    /// Put the highlighted command in the composer.
    func completeCommand() {
        let suggestions = commandSuggestions
        guard !suggestions.isEmpty else { return }
        draft = suggestions[min(commandSelection, suggestions.count - 1)].insertion
        commandSelection = 0
    }

    func moveCommandSelection(_ step: Int) {
        let count = commandSuggestions.count
        guard count > 0 else { return }
        commandSelection = ((commandSelection + step) % count + count) % count
    }

    /// The commands the app answers itself rather than the engine.
    private func perform(_ command: SlashCommand) async {
        switch command.action {
        case .newSession: await newSession()
        case .modelPicker: composerPanel = .model
        case .sessionPicker:
            if !showsSidebar { toggleSidebar() }
            focusSessionSearch = true
        case .copyResponse:
            let last = transcript.items.last { if case .assistant = $0.content { true } else { false } }
            if case .assistant(let text) = last?.content { Clipboard.copy(text) }
        case .execute: break
        }
    }

    func toggleSidebar() {
        withAnimation(.snappy(duration: 0.25)) { showsSidebar.toggle() }
    }

    func chooseAttachments() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = true
        panel.allowsMultipleSelection = true
        panel.directoryURL = workspace
        panel.prompt = "Attach"
        if panel.runModal() == .OK { attach(panel.urls) }
    }

    func attach(_ urls: [URL]) {
        for url in urls where !attachments.contains(url) { attachments.append(url) }
    }

    /// The typed message followed by the attached paths, relative to the project
    /// when they are inside it.
    static func prompt(_ text: String, attaching files: [URL], in workspace: URL) -> String {
        guard !files.isEmpty else { return text }
        let root = workspace.standardizedFileURL.path
        let paths = files.map { url -> String in
            let path = url.standardizedFileURL.path
            return path.hasPrefix(root + "/") ? String(path.dropFirst(root.count + 1)) : path
        }
        let list = paths.map { "- `\($0)`" }.joined(separator: "\n")
        return (text.isEmpty ? "" : text + "\n\n") + "Attached files:\n" + list
    }

    /// Start a fresh engine, e.g. after its path changed in Settings.
    func restartEngine() async {
        error = nil
        await restartForWorkspace()
    }

    private func restartForWorkspace() async {
        stop()
        transcript.removeAll()
        sessions = []
        selectedSession = nil
        await start()
    }

    func newSession() async {
        guard let connection = engine?.connection else { return }
        do {
            let result = try await connection.request(ACPMethod.sessionNew, params: ACPRequests.newSession(cwd: workspace), timeout: .seconds(30))
            guard let id = result["sessionId"]?.stringValue else { throw ACPProtocolError.malformed("session/new has no sessionId") }
            selectedSession = id
            transcript.removeAll()
            applyConfig(result["configOptions"])
            await refreshSessions()
        } catch { self.error = String(describing: error) }
    }

    func refreshSessions() async {
        guard let connection = engine?.connection, capabilities?.listSessions == true else { return }
        // The engine pages `session/list`; read every page, up to a bound.
        do {
            var all: [SessionSummary] = []
            var cursor: String?
            for _ in 0..<40 {
                let params = ACPRequests.listSessions(cwd: showsAllProjects ? nil : workspace, cursor: cursor)
                let result = try await connection.request(ACPMethod.sessionList, params: params, timeout: .seconds(15))
                all += SessionSummary.list(from: result)
                cursor = SessionSummary.nextCursor(from: result)
                if cursor == nil { break }
            }
            sessions = all
        } catch { self.error = String(describing: error) }
    }

    /// The request shown for the selected session, if any.
    var pendingPermission: PermissionRequest? {
        permissionQueue.first { $0.sessionId == selectedSession }
    }

    func select(_ session: SessionSummary) async {
        guard let connection = engine?.connection, capabilities?.loadSession == true else { return }
        // `session/load` replays the transcript as `session/update`
        // notifications before it answers, so select the session first or
        // the replay is filtered out as belonging to another session.
        selectedSession = session.id
        transcript.removeAll()
        do {
            let result = try await connection.request(ACPMethod.sessionLoad, params: ACPRequests.loadSession(id: session.id, cwd: URL(fileURLWithPath: session.cwd)), timeout: .seconds(30))
            // Another session may have been picked while this one loaded.
            guard selectedSession == session.id else { return }
            applyConfig(result["configOptions"])
        } catch { self.error = String(describing: error) }
    }

    func submit() async {
        guard let connection = engine?.connection, let id = selectedSession else { return }
        let typed = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !typed.isEmpty || !attachments.isEmpty, !busy else { return }
        if let command = SlashCommand.invoked(by: typed, in: commands), command.action != .execute {
            draft = ""
            await perform(command)
            return
        }
        let text = Self.prompt(typed, attaching: attachments, in: workspace)
        draft = ""
        attachments = []
        transcript.appendUser(text)
        busy = true
        turnSession = id
        defer {
            busy = false
            turnSession = nil
            // The turn is over, so the engine no longer waits on its requests.
            permissionQueue.removeAll { $0.sessionId == id }
        }
        do {
            _ = try await connection.request(ACPMethod.sessionPrompt, params: ACPRequests.prompt(sessionId: id, text: text))
            await refreshSessions()
        } catch { self.error = String(describing: error) }
    }

    func cancel() async {
        guard let connection = engine?.connection, let id = turnSession ?? selectedSession else { return }
        do {
            try await connection.notify(ACPMethod.sessionCancel, params: ACPRequests.session(id))
            // ACP: after `session/cancel` the client answers the session's
            // outstanding permission requests as cancelled.
            let waiting = permissionQueue.filter { $0.sessionId == id }
            permissionQueue.removeAll { $0.sessionId == id }
            for request in waiting {
                try await connection.respond(id: request.rpcId, result: ACPRequests.permissionCancelled)
            }
        } catch { self.error = String(describing: error) }
    }

    /// Answer exactly `request`, the one whose card was clicked. It leaves the
    /// queue before the reply is sent, so a double click cannot answer the
    /// next request with this one's option.
    func answer(_ request: PermissionRequest, with option: PermissionOption?) async {
        await reply(to: request, with: option.map { ACPRequests.permissionSelected(optionId: $0.id) } ?? ACPRequests.permissionCancelled)
    }

    /// Answer a multi-select question with every ticked option.
    func answer(_ request: PermissionRequest, choosing options: [PermissionOption]) async {
        guard !options.isEmpty else { return }
        await reply(to: request, with: ACPRequests.questionOptions(options))
    }

    /// Answer a question with free text.
    func answer(_ request: PermissionRequest, text: String) async {
        let text = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, let carrier = request.answerOptions.first else { return }
        await reply(to: request, with: ACPRequests.questionText(text, carrier: carrier))
    }

    private func reply(to request: PermissionRequest, with result: JSONValue) async {
        guard let connection = engine?.connection,
              let index = permissionQueue.firstIndex(where: { $0.id == request.id }) else { return }
        permissionQueue.remove(at: index)
        do { try await connection.respond(id: request.rpcId, result: result) }
        catch { self.error = String(describing: error) }
    }

    func setModel(_ model: String) async {
        guard let connection = engine?.connection, let session = selectedSession else { return }
        do {
            _ = try await connection.request(ACPMethod.sessionSetConfigOption, params: ACPRequests.setModel(sessionId: session, value: model), timeout: .seconds(15))
            selectedModel = model
        } catch { self.error = String(describing: error) }
    }

    func stop() {
        inboundTask?.cancel()
        inboundTask = nil
        if let engine { DispatchQueue.global().async { engine.stop() } }
        engine = nil
        capabilities = nil
        selectedSession = nil
        // The engine is being stopped, so its waiting requests die with it.
        permissionQueue = []
        busy = false
    }

    private func applyConfig(_ value: JSONValue?) {
        guard let value else { return }
        models = ModelOptions(configOptions: value)
        selectedModel = models.current
    }

    private func handle(_ inbound: ACPInbound) async {
        switch inbound {
        case .closed(let reason):
            busy = false
            if engine != nil { error = reason }
        case .notification(let method, let params) where method == ACPMethod.sessionUpdate
            && params["update"]?["sessionUpdate"]?.stringValue == "available_commands_update":
            // Commands are the same for every session, so take them from any.
            if case .availableCommands(let list) = SessionUpdate(notificationParams: params) { commands = list }
        case .notification(let method, let params) where method == ACPMethod.sessionUpdate:
            guard params["sessionId"]?.stringValue == selectedSession else { return }
            let update = SessionUpdate(notificationParams: params)
            switch update {
            case .title: await refreshSessions()
            case .configOptions(let options): applyConfig(options)
            default: transcript.apply(update)
            }
        case .request(let id, let method, let params) where method == ACPMethod.requestPermission:
            if let request = PermissionRequest(rpcId: id, params: params) { permissionQueue.append(request) }
            else { try? await engine?.connection.respond(id: id, error: .invalidParams(method)) }
        case .request(let id, let method, _):
            try? await engine?.connection.respond(id: id, error: .methodNotFound(method))
        default: break
        }
    }
}
