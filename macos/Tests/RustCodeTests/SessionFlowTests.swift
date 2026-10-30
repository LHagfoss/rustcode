import AppKit
import XCTest
import RustCodeKit
import RustCodeTerminal
@testable import RustCode

/// The model against a scripted ACP engine: chats are created, prompted,
/// switched and reopened the way the sidebar drives them.
@MainActor
final class SessionFlowTests: XCTestCase {
    private var directory: URL!
    private var saved: [String: Any] = [:]
    private static let keys = ["enginePath", "workspacePath", "lastConfigOptions", "pinnedSessions", "pinnedSessionSummaries", "archivedSessions"]

    override func setUp() async throws {
        _ = NSApplication.shared
        directory = FileManager.default.temporaryDirectory.appendingPathComponent("rustcode-flow-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let engine = directory.appendingPathComponent("fake-engine")
        try Self.fakeEngine.write(to: engine, atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: engine.path)
        for key in Self.keys { saved[key] = UserDefaults.standard.object(forKey: key) }
        for key in Self.keys { UserDefaults.standard.removeObject(forKey: key) }
        UserDefaults.standard.set(engine.path, forKey: "enginePath")
        UserDefaults.standard.set(directory.path, forKey: "workspacePath")
    }

    override func tearDown() async throws {
        for key in Self.keys {
            if let value = saved[key] { UserDefaults.standard.set(value, forKey: key) } else { UserDefaults.standard.removeObject(forKey: key) }
        }
        try? FileManager.default.removeItem(at: directory)
    }

    private func started() async throws -> DesktopModel {
        let model = DesktopModel()
        await model.start()
        XCTAssertNil(model.error)
        try await until { model.sessionsLoaded }
        return model
    }

    private func until(_ condition: @MainActor () -> Bool, file: StaticString = #filePath, line: UInt = #line) async throws {
        for _ in 0..<200 {
            if condition() { return }
            try await Task.sleep(for: .milliseconds(20))
        }
        XCTFail("timed out", file: file, line: line)
    }

    private func send(_ text: String, in model: DesktopModel) async {
        model.draft = text
        await model.submit()
    }

    /// #2178: a new chat's first answer and its follow-up both clear the
    /// sidebar spinner.
    func testNewChatAndFollowUpLeaveNoSpinner() async throws {
        let model = try await started()
        await model.newSession()
        await send("first", in: model)
        let id = try XCTUnwrap(model.selectedSession)
        XCTAssertTrue(model.runningSessions.isEmpty)
        await send("follow-up", in: model)
        XCTAssertTrue(model.runningSessions.isEmpty)
        XCTAssertEqual(model.transcript.items.count, 4)
        // The chat is listed once, so exactly one row can be selected.
        try await until { model.sessions.filter { $0.id == id }.count == 1 }
        XCTAssertEqual(model.sessions.filter { $0.id == id }.count, 1)
        model.stop()
    }

    /// #2652: ⌘⇧⏎ starts the message as a new chat off screen. The draft on
    /// screen stays a fresh, empty new chat; launching twice in a row makes
    /// two chats; a command is not launched.
    func testLaunchingInTheBackgroundKeepsTheComposerFree() async throws {
        let model = try await started()
        await model.newSession()
        let before = Set(model.sessions.map(\.id))
        model.draft = "first task"
        await model.launchInBackground()
        model.draft = "second task"
        await model.launchInBackground()
        XCTAssertNil(model.selectedSession, "the composer stays on a new chat")
        XCTAssertEqual(model.draft, "")
        XCTAssertTrue(model.transcript.items.isEmpty)
        try await until { Set(model.sessions.map(\.id)).subtracting(before).count == 2 }
        try await until { model.runningSessions.isEmpty }
        XCTAssertNil(model.error)

        model.draft = "/compact"
        XCTAssertFalse(model.canLaunchInBackground)
        model.stop()
    }

    /// A follow-up typed while the first answer streams queues, runs, and
    /// still leaves the chat idle.
    func testQueuedFollowUpLeavesNoSpinner() async throws {
        let model = try await started()
        await model.newSession()
        model.draft = "first"
        let first = Task { await model.submit() }
        try await until { model.busy }
        await send("queued", in: model)
        await first.value
        try await until { model.runningSessions.isEmpty && model.queuedPrompts.isEmpty }
        XCTAssertEqual(model.transcript.items.count, 4)
        model.stop()
    }

    /// Picking another chat while one still loads must not remember the
    /// first as empty: opened again, it shows its messages.
    func testSwitchingAwayMidLoadKeepsTheChat() async throws {
        let model = try await started()
        let slow = try XCTUnwrap(model.sessions.first { $0.id == "slow" })
        let quick = try XCTUnwrap(model.sessions.first { $0.id == "quick" })
        let loading = Task { await model.select(slow) }
        try await until { model.isLoadingSelected }
        await model.select(quick)
        XCTAssertEqual(model.transcript.items.count, Self.replayed)
        await loading.value
        XCTAssertEqual(model.selectedSession, "quick")
        XCTAssertEqual(model.transcript.items.count, Self.replayed)
        await model.select(slow)
        XCTAssertEqual(model.transcript.items.count, Self.replayed)
        XCTAssertFalse(model.isLoadingSelected)
        model.stop()
    }

    /// #2595: the model, effort and Fast picked on an empty draft are shown
    /// before the first message and applied before its prompt runs.
    func testDraftModelSettingsApplyBeforeTheFirstPrompt() async throws {
        let model = try await started()
        await model.newSession()
        XCTAssertEqual(model.models.fast, false, "Fast shows on the empty draft of a model that offers it")
        await model.setEffort("high")
        await model.setFast(true)
        await send("first", in: model)
        let id = try XCTUnwrap(model.selectedSession)
        XCTAssertEqual(model.models.effort, "high")
        XCTAssertEqual(model.models.fast, true)
        let reply = model.transcript.items.compactMap { item -> String? in
            if case .assistant(let text) = item.content { return text } else { return nil }
        }.last
        XCTAssertEqual(reply, "reply to first [plan high fast]")
        // The next new chat starts from the defaults again, Fast off.
        await model.newSession()
        XCTAssertNotEqual(model.selectedSession, id)
        model.stop()
    }

    /// #2595: a setting's reply that lands after another chat was picked
    /// stays with the chat it was made for.
    func testSlowSettingReplyStaysWithItsChat() async throws {
        let model = try await started()
        let quick = try XCTUnwrap(model.sessions.first { $0.id == "quick" })
        let slow = try XCTUnwrap(model.sessions.first { $0.id == "slow" })
        await model.select(quick)
        XCTAssertEqual(model.models.effort, "default")
        let setting = Task { await model.setEffort("high") }
        await Task.yield()
        await model.select(slow)
        await setting.value
        XCTAssertEqual(model.selectedSession, "slow")
        XCTAssertEqual(model.models.effort, "default", "quick's reply must not show on slow")
        await model.select(quick)
        XCTAssertEqual(model.models.effort, "high")
        // A model without Fast shows no toggle.
        await model.setModel("local")
        XCTAssertFalse(model.models.offersFast)
        model.stop()
    }

    /// #2258: each chat keeps its own unsent draft, and so does a new chat.
    func testDraftsStayWithTheirChat() async throws {
        let model = try await started()
        let quick = try XCTUnwrap(model.sessions.first { $0.id == "quick" })
        let slow = try XCTUnwrap(model.sessions.first { $0.id == "slow" })
        model.draft = "for the new chat"
        await model.select(quick)
        XCTAssertEqual(model.draft, "")
        model.draft = "for quick"
        await model.select(slow)
        XCTAssertEqual(model.draft, "")
        await model.select(quick)
        XCTAssertEqual(model.draft, "for quick")
        await model.newSession()
        XCTAssertEqual(model.draft, "for the new chat")
        model.draft = ""
        await model.select(quick)
        XCTAssertEqual(model.draft, "for quick")
        model.draft = ""
        await model.newSession()
        XCTAssertEqual(model.draft, "", "a cleared draft stays cleared")
        await model.select(quick)
        XCTAssertEqual(model.draft, "", "a cleared draft stays cleared")
        model.stop()
    }

    /// #2304: the terminal shows the shells of the chat on screen's folder.
    /// An open panel stays open across a switch, and each folder keeps its
    /// own shells for when its chat is opened again.
    func testTerminalFollowsTheChatsFolder() async throws {
        let other = directory.appendingPathComponent("other", isDirectory: true)
        try FileManager.default.createDirectory(at: other, withIntermediateDirectories: true)
        let model = try await started()
        defer { model.terminal.manager.terminateAll() }
        let manager = model.terminal.manager
        let quick = try XCTUnwrap(model.sessions.first { $0.id == "quick" })
        let elsewhere = try XCTUnwrap(model.sessions.first { $0.id == "elsewhere" })

        await model.select(quick)
        model.terminal.open()
        XCTAssertEqual(manager.project, TerminalSessionManager.standardize(model.chatProject.path))
        let shell = try XCTUnwrap(manager.selectedSession)

        await model.select(elsewhere)
        XCTAssertTrue(model.terminal.isShown, "an open terminal stays open")
        XCTAssertEqual(manager.project, "/tmp/rustcode-elsewhere")
        XCTAssertTrue(manager.projectSessions.isEmpty, "no shell of another folder is shown, none is started")

        await model.select(quick)
        XCTAssertEqual(manager.selectedSession?.id, shell.id, "the chat's folder keeps its shell")

        await model.newSession(in: other)
        XCTAssertEqual(manager.project, TerminalSessionManager.standardize(other.path))
        await model.newSession()
        XCTAssertEqual(manager.project, TerminalSessionManager.standardize(directory.path))
        model.terminal.hide()
        await model.select(elsewhere)
        XCTAssertFalse(model.terminal.isShown, "a closed terminal stays closed")
        model.stop()
    }

    /// #2306: a new default project leaves the open chat in its own folder,
    /// open and with its unsent text; a folder that cannot be read changes
    /// nothing.
    func testChangingTheDefaultKeepsTheOpenChat() async throws {
        let other = directory.appendingPathComponent("other", isDirectory: true)
        try FileManager.default.createDirectory(at: other, withIntermediateDirectories: true)
        let model = try await started()
        let elsewhere = try XCTUnwrap(model.sessions.first { $0.id == "elsewhere" })
        model.draft = "for the new chat"
        await model.select(elsewhere)
        model.draft = "for elsewhere"

        await model.setDefaultProject(other)
        XCTAssertEqual(model.workspace, other)
        XCTAssertEqual(UserDefaults.standard.string(forKey: "workspacePath"), other.path)
        XCTAssertEqual(model.selectedSession, "elsewhere")
        XCTAssertEqual(model.chatProject.path, "/tmp/rustcode-elsewhere")
        XCTAssertEqual(model.draft, "for elsewhere")
        XCTAssertNil(model.error)
        await model.newSession()
        XCTAssertEqual(model.chatProject, other, "new chats start in the new default")
        XCTAssertEqual(model.draft, "for the new chat")

        await model.setDefaultProject(directory.appendingPathComponent("missing", isDirectory: true))
        XCTAssertEqual(model.workspace, other)
        XCTAssertEqual(model.toasts.last?.style, .warning)
        model.stop()
    }

    /// #2255, #2256: ⌘⏎ steers the running turn instead of queueing; with
    /// nothing typed it sends the first queued message now; a steer the turn
    /// ended before reading runs next, shown once.
    func testCommandReturnSteersTheRunningTurn() async throws {
        let model = try await started()
        await model.newSession()
        model.draft = "long turn"
        let first = Task { await model.submit() }
        try await until { model.canSteer }
        model.draft = "steer read"
        await model.steerOrSubmit()
        XCTAssertTrue(model.selectedQueue.isEmpty, "a steer does not queue")
        await send("queued unread", in: model)
        XCTAssertEqual(model.selectedQueue.map(\.typed), ["queued unread"], "⏎ still queues")
        await model.steerOrSubmit()
        XCTAssertTrue(model.selectedQueue.isEmpty, "⌘⏎ with nothing typed sends the queued message now")
        await first.value
        try await until { model.runningSessions.isEmpty && model.queuedPrompts.isEmpty }
        let typed = model.transcript.items.compactMap { item -> String? in
            if case .user(let text) = item.content { return text }
            return nil
        }
        XCTAssertEqual(typed, ["long turn", "steer read", "queued unread"])
        let replies = model.transcript.items.filter { if case .assistant = $0.content { true } else { false } }
        XCTAssertEqual(replies.count, 2, "the unread steer ran as the next turn")
        model.stop()
    }

    /// ↑ with an empty composer recalls the last message to edit and resend;
    /// it does nothing when the composer already holds text.
    func testUpArrowRecallsTheLastMessage() async throws {
        let model = try await started()
        await model.newSession()
        await send("first request", in: model)
        try await until { !model.busy }
        await send("second request", in: model)
        try await until { !model.busy }

        XCTAssertTrue(model.recallLastMessage())
        XCTAssertEqual(model.draft, "second request")

        // With text in the composer ↑ must not clobber it.
        model.draft = "work in progress"
        XCTAssertFalse(model.recallLastMessage())
        XCTAssertEqual(model.draft, "work in progress")
        model.stop()
    }

    /// ↑ takes a message still waiting in the queue back first, newest first,
    /// and it leaves the queue.
    func testUpArrowEditsTheNewestQueuedMessage() async throws {
        let model = try await started()
        await model.newSession()
        await send("sent request", in: model)
        try await until { !model.busy }
        let session = try XCTUnwrap(model.selectedSession)
        model.queuedPrompts = [
            .init(sessionId: session, typed: "older queued", text: "older queued", images: []),
            .init(sessionId: session, typed: "newer queued", text: "newer queued", images: []),
        ]

        XCTAssertTrue(model.recallLastMessage())
        XCTAssertEqual(model.draft, "newer queued")
        XCTAssertEqual(model.selectedQueue.map(\.typed), ["older queued"])
        model.stop()
    }

    /// Picking a chat back while its first load still runs shows it once it is in.
    func testReturningToALoadingChatShowsItWhenLoaded() async throws {
        let model = try await started()
        let slow = try XCTUnwrap(model.sessions.first { $0.id == "slow" })
        let quick = try XCTUnwrap(model.sessions.first { $0.id == "quick" })
        let loading = Task { await model.select(slow) }
        try await until { model.isLoadingSelected }
        await model.select(quick)
        await model.select(slow)
        XCTAssertTrue(model.isLoadingSelected)
        await loading.value
        XCTAssertEqual(model.selectedSession, "slow")
        XCTAssertEqual(model.transcript.items.count, Self.replayed)
        model.stop()
    }

    /// ⌘K: the chat picked in the switcher opens and the switcher closes;
    /// picking the open chat only closes it. Not over Settings.
    func testSwitcherOpensThePickedChat() async throws {
        let model = try await started()
        model.toggleSwitcher()
        XCTAssertTrue(model.showsSwitcher)
        XCTAssertTrue(model.coversChat, "the chat's keys stay out of the switcher's field")
        let results = SessionSwitcher.results(sessions: model.sessions, query: "qui", pinned: [])
        XCTAssertEqual(results.map(\.id), ["quick"])
        model.switchTo(results[0].session)
        XCTAssertFalse(model.showsSwitcher)
        try await until { model.selectedSession == "quick" && !model.isLoadingSelected }
        XCTAssertEqual(model.transcript.items.count, Self.replayed)

        model.toggleSwitcher()
        model.switchTo(results[0].session)
        XCTAssertFalse(model.showsSwitcher)
        XCTAssertEqual(model.selectedSession, "quick")
        XCTAssertEqual(model.transcript.items.count, Self.replayed)

        model.toggleSwitcher()
        model.toggleSwitcher()
        XCTAssertFalse(model.showsSwitcher, "⌘K again closes it")
        model.showsSettings = true
        model.toggleSwitcher()
        XCTAssertFalse(model.showsSwitcher)
        model.stop()
    }

    /// #2527: the composer's message goes to another chat, which runs it in
    /// its own context while the open chat stays as it was.
    func testSendingToAnotherChatRunsItThereAndKeepsThisOne() async throws {
        let model = try await started()
        let quick = try XCTUnwrap(model.sessions.first { $0.id == "quick" })
        let slow = try XCTUnwrap(model.sessions.first { $0.id == "slow" })
        await model.select(quick)
        model.draft = "check the logs"
        XCTAssertTrue(model.canSendDraftElsewhere)
        model.chooseChatForDraft()
        XCTAssertTrue(model.showsSwitcher && model.switcherSendsDraft)
        model.switchTo(slow)
        XCTAssertFalse(model.showsSwitcher)
        XCTAssertFalse(model.switcherSendsDraft, "the next ⌘K switches again")
        // Sent once the chat is open in the engine, then run to its end.
        try await until { model.toasts.contains { $0.session == "slow" && $0.title.hasPrefix("Sent to") } }
        try await until { model.runningSessions.isEmpty }
        XCTAssertEqual(model.draft, "")
        XCTAssertEqual(model.selectedSession, "quick", "this chat stays open")
        XCTAssertEqual(model.transcript.items.count, Self.replayed, "and gets nothing")
        await model.select(slow)
        XCTAssertTrue(model.transcript.items.contains { item in
            if case .assistant(let text) = item.content { text == "reply to check the logs" } else { false }
        })
        model.stop()
    }

    /// A busy chat gets the message after its turn, and one that cannot be
    /// opened gives it back to the composer.
    func testSendingToABusyChatQueuesAndAFailedOneKeepsTheDraft() async throws {
        let model = try await started()
        let quick = try XCTUnwrap(model.sessions.first { $0.id == "quick" })
        let slow = try XCTUnwrap(model.sessions.first { $0.id == "slow" })
        await model.select(slow)
        model.draft = "long task"
        let running = Task { await model.submit() }
        try await until { model.runningSessions.contains("slow") }
        await model.select(quick)
        model.draft = "after that"
        await model.sendDraft(to: slow)
        XCTAssertEqual(model.queuedPrompts.map(\.typed), ["after that"])
        XCTAssertEqual(model.queuedPrompts.first?.sessionId, "slow")
        await running.value
        try await until { model.queuedPrompts.isEmpty && model.runningSessions.isEmpty }

        model.draft = "lost?"
        await model.sendDraft(to: SessionSummary(id: "elsewhere", cwd: "/tmp/not-its-folder"))
        XCTAssertEqual(model.draft, "lost?", "not delivered, so nothing is lost")
        XCTAssertNotNil(model.error)
        model.stop()
    }

    /// #2210: ⌘P pins and unpins the chat shown at once, and never moves what
    /// is typed, or the follow-up sent next, to another chat.
    func testPinActsOnTheShownChatAndKeepsTheDraft() async throws {
        let model = try await started()
        await model.newSession()
        model.draft = "not sent yet"
        model.togglePinSelected()
        XCTAssertTrue(model.pinnedSessions.isEmpty, "a new chat has nothing to pin yet")
        XCTAssertNil(model.selectedSession)
        XCTAssertEqual(model.draft, "not sent yet")
        XCTAssertEqual(model.toasts.last?.title, "Nothing to pin yet")

        await send("first", in: model)
        let id = try XCTUnwrap(model.selectedSession)
        model.draft = "follow-up"
        model.togglePinSelected()
        XCTAssertEqual(model.pinnedSessions, [id])
        XCTAssertTrue(model.isSelectedPinned)
        XCTAssertEqual(model.selectedSession, id)
        XCTAssertEqual(model.draft, "follow-up")

        await model.submit()
        XCTAssertEqual(model.selectedSession, id, "the follow-up stays in the chat it was typed in")
        XCTAssertEqual(model.transcript.items.count, 4)
        XCTAssertTrue(model.isSelectedPinned, "sending leaves the pin alone")

        model.togglePinSelected()
        XCTAssertFalse(model.isSelectedPinned)
        XCTAssertEqual(model.selectedSession, id)
        model.stop()
    }

    /// A chat shown before the sidebar lists it is pinned all the same; ⌘P
    /// used to do nothing for it.
    func testPinWorksForAChatNotListedYet() async throws {
        let model = try await started()
        let quick = try XCTUnwrap(model.sessions.first { $0.id == "quick" })
        await model.select(quick)
        model.sessions = []
        model.togglePinSelected()
        XCTAssertTrue(model.isSelectedPinned)
        XCTAssertEqual(model.selectedSession, "quick")
        XCTAssertEqual(model.toasts.last?.title, "Pinned")
        model.stop()
    }

    /// #2211: a chat started in another folder is created and listed there,
    /// opens there again, and leaves the default project and the next new
    /// chat alone.
    func testChatInAnotherFolderKeepsTheDefault() async throws {
        let other = directory.appendingPathComponent("other", isDirectory: true)
        try FileManager.default.createDirectory(at: other, withIntermediateDirectories: true)
        let model = try await started()
        let defaultProject = model.workspace

        await model.newSession(in: other)
        XCTAssertEqual(model.draftProject, other)
        XCTAssertEqual(model.chatProject, other)
        await send("here", in: model)
        let id = try XCTUnwrap(model.selectedSession)
        XCTAssertEqual(model.chatProject.path, other.path)
        XCTAssertEqual(model.workspace, defaultProject, "the default is unchanged")
        XCTAssertEqual(UserDefaults.standard.string(forKey: "workspacePath"), defaultProject.path)
        try await until { model.sessions.first { $0.id == id }?.cwd == other.path }

        await model.newSession()
        XCTAssertNil(model.draftProject)
        XCTAssertEqual(model.chatProject, defaultProject, "the next new chat starts in the default")

        // Picking the default for a draft is no override.
        await model.newSession(in: defaultProject)
        XCTAssertNil(model.draftProject)

        // Back to it, it still works in its own folder.
        let chat = try XCTUnwrap(model.sessions.first { $0.id == id })
        await model.select(chat)
        XCTAssertEqual(model.chatProject.path, other.path)
        XCTAssertEqual(model.transcript.items.count, 2)

        // A stored chat resumes in the folder the engine recorded for it,
        // which refuses any other.
        let elsewhere = try XCTUnwrap(model.sessions.first { $0.id == "elsewhere" })
        await model.select(elsewhere)
        XCTAssertNil(model.error)
        XCTAssertEqual(model.chatProject.path, "/tmp/rustcode-elsewhere")
        XCTAssertEqual(model.transcript.items.count, 2)
        XCTAssertEqual(model.workspace, defaultProject)
        model.stop()
    }

    /// #2556 and #2557: a subagent is messaged from its composer, the parent's
    /// row goes straight back to the chat, and the working agent stays listed
    /// under its chat after another chat is opened, until it ends and the
    /// chat is opened again from the agent's row.
    func testSubagentStaysFindableAcrossChatsAndTakesMessages() async throws {
        let model = try await started()
        let slow = try XCTUnwrap(model.sessions.first { $0.id == "slow" })
        let quick = try XCTUnwrap(model.sessions.first { $0.id == "quick" })
        await model.select(slow)
        try await until { model.subagents.first?.isActive == true }
        let agent = try XCTUnwrap(model.subagents.first)

        model.openAgent(agent)
        XCTAssertEqual(model.viewedAgent?.id, 1)
        let key = model.agentDraftKey(1)
        model.agentDrafts[key] = "half typed"
        // The parent's row closes the agent and keeps the chat.
        await model.selectFromSidebar(slow)
        XCTAssertNil(model.viewedAgent)
        XCTAssertEqual(model.selectedSession, "slow")
        model.openAgent(agent)
        XCTAssertEqual(model.agentDrafts[key], "half typed", "the draft waits with its agent")
        model.agentDrafts[key] = nil
        await model.sendToViewedAgent("then finish")
        XCTAssertNil(model.error)

        await model.selectFromSidebar(quick)
        XCTAssertEqual(model.selectedSession, "quick")
        XCTAssertNil(model.viewedAgent)
        XCTAssertEqual(model.backgroundAgents.agents(of: "slow").map(\.id), [1])
        // It ends off screen: its row stays and says so.
        try await until { model.backgroundAgents.agents(of: "slow").first?.status == .completed }

        await model.openAgent(model.backgroundAgents.agents(of: "slow")[0], in: slow)
        XCTAssertEqual(model.selectedSession, "slow")
        XCTAssertEqual(model.viewedAgent?.id, 1)
        XCTAssertTrue(model.backgroundAgents.agents(of: "slow").isEmpty)
        model.stop()
    }

    /// #2596: an agent that ends while it is read gives the screen back to
    /// its chat; one ending while another agent is read changes nothing.
    /// The chat's row and the agent's are never both selected, and an agent
    /// that ended stays under its chat to open, also after the chat was left
    /// and opened again.
    func testAnAgentEndingWhileReadReturnsToItsChat() async throws {
        let model = try await started()
        let pair = try XCTUnwrap(model.sessions.first { $0.id == "pair" })
        let quick = try XCTUnwrap(model.sessions.first { $0.id == "quick" })
        await model.select(pair)
        try await until { model.subagents.filter(\.isActive).count == 2 }
        func chatSelected() -> Bool {
            SubagentInfo.chatRowSelected("pair", selected: model.selectedSession, viewingAgent: model.viewedAgent?.id)
        }
        XCTAssertTrue(chatSelected())
        let first = try XCTUnwrap(model.subagents.first { $0.id == 1 })
        let second = try XCTUnwrap(model.subagents.first { $0.id == 2 })

        model.openAgent(first)
        XCTAssertFalse(chatSelected(), "the agent's row is the selected one")
        XCTAssertTrue(model.replacesChat)
        // The first is told to finish, and the reader moves on to the second
        // before it does: its end does not take the screen.
        await model.sendToViewedAgent("finish")
        model.openAgent(second)
        try await until { model.subagents.first { $0.id == 1 }?.status == .completed }
        try await Task.sleep(for: .milliseconds(1200))
        XCTAssertEqual(model.viewedAgent?.id, 2)

        // The second ends while it is read: back to the chat.
        await model.sendToViewedAgent("finish")
        try await until { model.viewedAgent == nil }
        XCTAssertEqual(model.selectedSession, "pair")
        XCTAssertTrue(chatSelected())
        XCTAssertFalse(model.replacesChat)
        // #2631: ended agents leave the sidebar; their results stay in the
        // chat's agents list.
        XCTAssertEqual(model.sidebarChildren(of: "pair").map(\.id), [], "ended agents do not look like work")
        XCTAssertEqual(model.subagents.map(\.id), [1, 2])

        // Left and opened again: still listed, and opened to read, one stays open.
        await model.selectFromSidebar(quick)
        await model.selectFromSidebar(pair)
        try await until { model.subagents.map(\.id) == [1, 2] }
        XCTAssertEqual(model.sidebarChildren(of: "pair").map(\.id), [])
        model.openAgent(try XCTUnwrap(model.subagents.first { $0.id == 1 }))
        try await Task.sleep(for: .milliseconds(1200))
        XCTAssertEqual(model.viewedAgent?.id, 1)
        XCTAssertEqual(model.viewedAgent?.info.status, .completed)
        model.stop()
    }

    /// #2553: Settings is a page over the chat. Opening it keeps the chat,
    /// its draft and its open agent state under it; a deep link opens its
    /// section, and Settings reopens where it was left.
    func testSettingsPageReturnsToTheChatItCovered() async throws {
        let model = try await started()
        let quick = try XCTUnwrap(model.sessions.first { $0.id == "quick" })
        await model.select(quick)
        model.draft = "half written"
        model.toggleSwitcher()
        XCTAssertTrue(model.showsSwitcher)

        model.openSettings()
        XCTAssertTrue(model.showsSettings)
        XCTAssertFalse(model.showsSwitcher, "the switcher gives way to Settings")
        XCTAssertEqual(model.settingsReturnSession, "quick", "Back's destination is the chat Settings covered")
        XCTAssertEqual(model.settingsBackTitle, quick.title)
        XCTAssertTrue(model.coversChat)
        model.settingsSection = .keybinds
        model.closeSettings()
        XCTAssertFalse(model.showsSettings)
        XCTAssertEqual(model.selectedSession, "quick")
        XCTAssertEqual(model.draft, "half written")
        XCTAssertEqual(model.transcript.items.count, Self.replayed)

        // ⌘, again: General, not the section last used. Check for Updates: About.
        model.openSettings()
        XCTAssertEqual(model.settingsSection, .general)
        model.settingsSection = .keybinds
        model.openSettings()
        XCTAssertEqual(model.settingsSection, .keybinds, "⌘, while open stays put")
        model.openSettings(.about)
        XCTAssertEqual(model.settingsSection, .about)
        model.closeSettings()
        XCTAssertEqual(model.draft, "half written")

        // A deep link while Settings is open moves to its section and keeps
        // Back's destination; Back returns there even if the selection moved.
        model.openSettings(.keybinds)
        model.openSettings(.about)
        XCTAssertEqual(model.settingsSection, .about)
        XCTAssertEqual(model.settingsReturnSession, "quick")
        let slow = try XCTUnwrap(model.sessions.first { $0.id == "slow" })
        await model.select(slow)
        XCTAssertEqual(model.settingsBackTitle, quick.title)
        model.closeSettings()
        XCTAssertFalse(model.showsSettings)
        try await until { model.selectedSession == "quick" }
        model.stop()
    }

    /// An ACP engine with two stored chats of `replayed` messages, "slow" (its
    /// load takes a moment) and "quick", that answers each prompt with one
    /// short reply.
    private static let replayed = 60

    private static let fakeEngine = #"""
    #!/usr/bin/env python3
    import json, sys, threading, time
    if sys.argv[1:] != ["--acp"]:
        sys.exit(1)
    lock = threading.Lock()
    def send(message):
        with lock:
            sys.stdout.write(json.dumps(message) + "\n")
            sys.stdout.flush()
    def update(session, kind, text):
        send({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": session,
              "update": {"sessionUpdate": kind, "content": {"type": "text", "text": text}}}})
    # Long replays: their updates queue up ahead of the load's response.
    history = [m for i in range(30) for m in ("question %d" % i, "answer %d" % i)]
    stored = {"slow": list(history), "quick": list(history), "elsewhere": ["question", "answer"],
              "pair": ["question", "answer"]}
    # The folder each chat was created in, as the engine records it.
    cwds = {"elsewhere": "/tmp/rustcode-elsewhere"}
    created = 0
    steers = {}
    # Subagents of "slow" and "pair", each working until a message saying
    # "finish" ends it.
    agents = {"slow": [{"id": 1, "name": "agent-1", "task": "dig", "status": "running", "messageCount": 1}],
              "pair": [{"id": i, "name": "agent-%d" % i, "task": "dig", "status": "running", "messageCount": 1}
                       for i in (1, 2)]}
    agent_messages = []
    def finish_agent(session, agent):
        time.sleep(0.5)
        for listed in agents[session]:
            if listed["id"] == agent:
                listed["status"] = "completed"
                listed["completion"] = "dug"
    # Each chat's model settings: "plan" offers effort and Fast, "local" neither.
    config = {}
    tuned = set()
    def options(sid):
        c = config.setdefault(sid, {"model": "plan", "effort": "default", "fast": False})
        opts = [{"id": "model", "type": "select", "currentValue": c["model"],
                 "options": [{"value": "plan", "name": "plan"}, {"value": "local", "name": "local"}]}]
        if c["model"] == "plan":
            opts.append({"id": "effort", "type": "select", "currentValue": c["effort"],
                         "options": [{"value": v, "name": v} for v in ("default", "low", "high")]})
            opts.append({"id": "fast", "type": "boolean", "currentValue": c["fast"]})
        return opts
    def handle(message):
        global created
        method, params, rid = message.get("method"), message.get("params", {}), message.get("id")
        result = {}
        if method == "initialize":
            result = {"protocolVersion": 1, "agentCapabilities": {"loadSession": True,
                      "sessionCapabilities": {"list": {}, "close": {}}, "_meta": {"rustcode": {"steer": 1, "agents": 1}}}}
        elif method == "_rustcode/agents":
            listed = agents.get(params["sessionId"], [])
            if params["action"] == "list":
                result = {"agents": listed}
            elif params["action"] == "history":
                result = {"updates": [{"sessionUpdate": "user_message_chunk", "content": {"type": "text", "text": "dig"}}]}
            elif params["action"] == "send":
                agent_messages.append(params["text"])
                if "finish" in params["text"]:
                    threading.Thread(target=finish_agent, args=(params["sessionId"], params["id"]), daemon=True).start()
                result = {"queued": False}
        elif method == "_rustcode/steer":
            steers.setdefault(params["sessionId"], []).append(params["text"])
            result = {"accepted": True}
        elif method == "session/new":
            created += 1
            sid = "new-%d" % created
            stored[sid] = []
            cwds[sid] = params["cwd"]
            result = {"sessionId": sid, "configOptions": options(sid)}
        elif method == "session/set_config_option":
            sid, key, value = params["sessionId"], params["configId"], params["value"]
            # A slow reply, so a chat can be switched before it lands.
            if value == "high":
                time.sleep(0.6)
            c = config.setdefault(sid, {"model": "plan", "effort": "default", "fast": False})
            c[key] = value
            tuned.add(sid)
            if key == "model" and value == "local":
                c["effort"], c["fast"] = "default", False
            result = {"configOptions": options(sid)}
        elif method == "session/list":
            result = {"sessions": [{"sessionId": sid, "cwd": cwds.get(sid, params.get("cwd", "/tmp")), "title": sid,
                                    "updatedAt": "2026-10-20T08:00:00Z"} for sid, messages in stored.items() if messages]}
        elif method == "session/load":
            sid = params["sessionId"]
            if sid in cwds and cwds[sid] != params["cwd"]:
                send({"jsonrpc": "2.0", "id": rid, "error": {"code": -32602,
                      "message": "session/load cwd does not match the recorded workspace"}})
                return
            if sid == "slow":
                time.sleep(0.4)
            for index, text in enumerate(stored[sid]):
                update(sid, "user_message_chunk" if index % 2 == 0 else "agent_message_chunk", text)
            result = {"configOptions": options(sid)}
        elif method == "session/prompt":
            sid = params["sessionId"]
            text = params["prompt"][0]["text"]
            time.sleep(0.8 if text.startswith("long") else 0.15)
            # The settings the turn ran with, to check a draft's were applied first.
            c = config.get(sid)
            settings = " [%s %s %s]" % (c["model"], c["effort"], "fast" if c["fast"] else "standard") if sid in tuned else ""
            update(sid, "agent_message_chunk", "reply to " + text + settings)
            stored[sid] += [text, "reply to " + text]
            result = {"stopReason": "end_turn"}
            # Steers the turn "read" are consumed; one marked unread comes back.
            unused = [steer for steer in steers.pop(sid, []) if "unread" in steer]
            if unused:
                result["_meta"] = {"rustcode": {"unusedSteers": unused}}
        if rid is not None:
            send({"jsonrpc": "2.0", "id": rid, "result": result})
    for line in sys.stdin:
        message = json.loads(line)
        if "method" in message:
            threading.Thread(target=handle, args=(message,), daemon=True).start()
    """#
}
