import AppKit
import Combine
import SwiftUI
import RustCodeKit
import RustCodeTerminal

@main
struct RustCodeApp: App {
    @StateObject private var model = DesktopModel()

    init() {
        // A write to the engine's stdin after it exits must surface as an
        // error, not kill the app with the default SIGPIPE action.
        signal(SIGPIPE, SIG_IGN)
        // One window, no window tabs: AppKit's View > Show Tab Bar would
        // otherwise take ⇧⌘T from the terminal.
        NSWindow.allowsAutomaticWindowTabbing = false
        AppearanceSetting.apply(UserDefaults.standard.string(forKey: AppearanceSetting.key))
    }

    var body: some Scene {
        WindowGroup {
            Group {
                if ProcessInfo.processInfo.arguments.contains("--visualization-demo") {
                    VisualizationDemoView()
                } else {
                    ContentView(model: model)
                        .task { await model.start() }
                        .task { model.updater.startPolling() }
                        .task { await model.whatsNew.checkOnLaunch(current: model.updater.currentVersion) }
                        #if DEBUG
                        .task { DebugDriver.start(model) }
                        #endif
                        .onDisappear { model.stop() }
                }
            }
            .frame(minWidth: 850, minHeight: 620)
        }
        .windowStyle(.hiddenTitleBar)
        .commands {
            CommandGroup(replacing: .newItem) {
                Button("New Thread") { Task { await model.newSession() } }
                    .keyboardShortcut(model.keybindings.shortcut(for: .newChat))
                // A chat with messages stays in its folder, so from one
                // this starts a new chat; the title says which it does.
                Button(model.selectedSession == nil ? "Choose Folder for This Thread…" : "New Thread in Folder…") {
                    Task { await model.chooseChatProject() }
                }
                .keyboardShortcut(model.keybindings.shortcut(for: .chooseChatProject))
                Button("Choose Default Project…") { model.chooseWorkspace() }
                    .keyboardShortcut(model.keybindings.shortcut(for: .chooseProject))
            }
            // AppKit's File > Close (⌘W) would close the only window.
            CommandGroup(replacing: .saveItem) {
                CloseTerminalMenuItem(terminal: model.terminal, keybindings: model.keybindings)
            }
            CommandGroup(after: .newItem) {
                Button("Switch Thread…") { model.toggleSwitcher() }
                    .keyboardShortcut(model.keybindings.shortcut(for: .switchChat))
                Button("Stop Turn") { Task { await model.cancel() } }
                    .keyboardShortcut(model.keybindings.shortcut(for: .stopTurn))
                    .disabled(!model.busy)
            }
            CommandGroup(after: .appInfo) {
                Button("Check for Updates…") {
                    model.openSettings(.about)
                    Task { await model.updater.check() }
                }
            }
            // Settings open as a page in the main window, not a window of their own.
            CommandGroup(replacing: .appSettings) {
                Button("Settings…") { model.openSettings() }
                    .keyboardShortcut(model.keybindings.shortcut(for: .settings))
            }
            CommandGroup(after: .textEditing) {
                Button("Find in Thread…") { model.showsFind = true }
                    .keyboardShortcut(model.keybindings.shortcut(for: .findInChat))
                Button("Find Next") { model.findNext() }
                    .keyboardShortcut(model.keybindings.shortcut(for: .findNext))
                Button("Find Previous") { model.findNext(-1) }
                    .keyboardShortcut(model.keybindings.shortcut(for: .findPrevious))
            }
            CommandGroup(replacing: .printItem) {
                // Never disabled: the menu item kept the state it had at
                // launch, with no chat open, so ⌘P stayed off after a chat
                // was opened. The action reads the chat shown when it runs.
                Button(model.isSelectedPinned ? "Unpin Thread" : "Pin Thread") { model.togglePinSelected() }
                    .keyboardShortcut(model.keybindings.shortcut(for: .pinChat))
            }
            CommandGroup(before: .sidebar) {
                Button(model.showsSidebar ? "Hide Sidebar" : "Show Sidebar") { model.toggleSidebar() }
                    .keyboardShortcut(model.keybindings.shortcut(for: .toggleSidebar))
                Button(model.showsPreview ? "Hide Preview" : "Show Preview") { model.togglePreview() }
                    .keyboardShortcut(model.keybindings.shortcut(for: .togglePreview))
                Button("Reload Preview") { model.preview.reload() }
                    .keyboardShortcut(model.keybindings.shortcut(for: .reloadPreview))
                    .disabled(!model.showsPreview)
                TerminalMenuItems(terminal: model.terminal, keybindings: model.keybindings)
            }
            CommandGroup(after: .help) {
                Button("What's New in RustCode") {
                    Task { await model.whatsNew.reopen(current: model.updater.currentVersion) }
                }
                Button("Charts, Tables & Trees in Thread…") { model.showsVisualizationGuide = true }
            }
        }
    }
}

/// The panel open above the composer.
/// `model` is the Model & Effort picker: model, reasoning effort and Fast.
enum ComposerPanel { case add, model, agents }

/// The shown chat's transcript, observed apart from the rest of the model.
/// Streamed chunks arrive far faster than the screen redraws, and each one
/// used to re-evaluate the whole window, so a change is announced at most
/// once per `publishInterval`; reads always see the latest transcript.
@MainActor
final class LiveTranscript: ObservableObject {
    var transcript = Transcript() {
        didSet { schedulePublish() }
    }
    private var publishPending = false

    static let publishInterval = Duration.milliseconds(33)

    private func schedulePublish() {
        guard !publishPending else { return }
        publishPending = true
        Task { @MainActor [weak self] in
            try? await Task.sleep(for: Self.publishInterval)
            guard let self else { return }
            self.publishPending = false
            self.objectWillChange.send()
        }
    }
}

/// What is typed in the composer. Kept out of `DesktopModel` so a keystroke
/// redraws the composer, not every view watching the model.
@MainActor
final class ComposerDraft: ObservableObject {
    @Published var text = ""
    /// The highlighted slash-command suggestion.
    @Published var commandSelection = 0
}

@MainActor
final class DesktopModel: ObservableObject {
    let updater = Updater()
    /// The once-per-release What's New card and its changelog (#2506).
    let whatsNew = WhatsNew()
    /// The user's app shortcuts, shared by the menus and Settings.
    let keybindings = KeybindingStore()
    /// Keeps the real menu's key equivalents in step with `keybindings`.
    private lazy var keybindingMenuSync = KeybindingMenuSync(store: keybindings)
    @Published var composerPanel: ComposerPanel? {
        didSet {
            if composerPanel == .model, oldValue != .model { startPanelTypeahead() }
            else if composerPanel != .model, !showsSwitcher { stopPanelTypeahead() }
        }
    }
    /// The `/context` breakdown shown in its sheet.
    @Published var contextSheet: ContextBreakdown?
    /// Chats being opened, each with its replayed transcript until it is in.
    /// Kept per chat: picking another chat mid-load used to drop the replay
    /// and remember the chat as empty, and it stayed blank until a restart.
    @Published private(set) var loadingSessions: Set<String> = []
    /// Chats open in this engine and chats being opened (`OpenSessions`).
    /// Only its loading set is published: a background chat's updates must
    /// not redraw the shown one.
    private var openChats = OpenSessions() {
        didSet { if openChats.loading != loadingSessions { loadingSessions = openChats.loading } }
    }
    /// The shown chat is still being opened.
    var isLoadingSelected: Bool { selectedSession.map(loadingSessions.contains) ?? false }
    /// A host command's long answer, shown in its sheet.
    @Published var commandResult: CommandResult?
    @Published private(set) var toasts: [Toast] = []
    /// Host commands running per chat: their answer is collected here and
    /// shown natively instead of in the chat.
    @Published private var runningCommands: [String: RunningCommand] = [:]

    /// Report commands sent beside a turn, whose answers are not the turn's.
    private var pendingReports = PendingReports()

    private struct RunningCommand {
        let input: String
        var output = ""
        let toast: UUID
        /// The answer already opened natively, as `/context` opens its sheet.
        var answered = false
    }
    /// Slash commands the engine advertised, and the highlighted suggestion.
    @Published var commands: [SlashCommand] = SlashCommand.appCommands
    var commandSelection: Int {
        get { composer.commandSelection }
        set { if composer.commandSelection != newValue { composer.commandSelection = newValue } }
    }
    /// A recognized slash command, shown as a chip in front of the draft.
    @Published var draftCommand: SlashCommand?
    /// The Settings section to show when the sheet opens.
    @Published var settingsSection: SettingsSection = .general
    let chat = LiveTranscript()
    var transcript: Transcript {
        get { chat.transcript }
        set { chat.transcript = newValue }
        _modify { yield &chat.transcript }
    }
    @Published var sessions: [SessionSummary] = []
    @Published var selectedSession: String? {
        didSet {
            if let selectedSession { unseenReplies.remove(selectedSession) }
            if oldValue != selectedSession {
                showAgents(of: selectedSession, leaving: oldValue)
                openToolRuns = []
                turnsOpenedWhileRunning = []
                syncTerminalProject()
            }
        }
    }
    /// Chats whose turn finished while another chat was on screen, marked
    /// with a dot in the sidebar until opened.
    @Published private(set) var unseenReplies: Set<String> = []
    /// The default project: new chats start here unless one is given its own
    /// folder. Set in Settings; the engine and preview run in it.
    @Published var workspace: URL {
        didSet {
            UserDefaults.standard.set(workspace.path, forKey: "workspacePath")
            preview.setWorkspace(workspace.path)
            syncTerminalProject()
        }
    }
    /// The folder picked for the new-chat draft only, leaving the default
    /// alone. Nil starts the chat in the default project.
    @Published var draftProject: URL? {
        didSet { syncTerminalProject() }
    }
    /// The folder each chat opened or created here works in. The engine keeps
    /// a chat in the folder it was created in, so this never goes stale.
    private var sessionProjects: [String: String] = [:]

    /// The folder the chat on screen works in: an open chat's own folder, or
    /// the draft's.
    var chatProject: URL {
        let cwd = selectedSession.flatMap { id in sessionProjects[id] ?? sessions.first { $0.id == id }?.cwd }
        return ProjectFolder.forChat(sessionCwd: cwd, draft: draftProject, default: workspace)
    }

    /// The terminal shows the shells of the chat on screen's folder, not the
    /// default project's. Each folder keeps its own shells, so going back to
    /// a chat finds them where they were, `cd` and all.
    private func syncTerminalProject() {
        terminal.setWorkspace(chatProject.path)
    }
    /// The localhost preview beside the chat, remembered per project. The
    /// agent can use it only through `localhost_preview`, after the user
    /// allows it for a chat (PreviewAgentBridge.swift).
    let preview = PreviewController()
    @Published var showsPreview = false {
        didSet { if showsPreview { showsChanges = false } }
    }
    /// The read-only Changes panel beside the chat, in the preview's place
    /// (#2642).
    @Published var showsChanges = false {
        didSet { if showsChanges { showsPreview = false } }
    }
    /// The workspace terminal, per project. The user's own shell: it never
    /// goes through the engine or reaches the agent.
    let terminal = TerminalController()
    let composer = ComposerDraft()
    var draft: String {
        get { composer.text }
        set { if composer.text != newValue { composer.text = newValue } }
    }
    /// One value for the whole life of the model: a new one on every redraw
    /// changed the environment of every message and redrew them all.
    private(set) lazy var messageActions = MessageActions(
        edit: { [weak self] in self?.edit($0) },
        retry: { [weak self] text in Task { await self?.retry(text) } },
        retryMessage: { [weak self] id in Task { await self?.retryMessage(id) } })
    @Published var showsSidebar = true
    @Published var showsSettings = false

    /// Settings in the window's place (#2553), at `section`, else General
    /// when it opens (an open Settings stays where it is). The chat stays as
    /// it was under it.
    func openSettings(_ section: SettingsSection? = nil) {
        if let section { settingsSection = section } else if !showsSettings { settingsSection = .general }
        composerPanel = nil
        showsSwitcher = false
        if !showsSettings { settingsReturnSession = selectedSession }
        showsSettings = true
    }

    /// Back from Settings to the chat it covered (#2593), even if something
    /// changed the selection meanwhile, unless that chat is gone.
    func closeSettings() {
        showsSettings = false
        guard let id = settingsReturnSession, id != selectedSession,
              let session = sessions.first(where: { $0.id == id }) else { return }
        Task { await select(session) }
    }

    /// The chat Back returns to, recorded when Settings opened.
    private(set) var settingsReturnSession: String?

    /// What the Back button names: the chat it returns to.
    var settingsBackTitle: String {
        sessions.first { $0.id == settingsReturnSession }?.title ?? "Thread"
    }
    /// `/remote`: the pairing panel over the chat.
    @Published var showsRemote = false
    /// `/visualize`, Help > Charts, Tables & Trees: the guide to in-chat
    /// visualizations, with prompts to insert.
    @Published var showsVisualizationGuide = false
    /// The latest share and pairing state, polled while the panel is open.
    @Published private(set) var remoteStatus: RemoteShareStatus?
    /// A start, pair, or stop request is in flight.
    @Published private(set) var remoteBusy = false
    private var remotePollTask: Task<Void, Never>?
    private var remoteSessionID: String?
    /// ⌘K: the chat switcher over the window.
    @Published var showsSwitcher = false {
        didSet {
            if !showsSwitcher {
                stopPanelTypeahead()
                switcherSendsDraft = false
            }
        }
    }
    /// The switcher picks a chat to send the composer's message to, instead
    /// of one to open (#2527).
    @Published private(set) var switcherSendsDraft = false
    /// Text typed between ⌘K or the model picker opening and its field
    /// taking the keyboard, which SwiftUI gives it a frame later. Without
    /// this, a fast typist's first letters went to whatever had focus, such
    /// as the sidebar search.
    private(set) var panelTypeahead = ""
    private var panelTypeaheadMonitor: Any?
    /// The switcher has text typed, so it searches the whole history.
    var switcherSearches = false {
        didSet { if switcherSearches { readAllSessionsForFilter() } }
    }
    /// ⌘F: the find bar over the chat. Its query and matches live in `find`,
    /// so typing in it redraws the find bar, not every view watching the model.
    @Published var showsFind = false
    /// The image open in the window's viewer.
    @Published var viewedImage: ViewedImage?
    let find = ChatFind()
    /// The sidebar lists chats from every project, or only the open one.
    @Published var showsAllProjects = UserDefaults.standard.object(forKey: "showsAllProjects") as? Bool ?? true {
        didSet {
            UserDefaults.standard.set(showsAllProjects, forKey: "showsAllProjects")
            Task { await refreshSessions() }
            scheduleContentSearch()
        }
    }
    /// Numbers `refreshSessions` calls so only the newest publishes.
    private var sessionListRequests = LatestRequest()
    /// False until the first session list arrives, so the sidebar can say it is loading.
    @Published private(set) var sessionsLoaded = false
    /// The engine's listing read so far, a page at a time (`SessionPages`),
    /// and the project it was read for; `sessions` is drawn from it.
    private var sessionPages = SessionPages()
    private var listedScope: URL?
    /// More chats are listed than the sidebar has read; it reads them as the
    /// reader scrolls near the end.
    @Published private(set) var hasMoreSessions = false
    @Published private(set) var loadingMoreSessions = false
    /// Reading the next page failed; the sidebar offers to try again.
    @Published private(set) var moreSessionsFailed = false
    /// Chats pinned to the top of the sidebar, kept on this Mac.
    @Published private(set) var pinnedSessions = Set(UserDefaults.standard.stringArray(forKey: "pinnedSessions") ?? []) {
        didSet { rememberPinned() }
    }
    /// What the sidebar last saw of each pinned chat, so a pinned chat whose
    /// page is not read yet still shows at the top.
    private var savedPinned: [SessionSummary] = (UserDefaults.standard.data(forKey: "pinnedSessionSummaries"))
        .flatMap { try? JSONDecoder().decode([SessionSummary].self, from: $0) } ?? []
    /// Chats hidden from the list without deleting them, kept on this Mac.
    @Published private(set) var archivedSessions = Set(UserDefaults.standard.stringArray(forKey: "archivedSessions") ?? [])
    /// When each archived chat was archived, which starts its month before
    /// the retention sweep moves it to the Trash (`SessionRetention`).
    private var archivedDates: [String: Date] = (UserDefaults.standard.dictionary(forKey: "archivedSessionDates") as? [String: Date]) ?? [:]
    private var sweepingRetention = false
    /// Chats marked done or snoozed, kept on this Mac (#2634, #2635).
    @Published var lifecycle: SidebarLifecycle = (UserDefaults.standard.data(forKey: SidebarLifecycle.defaultsKey))
        .flatMap { try? JSONDecoder().decode(SidebarLifecycle.self, from: $0) } ?? SidebarLifecycle() {
        didSet {
            guard lifecycle != oldValue else { return }
            UserDefaults.standard.set(try? JSONEncoder().encode(lifecycle), forKey: SidebarLifecycle.defaultsKey)
        }
    }
    @Published var showsArchived = false {
        didSet { if showsArchived { readAllSessionsForFilter() } }
    }
    /// The chat whose title the rename sheet is editing.
    @Published var renamingSession: SessionSummary?
    /// Set by /history to put the cursor in the chat search.
    @Published var focusSessionSearch = false
    /// Filters the sidebar's chats by title, and by contents once it has
    /// `SessionContentSearch.minimumQueryLength` characters.
    @Published var sessionFilter = "" {
        didSet {
            guard sessionFilter != oldValue else { return }
            scheduleContentSearch()
            readAllSessionsForFilter()
        }
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
    /// What the composer held for each chat not on screen, so a draft never
    /// follows the user into another chat. The new-chat draft is kept under
    /// `newChatQueue`.
    private var parkedDrafts: [String: ParkedDraft] = [:]

    struct ParkedDraft {
        var text: String
        var command: SlashCommand?
        var attachments: [URL]
        var images: [ImageAttachment]
    }
    /// Prompt turns in flight. Each chat runs its own turns, so a new chat is
    /// ready while another one works.
    @Published private(set) var turnActivity = TurnActivity()
    /// Chats with a prompt turn running, which the sidebar shows as a spinner.
    var runningSessions: Set<String> { turnActivity.sessions }
    /// The shown chat has a turn running. A draft never does.
    /// The shown chat is running a turn or a host command; a new message queues.
    var busy: Bool {
        startingChat || (selectedSession.map { runningSessions.contains($0) || runningCommands[$0] != nil } ?? false)
    }
    /// The first message of a new chat is on its way: the chat is being
    /// created, so it has no turn yet, but a message sent now must queue
    /// behind that one instead of running beside it.
    @Published private(set) var startingChat = false
    /// The shown chat is running a model turn, which the chat shows as Working.
    var isTurnRunning: Bool { selectedSession.map(runningSessions.contains) ?? false }
    #if DEBUG
    /// The debug driver's stand-in for a turn, without an engine.
    func debugTurn(_ running: Bool, in session: String) {
        if running { _ = turnActivity.begin(session) } else {
            endAllTurns(failure: nil)
            foldTurnsOpenedWhileRunning(in: session)
        }
    }

    /// The debug driver's stand-in for the agents list and an open agent.
    func debugAgents(_ agents: [SubagentInfo], viewing transcript: Transcript?) {
        debugHoldsAgents = true
        if let transcript {
            subagents = agents
            viewedAgent = agents.first.map { ViewedAgent(info: $0, transcript: transcript, loadedCount: $0.messageCount) }
        } else {
            // A new state for the agents, as a poll brings: the open one may end.
            applyAgents(agents, of: selectedSession ?? "", askedWhileViewing: viewedAgent?.id)
            subagents = agents
        }
    }
    /// The debug driver's agents stand in for the engine's, which has none.
    private var debugHoldsAgents = false
    #endif
    @Published var error: String?
    /// Every unanswered permission or question request, oldest first. The
    /// engine blocks until each one is answered, so none may be dropped.
    @Published var permissionQueue: [PermissionRequest] = [] {
        didSet { TurnNotifier.shared.setBadge(permissionQueue.count) }
    }
    @Published var models = ModelOptions.empty
    /// The plan's 5-hour and weekly windows for the shown chat's account, when
    /// the engine knows them (#2545).
    @Published var planUsage: PlanUsage?
    /// The pointer is on the usage meter, so its panel shows.
    @Published var showsPlanUsage = false
    /// The model and effort new chats start with, shared with the terminal UI.
    @Published private(set) var modelDefaults: ModelDefaults?
    @Published var selectedModel: String?
    /// The engine binary in use and where it was found, shown in Settings.
    @Published private(set) var engineLocation: EngineLocation?
    private var engine: EngineProcess?
    /// The running engine's connection, for Settings' skills and MCP pages.
    var engineConnection: ACPConnection? { engine?.connection }
    /// Discord Rich Presence, as the engine last reported it.
    @Published private(set) var presence: PresenceStatus?

    func loadPresence() async {
        guard let connection = engine?.connection else { return }
        let result = try? await connection.request(PresenceRequests.status, params: PresenceRequests.status(cwd: workspace), timeout: .seconds(10))
        presence = result.flatMap(PresenceStatus.init(result:))
    }

    func setPresence(_ enabled: Bool) async {
        guard let connection = engine?.connection else { return }
        do {
            let result = try await connection.request(PresenceRequests.set, params: PresenceRequests.set(enabled: enabled, cwd: workspace), timeout: .seconds(10))
            presence = PresenceStatus(result: result)
        } catch { self.error = "Could not change Discord Rich Presence: \(error)" }
    }

    // MARK: - Remote sharing

    /// Open the `/remote` panel for the shown chat and ask the engine to share
    /// it with a fresh pairing offer.
    func openRemote() async {
        guard let session = selectedSession, let connection = engine?.connection else {
            remotePollTask?.cancel()
            remotePollTask = nil
            remoteSessionID = nil
            remoteStatus = RemoteShareStatus(error: "Open a thread before sharing it.")
            showsRemote = true
            return
        }
        showsRemote = true
        await startRemote(session: session, connection: connection)
    }

    /// Turn remote access back on from the panel's off state.
    func turnOnRemote() async {
        guard let session = remoteSessionID ?? selectedSession, let connection = engine?.connection else {
            remoteStatus = RemoteShareStatus(error: "Open a thread before sharing it.")
            return
        }
        remoteSessionID = session
        await startRemote(session: session, connection: connection)
    }

    private func startRemote(session: String, connection: ACPConnection) async {
        remoteSessionID = session
        remoteBusy = true
        defer { remoteBusy = false }
        do {
            let result = try await connection.request(
                ACPMethod.remoteStart,
                params: ACPRequests.remoteStart(sessionId: session),
                timeout: .seconds(30))
            guard engine?.connection === connection else { return }
            remoteStatus = RemoteShareStatus(result: result)
            startRemotePolling(session: session, connection: connection)
        } catch {
            // Do not poll an engine that did not start sharing: its idle
            // status would replace this error with a plain off state.
            remotePollTask?.cancel()
            remotePollTask = nil
            remoteStatus = RemoteShareStatus(error: String(describing: error))
        }
    }

    /// Replace the pairing code without closing the panel.
    func refreshRemoteCode() async {
        guard let session = remoteSessionID, let connection = engine?.connection else { return }
        remoteBusy = true
        defer { remoteBusy = false }
        do {
            let result = try await connection.request(
                ACPMethod.remotePair,
                params: ACPRequests.remotePair(sessionId: session),
                timeout: .seconds(30))
            guard engine?.connection === connection else { return }
            remoteStatus = RemoteShareStatus(result: result)
        } catch {
            remoteStatus = RemoteShareStatus(error: String(describing: error))
        }
    }

    /// Switch remote access off for this Mac. The panel stays open so it can
    /// be turned on again without retyping `/remote`.
    func turnOffRemote() async {
        guard let session = remoteSessionID, let connection = engine?.connection else { return }
        remoteBusy = true
        defer { remoteBusy = false }
        do {
            let result = try await connection.request(
                ACPMethod.remoteStop,
                params: ACPRequests.remoteStop(sessionId: session),
                timeout: .seconds(30))
            guard engine?.connection === connection else { return }
            remoteStatus = RemoteShareStatus(result: result)
            remotePollTask?.cancel()
            remotePollTask = nil
        } catch {
            remoteStatus = RemoteShareStatus(error: String(describing: error))
        }
    }

    func closeRemote() {
        showsRemote = false
        remotePollTask?.cancel()
        remotePollTask = nil
        remoteSessionID = nil
    }

    /// A click outside the panel dismisses it without calling `closeRemote`.
    /// The binding is already false by then, so cancel the poll here.
    func remotePanelClosed() {
        remotePollTask?.cancel()
        remotePollTask = nil
        remoteSessionID = nil
    }

    private func startRemotePolling(session: String, connection: ACPConnection) {
        remotePollTask?.cancel()
        remotePollTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(1))
                guard let self, self.showsRemote, self.remoteSessionID == session,
                      self.engine?.connection === connection else { return }
                guard let result = try? await connection.request(
                    ACPMethod.remote,
                    params: ACPRequests.remoteStatus(sessionId: session),
                    timeout: .seconds(10)) else { continue }
                self.remoteStatus = RemoteShareStatus(result: result)
            }
        }
    }

    private var inboundTask: Task<Void, Never>?
    /// A shortcut change in Settings redraws the window's menus and tooltips.
    private var keybindingChanges: AnyCancellable?
    /// Inbound messages handled from the running engine, and who waits for a count.
    private var handledInbound = 0
    private var inboundWaiters: [(count: Int, resume: CheckedContinuation<Void, Never>)] = []
    private var capabilities: AgentInfo?
    /// Whether the running engine was told Smart UI is on; Settings offers a
    /// restart while this differs from the setting.
    @Published private(set) var engineSmartUI = false
    /// The engine can move a chat to another folder (#2459).
    var canMoveChat: Bool { capabilities?.sessionMove == true && capabilities?.closeSession == true }

    init() {
        let savedPath = UserDefaults.standard.string(forKey: "workspacePath")
        // An app opened from Finder or the Dock starts in `/`, which is no project.
        let launchPath = FileManager.default.currentDirectoryPath
        workspace = savedPath.map { URL(fileURLWithPath: $0, isDirectory: true) }
            ?? (launchPath == "/" ? FileManager.default.homeDirectoryForCurrentUser
                : URL(fileURLWithPath: launchPath, isDirectory: true))
        preview.setWorkspace(workspace.path)
        preview.chatTitle = { [weak self] id in self?.sessions.first { $0.id == id }?.title }
        terminal.setWorkspace(workspace.path)
        terminal.openInPreview = { [weak self] url in
            guard let self else { return }
            self.preview.load(url)
            if !self.showsPreview { self.togglePreview() }
        }
        TurnNotifier.shared.openSession = { [weak self] id in
            guard let self, let session = self.sessions.first(where: { $0.id == id }) else { return }
            Task { await self.select(session) }
        }
        TurnNotifier.shared.start()
        keybindingChanges = keybindings.objectWillChange.sink { [weak self] _ in
            self?.objectWillChange.send()
        }
        _ = keybindingMenuSync
    }

    private func chatTitle(_ session: String) -> String {
        sessions.first { $0.id == session }?.title ?? "RustCode"
    }

    /// Tell the user a chat they are not looking at finished, with the start
    /// of its reply: a notification in the background, a toast in front.
    private func notifyFinished(_ session: String) {
        let items = session == selectedSession ? transcript.items : openChats.snapshot(session)?.transcript.items ?? []
        var reply = ""
        if let last = items.last(where: { if case .assistant = $0.content { true } else { false } }),
           case .assistant(let text) = last.content {
            reply = notificationSnippet(text)
        }
        TurnNotifier.shared.notify(session: session, title: chatTitle(session), body: reply.isEmpty ? "Finished." : reply)
        if session != selectedSession { unseenReplies.insert(session) }
        if NSApp.isActive, session != selectedSession {
            showToast(Toast(style: .success, systemImage: "checkmark.circle.fill", title: chatTitle(session),
                            message: reply.isEmpty ? "Finished." : reply, session: session))
        }
    }

    /// Folded runs of tool calls the reader opened in the shown chat, by the
    /// id of each run's first call.
    @Published private(set) var openToolRuns: Set<Int> = []

    func toggleToolRun(_ run: Int) {
        if openToolRuns.remove(run) == nil {
            openToolRuns.insert(run)
            if isTurnRunning, run < 0 { turnsOpenedWhileRunning.insert(run) }
        }
    }

    /// Turn rows opened while their turn ran. When the turn ends, finished or
    /// stopped, they fold back to their first words, row and answer: a stopped
    /// turn opened to watch it stayed a pipeline.
    private var turnsOpenedWhileRunning: Set<Int> = []

    private func foldTurnsOpenedWhileRunning(in session: String) {
        guard session == selectedSession, !turnsOpenedWhileRunning.isEmpty else { return }
        openToolRuns.subtract(turnsOpenedWhileRunning)
        turnsOpenedWhileRunning = []
    }

    /// Long messages the reader opened, per chat (#2303).
    @Published private(set) var expandedMessages = ExpandedMessages()

    /// The open long messages of the shown chat.
    var expandedInChat: Set<Int> { expandedMessages.ids(in: selectedSession) }

    func toggleMessage(_ id: Int) {
        expandedMessages.toggle(id, in: selectedSession)
    }

    /// Keeps `id` open: a reply watched while it was written does not fold
    /// once it is done, and a find match shows where it is.
    func keepMessageOpen(_ id: Int) {
        guard !expandedMessages.contains(id, in: selectedSession) else { return }
        expandedMessages.expand(id, in: selectedSession)
    }

    // MARK: - Subagents

    /// A subagent's conversation, open in its panel.
    struct ViewedAgent: Identifiable {
        var info: SubagentInfo
        var transcript = Transcript()
        /// The child's message count when `transcript` was read.
        var loadedCount = -1
        var id: Int { info.id }
    }

    /// The shown chat's subagents, listed above the composer as the terminal's
    /// agent picker lists them.
    @Published private(set) var subagents: [SubagentInfo] = []
    /// The subagent open in the agent panel.
    @Published var viewedAgent: ViewedAgent?
    /// What each subagent's composer holds, by chat and agent, so a message
    /// half typed to one agent waits there while another is read (#2557).
    @Published var agentDrafts: [String: String] = [:]
    private var agentPollTask: Task<Void, Never>?
    /// The working agents of chats no longer on screen, nested under their
    /// rows in the sidebar until those chats are opened again (#2556).
    @Published private(set) var backgroundAgents = BackgroundAgents()
    private var backgroundAgentPollTask: Task<Void, Never>?

    /// Another chat is on screen: its agents replace the last one's, whose
    /// working agents stay listed under its row.
    private func showAgents(of session: String?, leaving previous: String? = nil) {
        agentPollTask?.cancel()
        agentPollTask = nil
        if let previous { backgroundAgents.leave(previous, agents: subagents) }
        subagents = session.map { backgroundAgents.take($0) } ?? []
        viewedAgent = nil
        if session != nil { pollAgents() }
        pollBackgroundAgents()
    }

    /// Read the agents of chats off screen every two seconds while one of
    /// them still works or runs a turn that may start more, so their rows
    /// show how each ends.
    func pollBackgroundAgents() {
        guard backgroundAgentPollTask == nil, capabilities?.agents == true,
              let connection = engine?.connection else { return }
        backgroundAgentPollTask = Task { [weak self] in
            while !Task.isCancelled {
                guard let self, self.engine?.connection === connection else { return }
                let sessions = self.backgroundAgents.working
                    .union(self.runningSessions)
                    .subtracting([self.selectedSession].compactMap { $0 })
                if sessions.isEmpty {
                    self.backgroundAgentPollTask = nil
                    return
                }
                for session in sessions.sorted() {
                    await self.readBackgroundAgents(of: session, on: connection)
                }
                try? await Task.sleep(for: .seconds(2))
            }
        }
    }

    private func readBackgroundAgents(of session: String, on connection: ACPConnection) async {
        #if DEBUG
        if debugHoldsAgents { return }
        #endif
        let result: JSONValue
        do {
            result = try await connection.request(
                ACPMethod.agents, params: ACPRequests.agents(sessionId: session), timeout: .seconds(10))
        } catch {
            // Opened meanwhile: the chat's own poll has its agents now.
            guard selectedSession != session else { return }
            // The engine answered that it cannot list them, as for a chat
            // deleted: they are gone. No answer at all (a timeout, a dropped
            // connection) keeps the last rows, marked stale (#2653).
            if error is RPCError {
                backgroundAgents.forget(session)
            } else {
                backgroundAgents.missed(session)
            }
            return
        }
        // Opened meanwhile: the chat's own poll has its agents now.
        guard selectedSession != session else { return }
        let agents = SubagentInfo.list(from: result)
        var next = backgroundAgents
        next.update(session, agents: agents)
        if next != backgroundAgents { backgroundAgents = next }
    }

    /// The children the sidebar nests under the shown chat.
    func sidebarChildren(of session: String) -> [SubagentInfo] {
        SubagentInfo.sidebarChildren(subagents, viewing: viewedAgent?.id)
    }

    /// Open `agent`, a working agent of `session`, from its row in the
    /// sidebar: its chat comes on screen first (#2556).
    func openAgent(_ agent: SubagentInfo, in session: SessionSummary) async {
        if session.id != selectedSession { await select(session) }
        guard selectedSession == session.id else { return }
        openAgent(subagents.first { $0.id == agent.id } ?? agent)
    }

    /// A click on a chat's row in the sidebar. The open chat's row while one
    /// of its agents is read goes straight back to the chat (#2556).
    func selectFromSidebar(_ session: SessionSummary) async {
        guard session.id != selectedSession else {
            viewedAgent = nil
            return
        }
        await select(session)
    }

    /// The key of `agent`'s draft in the shown chat.
    func agentDraftKey(_ agent: Int) -> String { "\(selectedSession ?? "-")/\(agent)" }

    /// Read the shown chat's agents once a second while its turn runs, a child
    /// is still working, or a child's panel is open; then once more, so the
    /// last state shows.
    func pollAgents() {
        guard agentPollTask == nil, capabilities?.agents == true,
              let session = selectedSession, let connection = engine?.connection else { return }
        agentPollTask = Task { [weak self] in
            while !Task.isCancelled {
                guard let self, self.selectedSession == session, self.engine?.connection === connection else { return }
                await self.readAgents(of: session, on: connection)
                guard !Task.isCancelled else { return }
                if !(self.isTurnRunning || self.subagents.contains(where: \.isActive) || self.viewedAgent != nil) {
                    self.agentPollTask = nil
                    return
                }
                try? await Task.sleep(for: .seconds(1))
            }
        }
    }

    private func readAgents(of session: String, on connection: ACPConnection) async {
        #if DEBUG
        if debugHoldsAgents { return }
        #endif
        // The child open when the list was asked for: an answer about it
        // may close its panel, never another one opened since (#2596).
        let asked = viewedAgent?.id
        guard let result = try? await connection.request(
            ACPMethod.agents, params: ACPRequests.agents(sessionId: session), timeout: .seconds(10)),
              selectedSession == session else { return }
        applyAgents(SubagentInfo.list(from: result), of: session, askedWhileViewing: asked)
        guard var viewed = viewedAgent, viewed.id == asked else { return }
        if let info = subagents.first(where: { $0.id == viewed.id }) { viewed.info = info }
        if viewed.info.messageCount != viewed.loadedCount,
           let history = try? await connection.request(
            ACPMethod.agents, params: ACPRequests.agentHistory(sessionId: session, id: viewed.id), timeout: .seconds(10)),
           viewedAgent?.id == viewed.id {
            viewed.transcript = SubagentInfo.transcript(from: history)
            viewed.loadedCount = viewed.info.messageCount
        }
        if viewedAgent?.id == viewed.id { viewedAgent = viewed }
    }

    /// A fresh agents list for the shown chat `session`. The child open on
    /// screen, still the one open when the list was asked for, that has
    /// ended since it was last seen working gives the screen back to the
    /// chat, which says how it ended (#2596).
    func applyAgents(_ agents: [SubagentInfo], of session: String, askedWhileViewing asked: Int?) {
        guard selectedSession == session else { return }
        if agents != subagents { subagents = agents }
        guard let viewed = viewedAgent, viewed.id == asked,
              let now = agents.first(where: { $0.id == viewed.id }),
              SubagentInfo.endsViewing(was: viewed.info, now: now) else { return }
        viewedAgent = nil
        confirm(now.endedMessage, systemImage: now.status == .completed ? "checkmark.circle" : "xmark.circle")
    }

    /// Open `agent`'s conversation in the agent panel.
    func openAgent(_ agent: SubagentInfo) {
        viewedAgent = ViewedAgent(info: agent)
        agentPollTask?.cancel()
        agentPollTask = nil
        pollAgents()
    }

    /// Open the agent a `spawn_agent` card started, by the id in its result.
    func openAgent(id: Int) {
        if let agent = subagents.first(where: { $0.id == id }) { openAgent(agent) }
    }

    /// More instructions for the open agent: an idle child starts on them, a
    /// running one reads them at its next step. A message the agent did not
    /// take goes back into its composer.
    func sendToViewedAgent(_ text: String) async {
        let text = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, let agent = viewedAgent, let session = selectedSession,
              let connection = engine?.connection else { return }
        let key = agentDraftKey(agent.id)
        do {
            let result = try await connection.request(
                ACPMethod.agents, params: ACPRequests.agentSend(sessionId: session, id: agent.id, text: text),
                timeout: .seconds(10))
            if result["queued"]?.boolValue == true {
                confirm("Queued for \(agent.info.name)", systemImage: "text.line.last.and.arrowtriangle.forward")
            }
            pollAgents()
        } catch {
            if (agentDrafts[key] ?? "").isEmpty { agentDrafts[key] = text }
            self.error = "\(agent.info.name) did not take the message: \(error)"
        }
    }

    /// Stop the open agent and the agents it started.
    func cancelViewedAgent() async {
        guard let agent = viewedAgent, let session = selectedSession, let connection = engine?.connection else { return }
        do {
            let result = try await connection.request(
                ACPMethod.agents, params: ACPRequests.agentCancel(sessionId: session, id: agent.id), timeout: .seconds(10))
            confirm(SubagentInfo.cancelSettled(result) ? "\(agent.info.name) stopped" : "Stopping \(agent.info.name)…",
                    systemImage: "stop.circle")
            pollAgents()
        } catch {
            self.error = "\(agent.info.name) could not be stopped: \(error)"
        }
    }

    // MARK: - Toasts

    func showToast(_ toast: Toast) {
        toasts.append(toast)
        if toasts.count > 4 { toasts.removeFirst(toasts.count - 4) }
        guard let lifetime = toast.lifetime else { return }
        Task { [weak self] in
            try? await Task.sleep(for: lifetime)
            self?.dismissToast(toast.id)
        }
    }

    func dismissToast(_ id: UUID) {
        toasts.removeAll { $0.id == id }
    }

    /// A click on a toast: its command answer, else the chat it is about.
    func open(_ toast: Toast) {
        dismissToast(toast.id)
        if let result = toast.result {
            commandResult = result
        } else if let id = toast.session, id != selectedSession,
                  let session = sessions.first(where: { $0.id == id }) {
            Task { await select(session) }
        }
    }

    /// A small confirmation of something just done.
    func confirm(_ title: String, systemImage: String) {
        showToast(Toast(style: .success, systemImage: systemImage, title: title))
    }

    func start() async {
        guard engine == nil else { return }
        do {
            let locator = EngineLocator(userSetting: UserDefaults.standard.string(forKey: "enginePath"))
            let location = try locator.locate()
            engineLocation = location
            var launched: EngineProcess?
            // A Finder-launched app has a minimal PATH; the engine's MCP
            // servers and their tools (glab, bunx, …) need the usual ones.
            let process = EngineProcess(executable: location.url, workingDirectory: workspace,
                                        environment: locator.engineEnvironment()) { [weak self] exit in
                Task { @MainActor in
                    // A late exit of an engine replaced by a project switch
                    // must not touch the state of the one now running.
                    guard let self, let launched, self.engine === launched else { return }
                    self.endAllTurns()
                    guard !exit.requested else { return }
                    // Drop the dead engine so the next `start()` launches a
                    // new one; its waiting requests can no longer be answered.
                    self.inboundTask?.cancel()
                    self.inboundTask = nil
                    self.releaseInboundWaiters()
                    self.engine = nil
                    self.capabilities = nil
                    self.permissionQueue = []
                    self.error = "RustCode engine exited (status \(exit.status))."
                    TurnNotifier.shared.notify(session: "", title: "RustCode stopped",
                                               body: "The engine exited (status \(exit.status)). Open RustCode to restart it.")
                }
            }
            launched = process
            try process.start()
            engine = process
            // Grants belong to the sessions of the engine they were given in.
            preview.revokeAgentAccess()
            inboundTask = Task { [weak self] in
                for await inbound in process.connection.inbound {
                    await self?.handle(inbound)
                    await self?.inboundHandled(from: process.connection)
                }
            }
            let smartUI = SmartUI.isEnabled
            let result = try await process.connection.request(ACPMethod.initialize, params: ACPRequests.initialize(appVersion: "0.1.0", smartUI: smartUI), timeout: .seconds(10))
            engineSmartUI = smartUI
            capabilities = try AgentInfo(initializeResult: result)
            try await process.connection.notify("initialized", params: [:])
            await refreshSessions()
            // The listed chat carries the folder the engine recorded for it;
            // a load from any other folder is refused and the chat shows empty.
            if let pending = pendingSelection {
                await select(sessions.first { $0.id == pending.id } ?? pending)
            }
            // Show the last known options at once, then read current ones: the
            // models and their effort levels change with config and engine.
            restoreLastConfig()
            await loadModelDefaults()
            if selectedSession == nil, modelDefaults == nil { await fetchModelOptions() }
        } catch { self.error = String(describing: error) }
    }

    /// ⇧⌘O: pick the default project for new chats. Cancelling changes
    /// nothing.
    func chooseWorkspace() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = false
        panel.directoryURL = workspace
        panel.prompt = "Use as Default"
        panel.message = "New threads start in this folder. Open threads keep their own."
        guard panel.runModal() == .OK, let url = panel.url else { return }
        Task { await setDefaultProject(url) }
    }

    /// Make `folder` the default project. The engine restarts there, and the
    /// chat on screen opens again in its own folder rather than moving.
    func setDefaultProject(_ folder: URL) async {
        guard usableProject(folder) else { return }
        let open = selectedSession.flatMap { id in sessions.first { $0.id == id } }
        // Keep the open chat's unsent text with it, and the composer for the
        // new chat the restart leaves on screen until it opens again.
        if open != nil {
            parkDraft()
            restoreDraft(for: nil)
        }
        workspace = folder
        draftProject = ProjectFolder.override(draftProject, default: folder)
        await restartForWorkspace()
        if let open { await select(open) }
    }

    /// ⌘O: pick the folder for the new-chat draft only. A chat with messages
    /// stays in its own folder, so from one this starts a new chat in the
    /// folder picked. Cancelling changes nothing.
    func chooseChatProject() async {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = false
        panel.directoryURL = chatProject
        if selectedSession == nil {
            panel.prompt = "Use for This Thread"
            panel.message = "This thread works in the folder you pick. New threads still start in \(workspace.lastPathComponent)."
        } else {
            panel.prompt = "Start New Thread"
            panel.message = "A new thread starts in the folder you pick. This thread stays in \(chatProject.lastPathComponent)."
        }
        guard panel.runModal() == .OK, let url = panel.url, usableProject(url) else { return }
        await newSession(in: url)
    }

    /// Change Project Folder… (#2459): the shown chat, and only it, works in
    /// the folder picked from now on; the default project and other chats
    /// keep theirs. The engine moves a closed chat, so the chat is closed,
    /// moved and opened again there. Cancelling changes nothing.
    func moveChatProject() async {
        guard capabilities?.sessionMove == true, capabilities?.closeSession == true,
              let id = selectedSession, let connection = engine?.connection else { return }
        guard !runningSessions.contains(id) else {
            error = "Stop the running turn before changing this thread's folder."
            return
        }
        let current = chatProject
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = false
        panel.directoryURL = current
        panel.prompt = "Move Thread Here"
        panel.message = "This thread works in the folder you pick from now on. Other threads and new threads keep their folders."
        guard panel.runModal() == .OK, let url = panel.url, usableProject(url),
              url.standardizedFileURL.path != current.standardizedFileURL.path else { return }
        var summary = sessions.first { $0.id == id } ?? SessionSummary(id: id, cwd: current.path)
        _ = try? await connection.request(ACPMethod.sessionClose, params: ACPRequests.session(id), timeout: .seconds(10))
        openChats.forget(id)
        do {
            let result = try await connection.request(ACPMethod.sessionMove, params: ACPRequests.moveSession(id: id, cwd: url),
                                                      timeout: .seconds(10))
            summary.cwd = result["cwd"]?.stringValue ?? url.path
            showToast(Toast(style: .info, systemImage: "folder", title: "Thread moved to \(url.lastPathComponent)",
                            message: summary.cwd.abbreviatingHome))
        } catch {
            // Still where it was: open it there again.
            self.error = "Could not move this thread: \(error)"
        }
        sessionProjects[id] = summary.cwd
        if let index = sessions.firstIndex(where: { $0.id == id }) { sessions[index].cwd = summary.cwd }
        await select(summary, reload: true)
    }

    /// A folder that cannot be read is no project: say so and keep things
    /// as they were.
    private func usableProject(_ folder: URL) -> Bool {
        guard !ProjectFolder.isUsable(folder) else { return true }
        showToast(Toast(style: .warning, systemImage: "folder.badge.questionmark",
                        title: "Can't use \(folder.lastPathComponent)",
                        message: "RustCode cannot read \(folder.path.abbreviatingHome). Nothing was changed."))
        return false
    }

    /// Start a new chat draft in `folder`, or in the default project.
    func newSession(in folder: URL?) async {
        if selectedSession != nil { await newSession() }
        draftProject = ProjectFolder.override(folder, default: workspace)
    }

    /// Step through the matches, wrapping at either end.
    func findNext(_ step: Int = 1) {
        guard !find.matches.isEmpty else { return }
        showsFind = true
        find.step(step)
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
        contentSearchTask = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(250))
            guard !Task.isCancelled, let self else { return }
            let matches = await self.searchContents(query)
            guard !Task.isCancelled else { return }
            self.contentMatches = matches
            self.contentQuery = query
        }
    }

    /// Chats whose messages contain `query`, in the sidebar's scope.
    func searchContents(_ query: String) async -> [SessionContentMatch] {
        let output = await runEngine(SessionContentSearch.arguments(query: query, allProjects: showsAllProjects))
        return output.flatMap { try? SessionContentSearch.parse($0) } ?? []
    }

    /// ⌘K opens the switcher, and closes it again. Not over Settings or
    /// another panel, which has the keyboard.
    func toggleSwitcher() {
        if showsSwitcher {
            showsSwitcher = false
        } else if !coversChat {
            showsSwitcher = true
            startPanelTypeahead()
        }
    }

    /// Holds keys typed before the switcher's or model picker's field has
    /// focus, and takes the keyboard from the field that had it.
    private func startPanelTypeahead() {
        stopPanelTypeahead()
        panelTypeahead = ""
        (NSApp.keyWindow ?? NSApp.mainWindow)?.makeFirstResponder(nil)
        panelTypeaheadMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            guard let text = SessionSwitcher.typeahead(event.characters,
                                                       commandKey: !event.modifierFlags.isDisjoint(with: [.command, .control]))
            else { return event }
            let taken = MainActor.assumeIsolated { () -> Bool in
                guard let self, self.showsSwitcher || self.composerPanel == .model else { return false }
                self.panelTypeahead += text
                return true
            }
            return taken ? nil : event
        }
    }

    /// The panel's field has the keyboard: hands it what was typed so far.
    func takePanelTypeahead() -> String {
        stopPanelTypeahead()
        defer { panelTypeahead = "" }
        return panelTypeahead
    }

    private func stopPanelTypeahead() {
        if let panelTypeaheadMonitor { NSEvent.removeMonitor(panelTypeaheadMonitor) }
        panelTypeaheadMonitor = nil
    }

    /// Open the chat picked in the switcher.
    func switchTo(_ session: SessionSummary) {
        let sends = switcherSendsDraft
        showsSwitcher = false
        if sends {
            Task { await sendDraft(to: session) }
            return
        }
        guard session.id != selectedSession else { return }
        Task { await select(session) }
    }

    /// The composer's + menu: pick a chat in the switcher, and the message
    /// goes there while this chat stays open (#2527).
    func chooseChatForDraft() {
        guard canSendDraftElsewhere else { return }
        composerPanel = nil
        showsSwitcher = true
        switcherSendsDraft = true
        startPanelTypeahead()
    }

    /// There is a typed message, with nothing the other chat could not take.
    var canSendDraftElsewhere: Bool {
        !draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && draftCommand == nil && images.isEmpty
            && engine?.connection != nil && capabilities?.loadSession == true
    }

    /// Send the composer's message to `session` without opening it: a one-way
    /// handoff. The chat runs it as a turn of its own, in its own folder,
    /// after any turn it is running; its answer arrives as for any chat in
    /// the background, a toast that opens it and a dot in the sidebar.
    func sendDraft(to session: SessionSummary) async {
        let typed = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !typed.isEmpty else { return }
        // The open chat is an ordinary send.
        if session.id == selectedSession {
            await submit()
            return
        }
        let title = chatTitle(session.id)
        let folder = URL(fileURLWithPath: session.cwd.isEmpty ? chatProject.path : session.cwd)
        let prompt = QueuedPrompt(sessionId: session.id, typed: typed, text: Self.prompt(typed, attaching: attachments, in: folder), images: [])
        draft = ""
        attachments = []
        guard await load(session) else {
            // Not delivered: the message comes back to be sent again.
            draft = typed
            error = "Could not open “\(title)” to send it the message."
            return
        }
        if runningSessions.contains(session.id) {
            queuedPrompts.append(prompt)
            showToast(Toast(style: .info, systemImage: "tray.and.arrow.down", title: "Queued for \(title)",
                            message: "It runs when that thread's turn ends.", session: session.id))
            return
        }
        showToast(Toast(style: .info, systemImage: "paperplane", title: "Sent to \(title)",
                        message: "Working there; this thread stays open.", session: session.id))
        await run(prompt)
    }

    /// A message, not a command, that a new chat of its own could start with.
    var canLaunchInBackground: Bool {
        let typed = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        return engine?.connection != nil && draftCommand == nil && !typed.hasPrefix("/") && !typed.hasPrefix("!")
            && (!typed.isEmpty || !attachments.isEmpty || !images.isEmpty)
            && (images.isEmpty || capabilities?.promptImages == true)
    }

    /// ⌘⇧⏎: start the message as a new chat that works in the background,
    /// and give the composer back empty for the next task (#2652). The new
    /// chat takes the project, model and effort the composer shows, and the
    /// same tool approval setting as any chat; nothing else is created (no
    /// worktree). It shows as Working in the sidebar and asks for attention
    /// there as any chat does. If the chat cannot be created, the message
    /// and its attachments come back to the composer.
    func launchInBackground() async {
        MarkedText.commitInWindows()
        guard canLaunchInBackground, let connection = engine?.connection else { return }
        let typed = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        let folder = chatProject
        let (savedDraft, savedAttachments, savedImages) = (draft, attachments, images)
        let text = Self.prompt(typed, attaching: attachments, in: folder)
        draft = ""
        attachments = []
        images = []
        guard let id = await createSession(in: folder, on: connection, selecting: false) else {
            draft = savedDraft
            attachments = savedAttachments
            images = savedImages
            return
        }
        startedSessions[id] = .started(id: id, cwd: folder.path, prompt: typed, model: selectedModel)
        await refreshSessions()
        let model = models.choices.first { $0.id == selectedModel }?.name ?? selectedModel ?? "the default model"
        showToast(Toast(style: .info, systemImage: "paperplane.circle", title: "Started in the background",
                        message: "\(folder.lastPathComponent) · \(model). Working there; this composer is free.", session: id))
        await run(QueuedPrompt(sessionId: id, typed: typed, text: text, images: savedImages))
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

    /// Ask the engine for the shown chat's plan usage. An engine without the
    /// request, or an account with no windows, leaves the meter hidden.
    func refreshPlanUsage() async {
        guard let connection = engineConnection, let id = selectedSession else { return }
        let answer = try? await connection.request(ACPMethod.usageLimits, params: ACPRequests.usageLimits(sessionId: id),
                                                   timeout: .seconds(15))
        guard id == selectedSession else { return }
        let usage = PlanUsage(json: answer)
        if usage != planUsage { planUsage = usage }
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
            if case .assistant(let text) = last?.content {
                Clipboard.copy(text)
                confirm("Copied the last reply", systemImage: "doc.on.doc.fill")
            }
        case .terminal: terminal.open()
        case .remote: await openRemote()
        case .visualizations: showsVisualizationGuide = true
        case .execute: break
        }
    }

    /// Put an example request from the visualization guide in the composer,
    /// replacing nothing the user typed, and close the guide.
    func insertExamplePrompt(_ prompt: String) {
        draftCommand = nil
        let typed = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        draft = typed.isEmpty ? prompt : typed + "\n\n" + prompt
        showsVisualizationGuide = false
    }

    // MARK: - Chat actions

    func toggleArchive(_ session: SessionSummary) {
        if archivedSessions.contains(session.id) {
            archivedSessions.remove(session.id)
            archivedDates[session.id] = nil
            confirm("Moved out of the archive", systemImage: "tray.and.arrow.up.fill")
        } else {
            archivedSessions.insert(session.id)
            archivedDates[session.id] = Date()
            lifecycle.forget(session.id)
            confirm("Archived", systemImage: "archivebox.fill")
            pinnedSessions.remove(session.id)
            UserDefaults.standard.set(Array(pinnedSessions), forKey: "pinnedSessions")
            if selectedSession == session.id { Task { await newSession() } }
        }
        rememberArchived()
    }

    private func rememberArchived() {
        UserDefaults.standard.set(Array(archivedSessions), forKey: "archivedSessions")
        UserDefaults.standard.set(archivedDates, forKey: "archivedSessionDates")
    }

    /// Archive chats idle a week and move chats archived a month to the
    /// Trash (#2489), at most once a day, over every project's chats. Pinned
    /// chats and chats running, open or just started are left alone. The
    /// first sweep dates the chats archived before dates were kept, so it
    /// archives but trashes nothing.
    private func sweepRetention() async {
        let now = Date()
        if let last = UserDefaults.standard.object(forKey: "sessionRetentionSweep") as? Date,
           now.timeIntervalSince(last) < SessionRetention.sweepInterval { return }
        guard !sweepingRetention, let connection = engine?.connection, capabilities?.listSessions == true else { return }
        sweepingRetention = true
        defer { sweepingRetention = false }
        var pages = SessionPages()
        do {
            repeat {
                let params = ACPRequests.listSessions(cwd: nil, cursor: pages.cursor)
                let result = try await connection.request(ACPMethod.sessionList, params: params, timeout: .seconds(15))
                pages.append(SessionSummary.list(from: result), next: SessionSummary.nextCursor(from: result))
            } while pages.hasMore
        } catch { return }
        archivedDates = SessionRetention.dated(archivedSessions, dates: archivedDates, now: now)
        var inUse = runningSessions.union(openChats.openIds).union(startedSessions.keys)
        if let selectedSession { inUse.insert(selectedSession) }
        let plan = SessionRetention.plan(sessions: pages.listed, archived: archivedDates, pinned: pinnedSessions,
                                         inUse: inUse, now: now)
        for id in plan.toArchive {
            archivedSessions.insert(id)
            archivedDates[id] = now
        }
        let expired = Set(plan.toTrash)
        var trashed = 0
        for session in pages.listed where expired.contains(session.id) {
            guard let directory = await sessionDirectory(session),
                  (try? await NSWorkspace.shared.recycle([directory])) != nil else { continue }
            forget(session.id)
            trashed += 1
        }
        rememberArchived()
        UserDefaults.standard.set(now, forKey: "sessionRetentionSweep")
        let lines = [plan.toArchive.isEmpty ? nil : "Archived \(plan.toArchive.count) thread\(plan.toArchive.count == 1 ? "" : "s") idle for a week.",
                     trashed == 0 ? nil : "Moved \(trashed) thread\(trashed == 1 ? "" : "s") archived for a month to the Trash."]
            .compactMap { $0 }
        guard !lines.isEmpty else { return }
        showToast(Toast(style: .info, systemImage: "archivebox", title: "Tidied the thread list",
                        message: lines.joined(separator: " ")))
    }

    /// Drop a chat whose folder is gone from everything the sidebar keeps.
    private func forget(_ id: String) {
        pinnedSessions.remove(id)
        UserDefaults.standard.set(Array(pinnedSessions), forKey: "pinnedSessions")
        archivedSessions.remove(id)
        archivedDates[id] = nil
        lifecycle.forget(id)
        sessions.removeAll { $0.id == id }
        sessionPages.remove(id)
        startedSessions[id] = nil
        parkedDrafts[id] = nil
    }

    /// Chats created here that the engine does not list yet.
    private var startedSessions: [String: SessionSummary] = [:]
    /// When each chat was last used here. The engine's list can lag a running
    /// turn, so a refresh mid-turn must not move the chat back down (#2418).
    private var lastActivity: [String: Date] = [:]

    var isSelectedPinned: Bool { selectedSession.map(pinnedSessions.contains) ?? false }

    /// ⌘P: pin or unpin the open chat, by its id: a chat just opened or
    /// started may not be in the sidebar's list yet, and ⌘P used to do nothing
    /// for it. It never changes the chat shown or what is typed.
    func togglePinSelected() {
        guard let id = selectedSession else {
            showToast(Toast(style: .info, systemImage: "pin", title: "Nothing to pin yet",
                            message: "A new thread can be pinned once its first message is sent."))
            return
        }
        togglePin(id: id)
    }

    func togglePin(_ session: SessionSummary) {
        togglePin(id: session.id)
    }

    private func togglePin(id: String) {
        if pinnedSessions.contains(id) {
            pinnedSessions.remove(id)
            confirm("Unpinned", systemImage: "pin.slash.fill")
        } else {
            pinnedSessions.insert(id)
            confirm("Pinned", systemImage: "pin.fill")
        }
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
            error = "Could not find this thread's folder to rename it."
            return
        }
        do {
            try title.write(to: directory.appendingPathComponent("title.txt"), atomically: true, encoding: .utf8)
            await refreshSessions()
        } catch { self.error = "Could not rename the thread: \(error.localizedDescription)" }
    }

    /// Move the chat's folder to the Trash, where it can be put back. A chat
    /// open in the engine is closed first so nothing writes to it afterwards.
    func trash(_ session: SessionSummary) async {
        if runningSessions.contains(session.id) {
            error = "Stop the running turn before moving this thread to the Trash."
            return
        }
        guard let directory = await sessionDirectory(session) else {
            error = "Could not find this thread's folder."
            return
        }
        if openChats.isOpen(session.id) || selectedSession == session.id,
           capabilities?.closeSession == true, let connection = engine?.connection {
            _ = try? await connection.request(ACPMethod.sessionClose, params: ACPRequests.session(session.id), timeout: .seconds(10))
        }
        openChats.forget(session.id)
        do {
            try await NSWorkspace.shared.recycle([directory])
        } catch {
            self.error = "Could not move the thread to the Trash: \(error.localizedDescription)"
            return
        }
        forget(session.id)
        rememberArchived()
        confirm("Moved to the Trash", systemImage: "trash.fill")
        if selectedSession == session.id {
            selectedSession = nil
            transcript.removeAll()
            await newSession(keepingDraft: false)
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

    /// ↑ on an empty composer: bring the last message the user sent back to
    /// edit and resend, as the terminal does. A message still waiting in the
    /// queue comes back first, newest first, since it is what the user typed
    /// last. False when there is nothing to recall or the composer already
    /// holds something, so ↑ still moves the cursor within the text.
    @discardableResult
    func recallLastMessage() -> Bool {
        guard draftCommand == nil, draft.isEmpty, attachments.isEmpty, images.isEmpty else { return false }
        if let queued = selectedQueue.last {
            editQueued(queued)
            return true
        }
        guard let message = transcript.items.last(where: { item in
            if case .user(let text) = item.content { return !ToolAttachment.isAttachment(text) }
            return false
        }), case .user(let text) = message.content else { return false }
        edit(text)
        return true
    }

    /// Retry: the chat goes back to just before message `id` and sends it
    /// again, so it does not show twice (#2278). Messages after it go too,
    /// once the user confirms. An engine without rewind sends it again below.
    func retryMessage(_ id: Int) async {
        // A second Retry while the first still asks or rewinds would send the
        // message twice: the turn only counts as busy once it is sent.
        guard !busy, !isRetrying, let item = transcript.items.first(where: { $0.id == id }),
              case .user(let text) = item.content else { return }
        isRetrying = true
        defer { isRetrying = false }
        guard capabilities?.rewind == true, let session = selectedSession,
              let connection = engine?.connection, let index = transcript.userIndex(of: id) else {
            return await retry(text)
        }
        let later = transcript.userMessages(after: id)
        if later > 0 {
            let alert = NSAlert()
            alert.messageText = "Retry this message?"
            alert.informativeText = later == 1
                ? "The message after it and its reply will be removed."
                : "The \(later) messages after it and their replies will be removed."
            alert.addButton(withTitle: "Retry")
            alert.addButton(withTitle: "Cancel")
            guard alert.runModal() == .alertFirstButtonReturn else { return }
        }
        do {
            _ = try await connection.request(
                ACPMethod.rewind, params: ACPRequests.rewind(sessionId: session, userIndex: index, text: text),
                timeout: .seconds(10))
        } catch {
            self.error = "Could not retry the message: \(error)"
            return
        }
        transcript.removeFrom(id)
        await retry(text)
    }

    /// A Retry is between its press and its send.
    private var isRetrying = false

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

    /// ⇧⌘P: show or hide the localhost preview.
    func togglePreview() {
        withAnimation(.snappy(duration: 0.25)) { showsPreview.toggle() }
    }

    func toggleChanges() {
        withAnimation(.snappy(duration: 0.25)) { showsChanges.toggle() }
    }

    func chooseAttachments() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = true
        panel.allowsMultipleSelection = true
        panel.directoryURL = chatProject
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

    private func inboundHandled(from connection: ACPConnection) {
        // A replaced engine's last message does not count for the new one.
        guard engine?.connection === connection else { return }
        handledInbound += 1
        let ready = inboundWaiters.filter { $0.count <= handledInbound }
        inboundWaiters.removeAll { $0.count <= handledInbound }
        for waiter in ready { waiter.resume.resume() }
    }

    /// Wait until every message the engine sent before now is handled. A
    /// response resolves ahead of the updates queued before it: without this,
    /// a chat opened with only part of its replay, and a finished turn read
    /// its reply before the last chunk.
    private func catchUpInbound(on connection: ACPConnection) async {
        let target = await connection.inboundCount
        guard engine?.connection === connection, handledInbound < target else { return }
        await withCheckedContinuation { inboundWaiters.append((target, $0)) }
    }

    /// The engine is gone: nothing more will be handled from it.
    private func releaseInboundWaiters() {
        handledInbound = 0
        let waiters = inboundWaiters
        inboundWaiters = []
        for waiter in waiters { waiter.resume.resume() }
    }

    /// Start a fresh engine, e.g. after its path changed in Settings.
    func restartEngine() async {
        error = nil
        await restartForWorkspace()
    }

    /// Keep the chat on screen open in the background; one still loading is
    /// stored by its load instead.
    private func stashSelectedSession() {
        guard let current = selectedSession else { return }
        openChats.stash(current, transcript: transcript, models: models, selectedModel: selectedModel)
    }

    private func restartForWorkspace() async {
        stop()
        transcript.removeAll()
        sessions = []
        sessionPages.reset()
        hasMoreSessions = false
        selectedSession = nil
        await start()
    }

    /// A new chat starts as an empty draft. The engine session is created when
    /// the first message is sent, so opening and leaving it leaves nothing behind.
    /// `keepingDraft` false drops what the composer holds instead of keeping
    /// it for the chat on screen, as when that chat is gone.
    func newSession(keepingDraft: Bool = true) async {
        stashSelectedSession()
        if keepingDraft { parkDraft() }
        restoreDraft(for: nil)
        selectedSession = nil
        transcript.removeAll()
        composerPanel = nil
        draftProject = nil
        startDraftFromDefaults()
    }

    /// A new chat starts with the default model and effort, not the last chat's.
    private func startDraftFromDefaults() {
        guard selectedSession == nil, let modelDefaults else { return }
        models = modelDefaults.options
        selectedModel = modelDefaults.model
        draftFastPick = false
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

    /// Create the engine session for a draft in `folder`, keeping the model
    /// picked for it.
    private func createSession(in folder: URL) async -> String? {
        guard let connection = engine?.connection else { return nil }
        return await createSession(in: folder, on: connection, selecting: true)
    }

    /// `selecting` false leaves the chat on screen as it is: the new chat is
    /// kept open off screen, with its own model list, for a background launch.
    private func createSession(in folder: URL, on connection: ACPConnection, selecting: Bool) async -> String? {
        let wanted = DraftModelSettings(options: models, selectedModel: selectedModel)
        do {
            let result = try await connection.request(ACPMethod.sessionNew, params: ACPRequests.newSession(cwd: folder), timeout: .seconds(30))
            guard let id = result["sessionId"]?.stringValue else { throw ACPProtocolError.malformed("session/new has no sessionId") }
            sessionProjects[id] = folder.path
            var options: ModelOptions
            if selecting {
                selectedSession = id
                applyConfig(result["configOptions"])
                options = models
            } else {
                options = result["configOptions"].map(ModelOptions.init(configOptions:)) ?? models
            }
            // The draft's model, effort and Fast, sent to this session by id
            // before the first prompt, so a chat picked meanwhile is untouched.
            for _ in 0..<3 {
                guard let change = wanted.nextChange(for: options),
                      let next = await send(change, to: id) else { break }
                options = next
            }
            if selecting {
                stashSelectedSession()
            } else {
                openChats.stash(id, transcript: Transcript(), models: options, selectedModel: options.current)
            }
            return id
        } catch {
            self.error = String(describing: error)
            return nil
        }
    }

    /// Read the session list again from its start, as far as the sidebar had
    /// read it, so a refresh never takes rows away from under the reader.
    func refreshSessions() async {
        guard let connection = engine?.connection, capabilities?.listSessions == true else { return }
        // Refreshes overlap when the scope or project changes: a slow All
        // listing must not land after, and overwrite, a later Project one. The
        // scope is read once so every page comes from the same listing.
        let request = sessionListRequests.begin()
        let scope = showsAllProjects ? nil : workspace
        let keep = scope == listedScope ? sessionPages.listed.count : 0
        do {
            var pages = SessionPages()
            var read = 0
            while pages.refreshWantsMore(keeping: keep, pagesRead: read) {
                let params = ACPRequests.listSessions(cwd: scope, cursor: pages.cursor)
                let result = try await connection.request(ACPMethod.sessionList, params: params, timeout: .seconds(15))
                guard sessionListRequests.isCurrent(request) else { return }
                pages.append(SessionSummary.list(from: result), next: SessionSummary.nextCursor(from: result))
                read += 1
            }
            sessionPages = pages
            listedScope = scope
            moreSessionsFailed = false
            publishSessions()
            sessionsLoaded = true
            Task { await sweepRetention() }
        } catch {
            guard sessionListRequests.isCurrent(request) else { return }
            self.error = String(describing: error)
        }
    }

    /// Read the next page of chats, when the sidebar nears the end of those
    /// it has. A page whose chats are all listed already reads on.
    func loadMoreSessions() async {
        guard !loadingMoreSessions, !moreSessionsFailed, let connection = engine?.connection,
              capabilities?.listSessions == true, sessionPages.hasMore else { return }
        // A refresh started meanwhile replaces the listing this page belongs to.
        let request = sessionListRequests.generation
        let scope = listedScope
        loadingMoreSessions = true
        defer { loadingMoreSessions = false }
        do {
            var pages = sessionPages
            repeat {
                let params = ACPRequests.listSessions(cwd: scope, cursor: pages.cursor)
                let result = try await connection.request(ACPMethod.sessionList, params: params, timeout: .seconds(15))
                guard sessionListRequests.isCurrent(request), sessionPages.cursor != nil else { return }
                if pages.append(SessionSummary.list(from: result), next: SessionSummary.nextCursor(from: result)) > 0 { break }
            } while pages.hasMore
            sessionPages = pages
            publishSessions()
        } catch {
            // A cursor whose chat was deleted is refused; a refresh starts over.
            guard sessionListRequests.isCurrent(request) else { return }
            moreSessionsFailed = true
        }
    }

    private var readAllTask: Task<Void, Never>?

    /// A search or the archive filters the whole history, not only the pages
    /// read so far: read the rest behind it while it is shown.
    private func readAllSessionsForFilter() {
        guard readAllTask == nil else { return }
        readAllTask = Task { [weak self] in
            while let self, !Task.isCancelled, self.hasMoreSessions, !self.moreSessionsFailed,
                  !self.sessionFilter.trimmingCharacters(in: .whitespaces).isEmpty || self.showsArchived
                    || self.switcherSearches {
                await self.loadMoreSessions()
                // Another page load may be running; wait for it, not spin.
                if self.loadingMoreSessions { try? await Task.sleep(for: .milliseconds(50)) }
            }
            self?.readAllTask = nil
        }
    }

    /// Try the page that failed again, from a fresh listing.
    func retryMoreSessions() async {
        moreSessionsFailed = false
        await refreshSessions()
        await loadMoreSessions()
    }

    /// `sessions` from the pages read: chats started here and not listed yet
    /// on top, and pinned chats from pages not read yet kept in.
    private func publishSessions() {
        let listed = sessionPages.listed
        startedSessions = startedSessions.filter { id, _ in !listed.contains { $0.id == id } }
        let scope = listedScope?.path
        let pinned = sessionPages.unlistedPinned(savedPinned, pinned: pinnedSessions, scope: scope)
        sessions = SessionSummary.touched(SessionSummary.merging(listed + pinned, started: Array(startedSessions.values).filter {
            scope == nil || $0.cwd == scope
        }), lastActivity)
        hasMoreSessions = sessionPages.hasMore
        rememberPinned()
    }

    /// A turn just started or ended in `id`: it is the latest activity until
    /// the engine's list says so too.
    private func touchSession(_ id: String) {
        lastActivity[id] = Date()
        sessions = SessionSummary.touched(sessions, lastActivity)
    }

    /// Keep what the sidebar knows of each pinned chat for the next launch.
    private func rememberPinned() {
        let shown = Dictionary(sessions.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
        let saved = Dictionary(savedPinned.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
        let next = pinnedSessions.sorted().compactMap { shown[$0] ?? saved[$0] }
        guard next != savedPinned else { return }
        savedPinned = next
        UserDefaults.standard.set(try? JSONEncoder().encode(next), forKey: "pinnedSessionSummaries")
    }

    /// A page is open in the chat's place: input over it is not the chat's.
    var replacesChat: Bool { viewedAgent != nil || showsSettings }

    /// A sheet or panel is open over the chat, which keeps its keys.
    var coversChat: Bool {
        showsSettings || renamingSession != nil || contextSheet != nil || commandResult != nil || composerPanel != nil
            || showsSwitcher || showsVisualizationGuide || viewedImage != nil || viewedAgent != nil
    }

    /// The request shown for the selected session, if any.
    var pendingPermission: PermissionRequest? {
        permissionQueue.first { $0.sessionId == selectedSession }
    }

    /// Keep the composer's contents for the chat on screen.
    private func parkDraft() {
        let parked = ParkedDraft(text: draft, command: draftCommand, attachments: attachments, images: images)
        let empty = parked.text.isEmpty && parked.command == nil && parked.attachments.isEmpty && parked.images.isEmpty
        parkedDrafts[selectedSession ?? Self.newChatQueue] = empty ? nil : parked
    }

    /// Fill the composer with what was kept for `session`, or empty it.
    private func restoreDraft(for session: String?) {
        let parked = parkedDrafts.removeValue(forKey: session ?? Self.newChatQueue)
        draft = parked?.text ?? ""
        draftCommand = parked?.command
        attachments = parked?.attachments ?? []
        images = parked?.images ?? []
        composer.commandSelection = 0
    }

    /// A chat picked while the engine was still starting, opened once it is up.
    private var pendingSelection: SessionSummary?

    /// `reload`: open the shown chat again, as it is after a move; its draft
    /// stays in the composer.
    func select(_ session: SessionSummary, reload: Bool = false) async {
        // Picked before the engine answered: the click used to do nothing.
        guard let connection = engine?.connection, capabilities != nil else {
            pendingSelection = session
            return
        }
        pendingSelection = nil
        guard capabilities?.loadSession == true, reload || session.id != selectedSession else { return }
        // A summary without its folder, as a pinned chat restored before the
        // list arrives, takes the listed one's.
        let session = session.cwd.isEmpty ? (sessions.first { $0.id == session.id } ?? session) : session
        if !reload {
            stashSelectedSession()
            parkDraft()
            restoreDraft(for: session.id)
        }
        // An open terminal stays open, showing this chat's folder's shells.
        if !session.cwd.isEmpty { sessionProjects[session.id] = session.cwd }
        if let open = openChats.snapshot(session.id) {
            selectedSession = session.id
            transcript = open.transcript
            models = open.models
            selectedModel = open.selectedModel
            return
        }
        selectedSession = session.id
        transcript.removeAll()
        // Picked again while its first load is still running: that load
        // shows it when it is in.
        guard !openChats.isLoading(session.id) else { return }
        guard let result = await loadInEngine(session, on: connection) else { return }
        // Another chat may have been picked while this one loaded.
        guard selectedSession == session.id, let loaded = openChats.snapshot(session.id) else { return }
        transcript = loaded.transcript
        applyConfig(result["configOptions"])
    }

    /// Make sure the engine has `session` open, without showing it: a chat
    /// sent a message from another one (#2527). True once it is open.
    private func load(_ session: SessionSummary) async -> Bool {
        if session.id == selectedSession || openChats.isOpen(session.id) { return true }
        guard let connection = engine?.connection else { return false }
        if openChats.isLoading(session.id) {
            // Another load of it is running; wait for that one.
            for _ in 0..<300 where openChats.isLoading(session.id) {
                try? await Task.sleep(for: .milliseconds(100))
            }
            return openChats.isOpen(session.id) || session.id == selectedSession
        }
        return await loadInEngine(session, on: connection) != nil
    }

    /// `session/load`, kept in `openChats`. It replays the transcript as
    /// `session/update` notifications before it answers. The replay builds
    /// off screen and shows at once: applied line by line, every replayed
    /// chunk redrew the whole chat. Nil when it failed.
    private func loadInEngine(_ session: SessionSummary, on connection: ACPConnection) async -> JSONValue? {
        openChats.beginLoad(session.id)
        defer {
            // A stopped engine already cleared these, and a new one may be
            // loading this chat again.
            if engine?.connection === connection { openChats.endLoad(session.id) }
        }
        do {
            let result = try await connection.request(ACPMethod.sessionLoad, params: ACPRequests.loadSession(id: session.id, cwd: URL(fileURLWithPath: session.cwd)), timeout: .seconds(30))
            await catchUpInbound(on: connection)
            // The engine replaced while this loaded has not opened it.
            guard engine?.connection === connection else { return nil }
            let options = result["configOptions"].map(ModelOptions.init(configOptions:)) ?? .empty
            openChats.finishLoad(session.id, models: options)
            return result
        } catch {
            self.error = String(describing: error)
            return nil
        }
    }

    /// A message written while a turn runs. It waits here and is sent when
    /// the chat's turn ends, as the iOS app queues them.
    struct QueuedPrompt: Identifiable, Equatable {
        var id = UUID()
        let sessionId: String
        /// What was typed, for the preview and for editing it again.
        let typed: String
        let text: String
        let images: [ImageAttachment]

        func moved(to session: String) -> QueuedPrompt {
            QueuedPrompt(id: id, sessionId: session, typed: typed, text: text, images: images)
        }
    }

    @Published var queuedPrompts: [QueuedPrompt] = []

    /// The selected chat's queued messages, oldest first.
    var selectedQueue: [QueuedPrompt] {
        queuedPrompts.filter { $0.sessionId == selectedSession || (startingChat && $0.sessionId == Self.newChatQueue) }
    }

    /// The queue of a chat still being created, before it has an id.
    static let newChatQueue = ""

    /// Messages queued while a chat was created move to it once it has an id.
    static func adoptQueued(_ queue: [QueuedPrompt], into session: String) -> [QueuedPrompt] {
        queue.map { $0.sessionId == newChatQueue ? $0.moved(to: session) : $0 }
    }

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

    /// Type a `!` command into the open project's terminal, showing the
    /// panel so its output is in view. The draft stays when it cannot run.
    private func runInTerminal(_ command: String) {
        switch terminal.run(command) {
        case .success(let session):
            draft = ""
            confirm("Ran in \(session.displayTitle)", systemImage: "terminal")
        case .failure(let failure):
            error = failure.description
        }
    }

    func submit() async {
        // A dead key's accent still composing is part of what was typed.
        MarkedText.commitInWindows()
        // App commands such as /terminal work without the engine.
        if let command = SlashCommand.appCommand(chip: draftCommand, draft: draft) {
            draftCommand = nil
            draft = ""
            await perform(command)
            return
        }
        // `! command` runs in the project's terminal, never through the
        // agent; `!!` sends a message that starts with one `!`.
        if draftCommand == nil, draft.hasPrefix("!") {
            switch InlineCommand.parse(draft.trimmingCharacters(in: .whitespacesAndNewlines)) {
            case .command(let command):
                runInTerminal(command)
                return
            case .invalid(let reason):
                error = reason
                return
            case .message(let text):
                draft = text
            }
        }
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
        // Commands that need no turn run now, even while the chat is busy;
        // the rest queue behind its turn like a message.
        switch SlashCommand.dispatch(typed, in: commands, plain: attachments.isEmpty && images.isEmpty) {
        case .app(let command):
            draft = ""
            await perform(command)
            return
        case .cancel:
            draft = ""
            if busy { await cancel() } else { confirm("Nothing to cancel", systemImage: "xmark.circle.fill") }
            return
        case .report where !startingChat && selectedSession != nil:
            draft = ""
            await runReport(typed, in: selectedSession!)
            return
        case .report, .prompt:
            break
        }
        let folder = chatProject
        let text = Self.prompt(typed, attaching: attachments, in: folder)
        let sentImages = images
        if busy {
            guard let id = startingChat ? Self.newChatQueue : selectedSession else { return }
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
        // Until the new chat's turn starts, ⏎ and ⌘⏎ queue behind this message.
        if created { startingChat = true }
        guard let id = created ? await createSession(in: folder) : selectedSession else {
            startingChat = false
            // Nothing was sent: what was queued behind it goes back too.
            let queued = queuedPrompts.filter { $0.sessionId == Self.newChatQueue }
            queuedPrompts.removeAll { $0.sessionId == Self.newChatQueue }
            draft = ([typed] + queued.map(\.typed)).joined(separator: "\n\n")
            images = sentImages + queued.flatMap(\.images)
            return
        }
        if created {
            startedSessions[id] = .started(id: id, cwd: folder.path, prompt: typed, model: selectedModel)
            await refreshSessions()
            queuedPrompts = Self.adoptQueued(queuedPrompts, into: id)
            // `run` marks the turn before it waits, so busy never drops between.
            startingChat = false
        }
        await run(QueuedPrompt(sessionId: id, typed: typed, text: text, images: sentImages))
    }

    /// Why each chat's last turn stopped, when the provider or the connection
    /// stopped it and `continue` picks it up.
    @Published private(set) var stoppedTurns: [String: String] = [:]

    /// Continue the shown chat's stopped turn where it stopped.
    func continueStoppedTurn() async {
        guard !busy, let id = selectedSession, stoppedTurns[id] != nil else { return }
        await run(QueuedPrompt(sessionId: id, typed: "continue", text: "continue", images: []))
    }

    /// Run one prompt as the chat's turn, then the chat's next queued message,
    /// unless the turn was stopped.
    private func run(_ prompt: QueuedPrompt) async {
        guard let connection = engine?.connection else { return }
        let id = prompt.sessionId
        stoppedTurns[id] = nil
        let isCommand = prompt.text == prompt.typed && prompt.images.isEmpty
            && SlashCommand.invoked(by: prompt.typed, in: commands)?.action == .execute
        if isCommand {
            // Its answer is shown natively, not added to the chat, and most
            // answer at once: the chat never says Working, and a progress toast
            // shows only for a command that takes a moment, such as /compact.
            let toast = Toast(style: .progress, systemImage: "terminal", title: "Running \(prompt.typed)")
            runningCommands[id] = RunningCommand(input: prompt.typed, toast: toast.id)
            Task { @MainActor [weak self] in
                try? await Task.sleep(for: .milliseconds(400))
                guard let self, self.runningCommands[id]?.toast == toast.id else { return }
                self.showToast(toast)
            }
        } else {
            let markers = prompt.images.compactMap { $0.writePreview() }.map(ImageMarkers.marker)
            let shown = ([prompt.text] + markers).filter { !$0.isEmpty }.joined(separator: "\n")
            appendToTranscript(of: id) {
                $0.beginTurn(at: Date())
                $0.appendUser(shown, at: Date())
            }
        }
        // Ended exactly once below however the turn ends, so the chat cannot
        // keep spinning after its answer.
        let turn = isCommand ? nil : turnActivity.begin(id)
        if turn != nil { if id == selectedSession { pollAgents() } else { pollBackgroundAgents() } }
        // The chat in use moves to the top as its turn starts, not only once
        // a long turn ends (#2418); the end touches it again below.
        touchSession(id)
        var stopped = false
        var cancelled = false
        var failure: String?
        do {
            let result = try await connection.request(ACPMethod.sessionPrompt, params: ACPRequests.prompt(sessionId: id, text: prompt.text, images: prompt.images))
            await catchUpInbound(on: connection)
            stopped = result["stopReason"]?.stringValue == "cancelled"
            cancelled = stopped
            if stopped { failure = "Cancelled." }
            requeueSteers(in: id, unused: ACPRequests.unusedSteers(in: result))
        } catch {
            requeueSteers(in: id, unused: nil)
            self.error = String(describing: error)
            stopped = true
            failure = "The turn failed."
            TurnNotifier.shared.notify(session: id, title: chatTitle(id), body: "The turn failed: \(error)")
        }
        // The turn is over, so the engine no longer waits on its requests.
        permissionQueue.removeAll { $0.sessionId == id }
        // A thought the turn ended on stops its clock, and no card keeps spinning.
        appendToTranscript(of: id) { $0.finishTurn(at: Date(), failure: failure, cancelled: cancelled) }
        foldTurnsOpenedWhileRunning(in: id)
        if isCommand {
            finishCommand(in: id)
        } else if !stopped {
            notifyFinished(id)
        }
        // The chat just used moves to the top at once; the list refreshes
        // behind the answer: waiting for it kept the chat spinning until the
        // engine listed every session.
        touchSession(id)
        Task { await refreshSessions() }
        if id == selectedSession { Task { await refreshPlanUsage() } }
        if let turn { turnActivity.end(turn) }
        // A queued message starts at once, with no wait in between, so
        // "Working" stays up across the hand-off instead of blinking off.
        guard !stopped, let next = queuedPrompts.first(where: { $0.sessionId == id }) else { return }
        removeQueued(next)
        await run(next)
    }

    /// Messages handed to each chat's running turn, until that turn ends.
    private var steered: [String: [QueuedPrompt]] = [:]

    /// ⌘⏎ can hand the shown chat's running turn a message.
    var canSteer: Bool { isTurnRunning && capabilities?.steer == true }

    /// ⌘⏎: while the shown chat's turn runs, the composer's message steers
    /// it, read after its current step instead of waiting for the turn to
    /// end; with nothing typed, the oldest queued message goes instead.
    /// Commands, images, and a turn that cannot take a steer right now queue
    /// as ⏎ does. With no turn running, ⌘⏎ sends like ⏎.
    func steerOrSubmit() async {
        MarkedText.commitInWindows()
        guard canSteer, let id = selectedSession else { return await submit() }
        let typed = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        if typed.isEmpty, attachments.isEmpty, images.isEmpty, draftCommand == nil {
            if let next = selectedQueue.first { await sendNow(next) }
            return
        }
        guard draftCommand == nil, images.isEmpty, !typed.hasPrefix("/"), !typed.hasPrefix("!") else {
            return await submit()
        }
        let prompt = QueuedPrompt(sessionId: id, typed: typed, text: Self.prompt(typed, attaching: attachments, in: chatProject), images: [])
        draft = ""
        attachments = []
        if await !steer(prompt) {
            queuedPrompts.append(prompt)
            confirm("Queued: the turn is waiting for you", systemImage: "text.line.last.and.arrowtriangle.forward")
        }
    }

    /// A queued message's Send Now, and ⌘⏎ with nothing typed: the running
    /// turn reads it after its current step. The rest of the queue keeps its
    /// order.
    func sendNow(_ prompt: QueuedPrompt) async {
        guard prompt.images.isEmpty else {
            return confirm("Images wait for the next turn", systemImage: "photo")
        }
        if await steer(prompt) {
            removeQueued(prompt)
        } else {
            confirm("The turn can't take it now. It stays queued", systemImage: "text.line.last.and.arrowtriangle.forward")
        }
    }

    /// Hand `prompt` to its chat's running turn. False when the turn did not
    /// take it, as while it waits on an approval or a question.
    private func steer(_ prompt: QueuedPrompt) async -> Bool {
        guard let connection = engine?.connection, runningSessions.contains(prompt.sessionId) else { return false }
        let params = ACPRequests.steer(sessionId: prompt.sessionId, text: prompt.text)
        guard let result = try? await connection.request(ACPMethod.steer, params: params, timeout: .seconds(10)),
              result["accepted"]?.boolValue == true else { return false }
        steered[prompt.sessionId, default: []].append(prompt)
        appendToTranscript(of: prompt.sessionId) { $0.appendUser(prompt.text, at: Date()) }
        confirm("Steering: read after the current step", systemImage: "arrow.turn.down.right")
        return true
    }

    /// The turn ended: steers it never read leave the chat and run next,
    /// first in its queue, in the order they were sent, so each shows where
    /// its answer starts. `unused` nil means the turn failed and any of them
    /// may be unread.
    private func requeueSteers(in session: String, unused: [String]?) {
        guard let sent = steered.removeValue(forKey: session) else { return }
        var left = unused?.map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
        let unread = sent.filter { prompt in
            guard left != nil else { return true }
            guard let index = left!.firstIndex(of: prompt.text.trimmingCharacters(in: .whitespacesAndNewlines)) else { return false }
            left!.remove(at: index)
            return true
        }
        appendToTranscript(of: session) { transcript in
            for prompt in unread { transcript.removeLastUser(prompt.text) }
        }
        queuedPrompts.insert(contentsOf: unread, at: queuedPrompts.firstIndex { $0.sessionId == session } ?? queuedPrompts.endIndex)
    }

    /// Send a report command now, beside any running turn: it never joins
    /// the queue, and its answer shows for the chat it was typed in.
    private func runReport(_ typed: String, in session: String) async {
        guard let connection = engine?.connection else { return }
        let id = pendingReports.begin(typed, in: session)
        var failed = false
        do {
            _ = try await connection.request(ACPMethod.sessionPrompt, params: ACPRequests.prompt(sessionId: session, text: typed), timeout: .seconds(30))
        } catch {
            self.error = String(describing: error)
            failed = true
        }
        guard let report = pendingReports.finish(id, in: session), !report.answered, !failed else { return }
        confirm("\(typed) done", systemImage: "checkmark.circle.fill")
    }

    /// Show a report's answer, which may arrive mid-turn: it never reaches the chat.
    private func showReport(_ update: SessionUpdate, input: String, in session: String) {
        switch update {
        case .contextReport(_, let context):
            if session == selectedSession { contextSheet = context }
        case .agentText(let text, _, _):
            showCommandResult(CommandResult(input: input, output: text), in: session)
        default:
            break
        }
    }

    /// Show a host command's answer: a toast when it is one short line, its
    /// sheet when longer, unless it already opened natively.
    private func finishCommand(in session: String) {
        guard let command = runningCommands.removeValue(forKey: session) else { return }
        dismissToast(command.toast)
        guard !command.answered else { return }
        showCommandResult(CommandResult(input: command.input, output: command.output), in: session)
    }

    private func showCommandResult(_ result: CommandResult, in session: String) {
        if result.content.isEmpty {
            confirm("\(result.input) done", systemImage: "checkmark.circle.fill")
        } else if result.isBrief {
            showToast(Toast(style: .success, systemImage: "terminal", title: result.title, message: result.content))
        } else if session == selectedSession, NSApp.isActive {
            commandResult = result
        } else {
            showToast(Toast(systemImage: "terminal", title: result.title, message: chatTitle(session), result: result))
        }
    }

    /// Route a running host command's answer away from the chat. False when
    /// the update is not part of one.
    private func captureCommandUpdate(_ update: SessionUpdate, in session: String) -> Bool {
        guard var command = runningCommands[session] else { return false }
        switch update {
        case .agentText(let text, _, _):
            command.output += text
        case .contextReport(_, let context):
            command.answered = true
            if session == selectedSession { contextSheet = context }
        case .compaction:
            // `/compact` answers with the chat's compaction card, which is
            // the signal; the earlier messages stay above it.
            command.answered = true
            appendToTranscript(of: session) { $0.apply(update, at: Date()) }
        default:
            return false
        }
        runningCommands[session] = command
        return true
    }

    /// The selected chat's transcript is live; another open chat's is stashed.
    private func appendToTranscript(of session: String, _ update: (inout Transcript) -> Void) {
        if session == selectedSession {
            update(&transcript)
        } else {
            openChats.updateTranscript(of: session, update)
        }
    }

    func cancel() async {
        guard let connection = engine?.connection, let id = selectedSession, busy else { return }
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

    static let autoApproveKey = "autoApproveTools"

    /// Tool approvals are answered "allow" without asking. Questions still wait.
    static var autoApproves: Bool { UserDefaults.standard.bool(forKey: autoApproveKey) }

    /// Turn auto-approve on or off; turning it on also approves every tool
    /// request already waiting.
    func setAutoApprove(_ on: Bool) async {
        UserDefaults.standard.set(on, forKey: Self.autoApproveKey)
        guard on else { return }
        confirm("Tools are approved automatically", systemImage: "checkmark.shield.fill")
        for request in permissionQueue {
            if let option = request.autoApproveOption { await answer(request, with: option) }
        }
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
            let previous = selectedModel
            selectedModel = model   // applied when the draft becomes a session
            if let modelDefaults {
                models = modelDefaults.draftOptions(models, switchingTo: model, fastPick: draftFastPick)
            } else if model != previous {
                // Without the defaults it is not known whether this model
                // offers Fast, so no toggle claims it does.
                models.fast = nil
            }
            return
        }
        await send(.model(model), to: session)
    }

    /// Reasoning effort for the chat. A draft keeps it until its session exists.
    func setEffort(_ effort: String) async {
        guard let session = selectedSession else {
            models.effort = effort
            return
        }
        await send(.effort(effort), to: session)
    }

    /// The Fast choice made on the draft, kept while a model without Fast is
    /// picked for it.
    private var draftFastPick = false

    /// Fast mode for the chat (ChatGPT plans only). A draft keeps it until
    /// its session exists.
    func setFast(_ enabled: Bool) async {
        guard let session = selectedSession else {
            draftFastPick = enabled
            if models.offersFast { models.fast = enabled }
            return
        }
        await send(.fast(enabled), to: session)
    }

    /// How the chat's model writes tool calls. The engine saves it on the
    /// model's profile, so it needs a session; a draft has none to send to.
    func setToolProtocol(_ id: String) async {
        guard let session = selectedSession, id != models.toolProtocol else { return }
        await send(.toolProtocol(id), to: session)
    }

    /// Change one of `session`'s model settings and return its options after.
    /// The reply goes to that session even when another chat is on screen by
    /// then, so a slow reply never shows one chat's settings on another.
    @discardableResult
    private func send(_ change: DraftModelSettings.Change, to session: String) async -> ModelOptions? {
        guard let connection = engine?.connection else { return nil }
        do {
            let result = try await connection.request(
                ACPMethod.sessionSetConfigOption, params: change.params(sessionId: session), timeout: .seconds(15))
            let current = session == selectedSession ? models : openChats.snapshot(session)?.models
            let options = result["configOptions"].map(ModelOptions.init(configOptions:))
                ?? current.map(change.applied(to:)) ?? .empty
            show(options, for: session, config: result["configOptions"])
            return options
        } catch {
            self.error = String(describing: error)
            return nil
        }
    }

    /// Model options the engine sent for `session`: shown when it is the
    /// chat on screen, else kept for when it is picked again.
    private func show(_ options: ModelOptions, for session: String, config: JSONValue? = nil) {
        if session == selectedSession {
            if let config { applyConfig(config) } else {
                models = options
                selectedModel = options.current
            }
        } else {
            openChats.updateModels(of: session, options)
        }
    }

    func stop() {
        releaseInboundWaiters()
        openChats.reset()
        remotePollTask?.cancel()
        remotePollTask = nil
        remoteSessionID = nil
        showsRemote = false
        inboundTask?.cancel()
        inboundTask = nil
        if let engine { DispatchQueue.global().async { engine.stop() } }
        engine = nil
        capabilities = nil
        selectedSession = nil
        // The engine is being stopped, so its waiting requests die with it.
        permissionQueue = []
        endAllTurns()
    }

    /// The engine is gone, and every turn with it: no card one of them was
    /// running may keep spinning for an answer that cannot come.
    private func endAllTurns(failure: String? = "The engine stopped.") {
        for session in turnActivity.sessions {
            appendToTranscript(of: session) { $0.finishTurn(at: Date(), failure: failure) }
        }
        turnActivity.reset()
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
            let (pickedModel, pickedEffort, pickedFast) = (selectedModel, models.effort, models.fast)
            applyConfig(result["configOptions"])
            if selectedSession == nil {
                if let pickedModel, models.choices.contains(where: { $0.id == pickedModel }) { selectedModel = pickedModel }
                if let pickedEffort, models.efforts.contains(where: { $0.id == pickedEffort }) { models.effort = pickedEffort }
                if let pickedFast, models.offersFast { models.fast = pickedFast }
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
            endAllTurns()
            if engine != nil { error = reason }
        case .notification(let method, let params) where method == ACPMethod.sessionUpdate
            && params["update"]?["sessionUpdate"]?.stringValue == "available_commands_update":
            // Commands are the same for every session, so take them from any.
            if case .availableCommands(let list) = SessionUpdate(notificationParams: params) { commands = SlashCommand.withAppCommands(list) }
        case .notification(let method, let params) where method == ACPMethod.sessionUpdate:
            guard let sessionId = params["sessionId"]?.stringValue else { return }
            if SessionUpdate.isReport(notificationParams: params), let input = pendingReports.answer(in: sessionId) {
                showReport(SessionUpdate(notificationParams: params), input: input, in: sessionId)
                return
            }
            if captureCommandUpdate(SessionUpdate(notificationParams: params), in: sessionId) { return }
            // A chat being opened collects its replay off screen, shown or
            // not; a chat open in the background keeps its transcript current.
            let route = openChats.route(SessionUpdate(notificationParams: params),
                                        isReport: SessionUpdate.isReport(notificationParams: params),
                                        in: sessionId, selected: selectedSession)
            if route == .replay { return }
            if case .stopped(let text) = SessionUpdate(notificationParams: params) {
                // Kept until the chat sends again, so a turn that stopped while
                // the reader was away still says so when they come back.
                stoppedTurns[sessionId] = text
                if sessionId != selectedSession {
                    showToast(Toast(style: .warning, systemImage: "exclamationmark.octagon.fill", title: text,
                                    message: chatTitle(sessionId), session: sessionId))
                }
                return
            }
            if case .notice(let text) = SessionUpdate(notificationParams: params) {
                showToast(Toast(style: .warning, systemImage: "exclamationmark.octagon.fill", title: text,
                                message: sessionId == selectedSession ? nil : chatTitle(sessionId),
                                session: sessionId == selectedSession ? nil : sessionId))
                return
            }
            guard route == .shown else { return }
            let update = SessionUpdate(notificationParams: params)
            switch update {
            case .title: await refreshSessions()
            case .configOptions(let options): applyConfig(options)
            case .contextReport(_, let context):
                transcript.apply(update, at: Date())
                contextSheet = context
            default:
                transcript.apply(update, at: Date())
            }
        case .request(let id, let method, let params) where method == ACPMethod.requestPermission:
            if let request = PermissionRequest(rpcId: id, params: params) {
                permissionQueue.append(request)
                if Self.autoApproves, let option = request.autoApproveOption {
                    await answer(request, with: option)
                    return
                }
                if NSApp.isActive, request.sessionId != selectedSession {
                    showToast(Toast(style: .warning, systemImage: request.question == nil ? "hand.raised.fill" : "questionmark.bubble.fill",
                                    title: request.question == nil ? "Approval needed" : "Question waiting",
                                    message: chatTitle(request.sessionId), session: request.sessionId))
                }
                TurnNotifier.shared.notify(
                    session: request.sessionId,
                    title: request.question == nil ? "Approval needed" : "RustCode has a question",
                    body: "\(chatTitle(request.sessionId)): \(request.question?.text ?? request.title)")
            }
            else { try? await engine?.connection.respond(id: id, error: .invalidParams(method)) }
        case .request(let id, let method, let params) where method == ACPMethod.preview:
            // Show the page the agent is about to use, and answer off the
            // inbound loop: the first request waits for the user's approval.
            guard let connection = engine?.connection else { return }
            if !showsPreview { togglePreview() }
            if let session = params["sessionId"]?.stringValue, preview.agent.access.origin(for: session) == nil {
                TurnNotifier.shared.notify(session: session, title: "Preview access requested",
                                           body: "\(chatTitle(session)) wants to use the localhost preview.")
            }
            Task { [weak self] in
                guard let result = await self?.preview.runAgentRequest(params) else { return }
                try? await connection.respond(id: id, result: result)
            }
        case .request(let id, let method, _):
            try? await engine?.connection.respond(id: id, error: .methodNotFound(method))
        default: break
        }
    }
}
