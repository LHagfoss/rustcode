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
                .task { model.updater.startPolling() }
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
                    .keyboardShortcut("b", modifiers: .command)
            }
        }
    }
}

/// The panel open above the composer.
enum ComposerPanel { case add, model, effort }

@MainActor
final class DesktopModel: ObservableObject {
    let updater = Updater()
    @Published var composerPanel: ComposerPanel?
    /// The `/context` breakdown shown in its sheet.
    @Published var contextSheet: ContextBreakdown?
    /// Slash commands the engine advertised, and the highlighted suggestion.
    @Published var commands: [SlashCommand] = []
    @Published var commandSelection = 0
    /// A recognized slash command, shown as a chip in front of the draft.
    @Published var draftCommand: SlashCommand?
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
            scheduleContentSearch()
        }
    }
    /// False until the first session list arrives, so the sidebar can say it is loading.
    @Published private(set) var sessionsLoaded = false
    /// Chats pinned to the top of the sidebar, kept on this Mac.
    @Published private(set) var pinnedSessions = Set(UserDefaults.standard.stringArray(forKey: "pinnedSessions") ?? [])
    /// Chats hidden from the list without deleting them, kept on this Mac.
    @Published private(set) var archivedSessions = Set(UserDefaults.standard.stringArray(forKey: "archivedSessions") ?? [])
    @Published var showsArchived = false
    /// The chat whose title the rename sheet is editing.
    @Published var renamingSession: SessionSummary?
    /// Set by /history to put the cursor in the chat search.
    @Published var focusSessionSearch = false
    /// Filters the sidebar's chats by title, and by contents once it has
    /// `SessionContentSearch.minimumQueryLength` characters.
    @Published var sessionFilter = "" {
        didSet { if sessionFilter != oldValue { scheduleContentSearch() } }
    }
    /// Chats whose messages contain `contentQuery`, from the engine.
    @Published private(set) var contentMatches: [SessionContentMatch] = []
    private var contentQuery: String?
    private var contentSearchTask: Task<Void, Never>?
    /// Files and folders added with + or by dropping them on the composer. The
    /// engine takes text prompts, so they are sent as paths for the agent to read.
    @Published var attachments: [URL] = []
    /// Images pasted, dropped or attached; sent as ACP image blocks.
    @Published var images: [ImageAttachment] = []
    @Published var busy = false
    @Published var error: String?
    /// Every unanswered permission or question request, oldest first. The
    /// engine blocks until each one is answered, so none may be dropped.
    @Published var permissionQueue: [PermissionRequest] = [] {
        didSet { TurnNotifier.shared.setBadge(permissionQueue.count) }
    }
    @Published var models = ModelOptions.empty
    /// The model and effort new chats start with, shared with the terminal UI.
    @Published private(set) var modelDefaults: ModelDefaults?
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
        TurnNotifier.shared.openSession = { [weak self] id in
            guard let self, let session = self.sessions.first(where: { $0.id == id }) else { return }
            Task { await self.select(session) }
        }
        TurnNotifier.shared.start()
    }

    private func chatTitle(_ session: String) -> String {
        sessions.first { $0.id == session }?.title ?? "RustCode"
    }

    /// Tell the user a background chat finished, with the start of its reply.
    private func notifyFinished(_ session: String) {
        let items = session == selectedSession ? transcript.items : openSessions[session]?.transcript.items ?? []
        var reply = ""
        if let last = items.last(where: { if case .assistant = $0.content { true } else { false } }),
           case .assistant(let text) = last.content {
            reply = notificationSnippet(text)
        }
        TurnNotifier.shared.notify(session: session, title: chatTitle(session), body: reply.isEmpty ? "Finished." : reply)
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
            // Show the last known options at once, then read current ones: the
            // models and their effort levels change with config and engine.
            restoreLastConfig()
            await loadModelDefaults()
            if selectedSession == nil, modelDefaults == nil { await fetchModelOptions() }
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
        let shown = sessions.filter { archivedSessions.contains($0.id) == showsArchived }
        guard !filter.isEmpty else { return shown }
        let matches = contentQuery == SessionContentSearch.query(for: sessionFilter) ? contentMatches : []
        return SessionContentSearch.merge(shown: shown, filter: filter, contentMatches: matches)
    }

    /// The text around the search match of a chat found only by its contents,
    /// shown in place of its date.
    func searchSnippet(for session: SessionSummary) -> String? {
        let filter = sessionFilter.trimmingCharacters(in: .whitespaces)
        guard let query = SessionContentSearch.query(for: sessionFilter), query == contentQuery,
              !SessionContentSearch.titleMatches(session, filter: filter) else { return nil }
        return contentMatches.first { $0.id == session.id }?.snippet
    }

    /// Search chat contents for the filter after a short pause in typing.
    private func scheduleContentSearch() {
        contentSearchTask?.cancel()
        guard let query = SessionContentSearch.query(for: sessionFilter) else {
            contentSearchTask = nil
            contentQuery = nil
            contentMatches = []
            return
        }
        let arguments = SessionContentSearch.arguments(query: query, allProjects: showsAllProjects)
        contentSearchTask = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(250))
            guard !Task.isCancelled, let self else { return }
            let output = await self.runEngine(arguments)
            guard !Task.isCancelled else { return }
            self.contentMatches = output.flatMap { try? SessionContentSearch.parse($0) } ?? []
            self.contentQuery = query
        }
    }

    /// Commands matching the draft while its first word is being typed.
    var commandSuggestions: [SlashCommand] {
        draftCommand == nil ? SlashCommand.suggestions(for: draft, in: commands) : []
    }

    /// Turn the highlighted suggestion into the command chip.
    func completeCommand() {
        let suggestions = commandSuggestions
        guard !suggestions.isEmpty else { return }
        draftCommand = suggestions[min(commandSelection, suggestions.count - 1)]
        draft = ""
        commandSelection = 0
    }

    /// Typing a whole command name and a space makes it a chip.
    func promoteTypedCommand() {
        guard draftCommand == nil, draft.hasPrefix("/"), let space = draft.firstIndex(of: " "),
              let command = SlashCommand.invoked(by: String(draft[..<space]), in: commands) else { return }
        draftCommand = command
        draft = String(draft[draft.index(after: space)...])
    }

    /// Backspace on an empty draft turns the chip back into text.
    func unwrapCommandChip() -> Bool {
        guard let chip = draftCommand, draft.isEmpty else { return false }
        draftCommand = nil
        draft = chip.name
        return true
    }

    /// The rest of the highlighted command's name after what is typed, shown in gray.
    var commandCompletionSuffix: String? {
        let suggestions = commandSuggestions
        guard draftCommand == nil, !suggestions.isEmpty else { return nil }
        let name = suggestions[min(commandSelection, suggestions.count - 1)].name
        guard name.lowercased().hasPrefix(draft.lowercased()), name.count > draft.count else { return nil }
        return String(name.dropFirst(draft.count))
    }

    func moveCommandSelection(_ step: Int) {
        let count = commandSuggestions.count
        guard count > 0 else { return }
        commandSelection = ((commandSelection + step) % count + count) % count
    }

    /// Open the context sheet on the chat's latest breakdown.
    func showContext() {
        guard let usage = transcript.usage else { return }
        contextSheet = usage.context ?? ContextBreakdown(usage: usage)
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

    // MARK: - Chat actions

    func toggleArchive(_ session: SessionSummary) {
        if archivedSessions.contains(session.id) {
            archivedSessions.remove(session.id)
        } else {
            archivedSessions.insert(session.id)
            pinnedSessions.remove(session.id)
            UserDefaults.standard.set(Array(pinnedSessions), forKey: "pinnedSessions")
            if selectedSession == session.id { Task { await newSession() } }
        }
        UserDefaults.standard.set(Array(archivedSessions), forKey: "archivedSessions")
    }

    func togglePin(_ session: SessionSummary) {
        if pinnedSessions.contains(session.id) { pinnedSessions.remove(session.id) } else { pinnedSessions.insert(session.id) }
        UserDefaults.standard.set(Array(pinnedSessions), forKey: "pinnedSessions")
    }

    /// The chat's folder in the session store, from `rustcode sessions path`,
    /// accepted only when it is that session's folder inside the store.
    func sessionDirectory(_ session: SessionSummary) async -> URL? {
        let output = await runEngine(["sessions", "path", session.id]).map { String(decoding: $0, as: UTF8.self) }
        guard let path = output?.trimmingCharacters(in: .whitespacesAndNewlines), !path.isEmpty else { return nil }
        let url = URL(fileURLWithPath: path, isDirectory: true).standardizedFileURL
        guard url.lastPathComponent == session.id, url.path.contains("/rustcode/sessions/") else { return nil }
        return url
    }

    /// Store a new title the way the engine does, in the chat's `title.txt`.
    func rename(_ session: SessionSummary, to title: String) async {
        let title = title.split(whereSeparator: \.isNewline).joined(separator: " ").trimmingCharacters(in: .whitespaces)
        guard !title.isEmpty else { return }
        guard let directory = await sessionDirectory(session) else {
            error = "Could not find this chat's folder to rename it."
            return
        }
        do {
            try title.write(to: directory.appendingPathComponent("title.txt"), atomically: true, encoding: .utf8)
            await refreshSessions()
        } catch { self.error = "Could not rename the chat: \(error.localizedDescription)" }
    }

    /// Move the chat's folder to the Trash, where it can be put back. A chat
    /// open in the engine is closed first so nothing writes to it afterwards.
    func trash(_ session: SessionSummary) async {
        if busy, turnSession == session.id {
            error = "Stop the running turn before moving this chat to the Trash."
            return
        }
        guard let directory = await sessionDirectory(session) else {
            error = "Could not find this chat's folder."
            return
        }
        if openSessions[session.id] != nil || selectedSession == session.id,
           capabilities?.closeSession == true, let connection = engine?.connection {
            _ = try? await connection.request(ACPMethod.sessionClose, params: ACPRequests.session(session.id), timeout: .seconds(10))
        }
        openSessions[session.id] = nil
        do {
            try await NSWorkspace.shared.recycle([directory])
        } catch {
            self.error = "Could not move the chat to the Trash: \(error.localizedDescription)"
            return
        }
        pinnedSessions.remove(session.id)
        UserDefaults.standard.set(Array(pinnedSessions), forKey: "pinnedSessions")
        sessions.removeAll { $0.id == session.id }
        if selectedSession == session.id {
            selectedSession = nil
            transcript.removeAll()
            await newSession()
        }
    }

    func revealInFinder(_ session: SessionSummary) async {
        guard let directory = await sessionDirectory(session) else { return }
        NSWorkspace.shared.activateFileViewerSelecting([directory])
    }

    /// Put a message back in the composer to change and send again.
    func edit(_ text: String) {
        draftCommand = nil
        let message = ImageMarkers.split(text)
        draft = message.text
        images = message.images.compactMap { try? ImageAttachment.load($0) }
    }

    /// Send a message again as a new prompt.
    func retry(_ text: String) async {
        guard !busy else { return }
        draftCommand = nil
        draft = text
        await submit()
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

    func chooseImages() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = true
        panel.allowsMultipleSelection = true
        panel.allowedContentTypes = [.image]
        panel.prompt = "Attach"
        if panel.runModal() == .OK { attach(panel.urls) }
    }

    /// Image files become images; other files and folders are sent as paths.
    func attach(_ urls: [URL]) {
        for url in urls {
            do {
                if let image = try ImageAttachment.load(url) {
                    images.append(image)
                    continue
                }
            } catch {
                self.error = "Could not attach \(url.lastPathComponent): \(error.localizedDescription)"
                continue
            }
            if !attachments.contains(url) { attachments.append(url) }
        }
    }

    /// ⌘V with images on the pasteboard attaches them. False leaves the paste
    /// to the text field.
    func pasteImages(from pasteboard: NSPasteboard = .general) -> Bool {
        do {
            let pasted = try ImageAttachment.fromPasteboard(pasteboard)
            images += pasted
            return !pasted.isEmpty
        } catch {
            self.error = error.localizedDescription
            return true
        }
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

    /// What a session opened in this engine looked like when the user left it.
    /// The engine refuses a second `session/load` of an open session, so
    /// switching back restores this instead, kept current by its updates.
    private struct OpenSession {
        var transcript: Transcript
        var models: ModelOptions
        var selectedModel: String?
    }

    private var openSessions: [String: OpenSession] = [:]

    private func stashSelectedSession() {
        guard let current = selectedSession else { return }
        openSessions[current] = OpenSession(transcript: transcript, models: models, selectedModel: selectedModel)
    }

    private func restartForWorkspace() async {
        stop()
        transcript.removeAll()
        sessions = []
        selectedSession = nil
        await start()
    }

    /// A new chat starts as an empty draft. The engine session is created when
    /// the first message is sent, so opening and leaving it leaves nothing behind.
    func newSession() async {
        stashSelectedSession()
        selectedSession = nil
        transcript.removeAll()
        draftCommand = nil
        composerPanel = nil
        startDraftFromDefaults()
    }

    /// A new chat starts with the default model and effort, not the last chat's.
    private func startDraftFromDefaults() {
        guard selectedSession == nil, let modelDefaults else { return }
        models = modelDefaults.options
        selectedModel = modelDefaults.model
    }

    func loadModelDefaults() async {
        guard let output = await runEngine(["defaults"]), let defaults = try? ModelDefaults(json: output) else { return }
        modelDefaults = defaults
        startDraftFromDefaults()
    }

    /// Make a model and/or effort the default for new chats.
    func setModelDefaults(model: String? = nil, effort: String? = nil) async {
        var arguments = ["defaults"]
        if let model { arguments += ["--model", model] }
        if let effort { arguments += ["--effort", effort] }
        guard let output = await runEngine(arguments), let defaults = try? ModelDefaults(json: output) else {
            error = "Could not save the default model."
            return
        }
        modelDefaults = defaults
        startDraftFromDefaults()
    }

    /// Run the engine binary with `arguments` and return its output, or nil
    /// when it fails.
    private func runEngine(_ arguments: [String]) async -> Data? {
        guard let engine = engineLocation?.url else { return nil }
        let directory = workspace
        return await Task.detached {
            let process = Process()
            process.executableURL = engine
            process.arguments = arguments
            process.currentDirectoryURL = directory
            let pipe = Pipe()
            process.standardOutput = pipe
            process.standardError = FileHandle.nullDevice
            guard (try? process.run()) != nil else { return nil }
            let data = pipe.fileHandleForReading.readDataToEndOfFile()
            process.waitUntilExit()
            return process.terminationStatus == 0 ? data : nil
        }.value
    }

    /// Create the engine session for a draft, keeping the model picked for it.
    private func createSession() async -> String? {
        guard let connection = engine?.connection else { return nil }
        let wanted = selectedModel
        let wantedEffort = models.effort
        do {
            let result = try await connection.request(ACPMethod.sessionNew, params: ACPRequests.newSession(cwd: workspace), timeout: .seconds(30))
            guard let id = result["sessionId"]?.stringValue else { throw ACPProtocolError.malformed("session/new has no sessionId") }
            selectedSession = id
            applyConfig(result["configOptions"])
            if let wanted, wanted != selectedModel, models.choices.contains(where: { $0.id == wanted }) {
                await setModel(wanted)
            }
            if let wantedEffort, wantedEffort != models.effort, models.efforts.contains(where: { $0.id == wantedEffort }) {
                await setEffort(wantedEffort)
            }
            stashSelectedSession()
            return id
        } catch {
            self.error = String(describing: error)
            return nil
        }
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
            sessionsLoaded = true
        } catch { self.error = String(describing: error) }
    }

    /// The request shown for the selected session, if any.
    var pendingPermission: PermissionRequest? {
        permissionQueue.first { $0.sessionId == selectedSession }
    }

    func select(_ session: SessionSummary) async {
        guard let connection = engine?.connection, capabilities?.loadSession == true,
              session.id != selectedSession else { return }
        stashSelectedSession()
        if let open = openSessions[session.id] {
            selectedSession = session.id
            transcript = open.transcript
            models = open.models
            selectedModel = open.selectedModel
            return
        }
        // `session/load` replays the transcript as `session/update`
        // notifications before it answers, so select the session first or
        // the replay is filtered out as belonging to another session.
        selectedSession = session.id
        transcript.removeAll()
        do {
            let result = try await connection.request(ACPMethod.sessionLoad, params: ACPRequests.loadSession(id: session.id, cwd: URL(fileURLWithPath: session.cwd)), timeout: .seconds(30))
            // Another session may have been picked while this one loaded.
            guard selectedSession == session.id else {
                openSessions[session.id] = openSessions[session.id] ?? OpenSession(transcript: Transcript(), models: .empty)
                return
            }
            applyConfig(result["configOptions"])
            stashSelectedSession()
        } catch { self.error = String(describing: error) }
    }

    /// A message written while a turn runs. It waits here and is sent when
    /// the chat's turn ends, as the iOS app queues them.
    struct QueuedPrompt: Identifiable, Equatable {
        let id = UUID()
        let sessionId: String
        /// What was typed, for the preview and for editing it again.
        let typed: String
        let text: String
        let images: [ImageAttachment]
    }

    @Published var queuedPrompts: [QueuedPrompt] = []

    /// The selected chat's queued messages, oldest first.
    var selectedQueue: [QueuedPrompt] { queuedPrompts.filter { $0.sessionId == selectedSession } }

    func removeQueued(_ prompt: QueuedPrompt) {
        queuedPrompts.removeAll { $0.id == prompt.id }
    }

    /// Take a queued message back into the composer.
    func editQueued(_ prompt: QueuedPrompt) {
        removeQueued(prompt)
        draftCommand = nil
        draft = prompt.typed
        images = prompt.images
    }

    func submit() async {
        guard engine?.connection != nil else { return }
        if let chip = draftCommand {
            draft = draft.isEmpty ? chip.name : chip.name + " " + draft
            draftCommand = nil
        }
        let typed = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !typed.isEmpty || !attachments.isEmpty || !images.isEmpty else { return }
        if !images.isEmpty, capabilities?.promptImages != true {
            error = "This RustCode engine cannot take images. Update RustCode to attach them."
            return
        }
        if let command = SlashCommand.invoked(by: typed, in: commands), command.action != .execute {
            draft = ""
            await perform(command)
            return
        }
        let text = Self.prompt(typed, attaching: attachments, in: workspace)
        let sentImages = images
        if busy {
            // A draft has no chat to queue for until its first turn creates one.
            guard let id = selectedSession ?? turnSession else { return }
            queuedPrompts.append(QueuedPrompt(sessionId: id, typed: typed, text: text, images: sentImages))
            draft = ""
            attachments = []
            images = []
            return
        }
        draft = ""
        attachments = []
        images = []
        let created = selectedSession == nil
        guard let id = selectedSession == nil ? await createSession() : selectedSession else {
            draft = typed
            images = sentImages
            return
        }
        if created { await refreshSessions() }
        await run(QueuedPrompt(sessionId: id, typed: typed, text: text, images: sentImages))
    }

    /// Run one prompt as the chat's turn, then the chat's next queued message,
    /// unless the turn was stopped.
    private func run(_ prompt: QueuedPrompt) async {
        guard let connection = engine?.connection else { return }
        let id = prompt.sessionId
        if prompt.text == prompt.typed, prompt.images.isEmpty,
           SlashCommand.invoked(by: prompt.typed, in: commands)?.action == .execute {
            appendToTranscript(of: id) { $0.appendCommand(prompt.typed) }
        } else {
            let markers = prompt.images.compactMap { $0.writePreview() }.map(ImageMarkers.marker)
            let shown = ([prompt.text] + markers).filter { !$0.isEmpty }.joined(separator: "\n")
            appendToTranscript(of: id) { $0.appendUser(shown) }
        }
        busy = true
        turnSession = id
        var stopped = false
        do {
            let result = try await connection.request(ACPMethod.sessionPrompt, params: ACPRequests.prompt(sessionId: id, text: prompt.text, images: prompt.images))
            stopped = result["stopReason"]?.stringValue == "cancelled"
            await refreshSessions()
        } catch {
            self.error = String(describing: error)
            stopped = true
        }
        busy = false
        turnSession = nil
        // The turn is over, so the engine no longer waits on its requests.
        permissionQueue.removeAll { $0.sessionId == id }
        if !stopped { notifyFinished(id) }
        guard !stopped, let next = queuedPrompts.first(where: { $0.sessionId == id }) else { return }
        removeQueued(next)
        await run(next)
    }

    /// The selected chat's transcript is live; another open chat's is stashed.
    private func appendToTranscript(of session: String, _ update: (inout Transcript) -> Void) {
        if session == selectedSession {
            update(&transcript)
        } else if openSessions[session] != nil {
            update(&openSessions[session]!.transcript)
        }
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
        guard let session = selectedSession else {
            selectedModel = model   // applied when the draft becomes a session
            if let modelDefaults {
                models.efforts = modelDefaults.efforts(for: model)
                if !models.efforts.contains(where: { $0.id == models.effort }) {
                    models.effort = models.efforts.isEmpty ? nil : "default"
                }
            }
            return
        }
        guard let connection = engine?.connection else { return }
        do {
            let result = try await connection.request(ACPMethod.sessionSetConfigOption, params: ACPRequests.setModel(sessionId: session, value: model), timeout: .seconds(15))
            if let options = result["configOptions"] { applyConfig(options) }
            selectedModel = model
        } catch { self.error = String(describing: error) }
    }

    /// Reasoning effort for the chat. A draft keeps it until its session exists.
    func setEffort(_ effort: String) async {
        guard let session = selectedSession else {
            models.effort = effort
            return
        }
        guard let connection = engine?.connection else { return }
        do {
            let result = try await connection.request(
                ACPMethod.sessionSetConfigOption,
                params: ACPRequests.setConfig(sessionId: session, configId: "effort", value: effort),
                timeout: .seconds(15))
            if let options = result["configOptions"] { applyConfig(options) } else { models.effort = effort }
        } catch { self.error = String(describing: error) }
    }

    func stop() {
        openSessions = [:]
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
        UserDefaults.standard.set(try? JSONEncoder().encode(value), forKey: "lastConfigOptions")
    }

    /// With no saved model list, open a throwaway session to read one and close
    /// it again; a session without messages is not listed.
    private func fetchModelOptions() async {
        guard let connection = engine?.connection else { return }
        do {
            let result = try await connection.request(ACPMethod.sessionNew, params: ACPRequests.newSession(cwd: workspace), timeout: .seconds(30))
            // Keep a model or effort picked on the draft while this was loading.
            let (pickedModel, pickedEffort) = (selectedModel, models.effort)
            applyConfig(result["configOptions"])
            if selectedSession == nil {
                if let pickedModel, models.choices.contains(where: { $0.id == pickedModel }) { selectedModel = pickedModel }
                if let pickedEffort, models.efforts.contains(where: { $0.id == pickedEffort }) { models.effort = pickedEffort }
            }
            if let id = result["sessionId"]?.stringValue, capabilities?.closeSession == true {
                _ = try? await connection.request(ACPMethod.sessionClose, params: ACPRequests.session(id), timeout: .seconds(10))
            }
        } catch { self.error = String(describing: error) }
    }

    /// A draft has no session yet, so its model picker uses the options the
    /// engine last sent.
    private func restoreLastConfig() {
        guard models.choices.isEmpty,
              let data = UserDefaults.standard.data(forKey: "lastConfigOptions"),
              let value = try? JSONDecoder().decode(JSONValue.self, from: data) else { return }
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
            guard let sessionId = params["sessionId"]?.stringValue else { return }
            // A session open in the background keeps its transcript current.
            guard sessionId == selectedSession else {
                if var open = openSessions[sessionId] {
                    open.transcript.apply(SessionUpdate(notificationParams: params))
                    openSessions[sessionId] = open
                }
                return
            }
            let update = SessionUpdate(notificationParams: params)
            switch update {
            case .title: await refreshSessions()
            case .configOptions(let options): applyConfig(options)
            case .contextReport(_, let context):
                transcript.apply(update)
                contextSheet = context
            default: transcript.apply(update)
            }
        case .request(let id, let method, let params) where method == ACPMethod.requestPermission:
            if let request = PermissionRequest(rpcId: id, params: params) {
                permissionQueue.append(request)
                TurnNotifier.shared.notify(
                    session: request.sessionId,
                    title: request.question == nil ? "Approval needed" : "RustCode has a question",
                    body: "\(chatTitle(request.sessionId)): \(request.question?.text ?? request.title)")
            }
            else { try? await engine?.connection.respond(id: id, error: .invalidParams(method)) }
        case .request(let id, let method, _):
            try? await engine?.connection.respond(id: id, error: .methodNotFound(method))
        default: break
        }
    }
}
