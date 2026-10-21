import AppKit
import XCTest
import RustCodeKit
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
    stored = {"slow": list(history), "quick": list(history), "elsewhere": ["question", "answer"]}
    # The folder each chat was created in, as the engine records it.
    cwds = {"elsewhere": "/tmp/rustcode-elsewhere"}
    created = 0
    def handle(message):
        global created
        method, params, rid = message.get("method"), message.get("params", {}), message.get("id")
        result = {}
        if method == "initialize":
            result = {"protocolVersion": 1, "agentCapabilities": {"loadSession": True,
                      "sessionCapabilities": {"list": {}, "close": {}}}}
        elif method == "session/new":
            created += 1
            sid = "new-%d" % created
            stored[sid] = []
            cwds[sid] = params["cwd"]
            result = {"sessionId": sid}
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
        elif method == "session/prompt":
            sid = params["sessionId"]
            text = params["prompt"][0]["text"]
            time.sleep(0.15)
            update(sid, "agent_message_chunk", "reply to " + text)
            stored[sid] += [text, "reply to " + text]
            result = {"stopReason": "end_turn"}
        if rid is not None:
            send({"jsonrpc": "2.0", "id": rid, "result": result})
    for line in sys.stdin:
        message = json.loads(line)
        if "method" in message:
            threading.Thread(target=handle, args=(message,), daemon=True).start()
    """#
}
