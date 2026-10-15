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
    }

    var body: some Scene {
        WindowGroup {
            ContentView(model: model)
                .frame(minWidth: 850, minHeight: 620)
                .task { await model.start() }
                .onDisappear { model.stop() }
        }
        .commands {
            CommandGroup(replacing: .newItem) {
                Button("New Chat") { Task { await model.newSession() } }
                    .keyboardShortcut("n", modifiers: .command)
            }
            CommandGroup(after: .appInfo) {
                Button("Choose Project…") { model.chooseWorkspace() }
                    .keyboardShortcut("o", modifiers: .command)
            }
        }
    }
}

@MainActor
final class DesktopModel: ObservableObject {
    struct Message: Identifiable {
        let id = UUID()
        var role: String
        var text: String
    }

    @Published var messages: [Message] = []
    @Published var sessions: [SessionSummary] = []
    @Published var selectedSession: String?
    @Published var workspace: URL {
        didSet { UserDefaults.standard.set(workspace.path, forKey: "workspacePath") }
    }
    @Published var draft = ""
    @Published var busy = false
    @Published var error: String?
    /// Every unanswered permission or question request, oldest first. The
    /// engine blocks until each one is answered, so none may be dropped.
    @Published var permissionQueue: [PermissionRequest] = []
    @Published var models = ModelOptions.empty
    @Published var selectedModel: String?
    private var engine: EngineProcess?
    /// The session whose prompt turn is running; Stop cancels this one even
    /// after the user switches to another session.
    private var turnSession: String?
    private var inboundTask: Task<Void, Never>?
    private var capabilities: AgentInfo?

    init() {
        let savedPath = UserDefaults.standard.string(forKey: "workspacePath")
        workspace = savedPath.map { URL(fileURLWithPath: $0, isDirectory: true) }
            ?? URL(fileURLWithPath: FileManager.default.currentDirectoryPath, isDirectory: true)
    }

    func start() async {
        guard engine == nil else { return }
        do {
            let location = try EngineLocator(userSetting: UserDefaults.standard.string(forKey: "enginePath")).locate()
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

    private func restartForWorkspace() async {
        stop()
        messages = []
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
            messages = []
            applyConfig(result["configOptions"])
            await refreshSessions()
        } catch { self.error = String(describing: error) }
    }

    func refreshSessions() async {
        guard let connection = engine?.connection, capabilities?.listSessions == true else { return }
        do {
            let result = try await connection.request(ACPMethod.sessionList, params: ACPRequests.listSessions(cwd: workspace), timeout: .seconds(15))
            sessions = SessionSummary.list(from: result)
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
        messages = []
        do {
            let result = try await connection.request(ACPMethod.sessionLoad, params: ACPRequests.loadSession(id: session.id, cwd: URL(fileURLWithPath: session.cwd)), timeout: .seconds(30))
            // Another session may have been picked while this one loaded.
            guard selectedSession == session.id else { return }
            applyConfig(result["configOptions"])
        } catch { self.error = String(describing: error) }
    }

    func submit() async {
        guard let connection = engine?.connection, let id = selectedSession else { return }
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, !busy else { return }
        draft = ""
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
        case .notification(let method, let params) where method == ACPMethod.sessionUpdate:
            guard params["sessionId"]?.stringValue == selectedSession else { return }
            switch SessionUpdate(notificationParams: params) {
            case .userText(let text): messages.append(Message(role: "You", text: text))
            case .agentText(let text): appendAssistant(text)
            case .thoughtText(let text): appendAssistant("Thinking: \(text)")
            case .toolCall(_, let title, _, let status, let input):
                let detail = input.map { "\n\($0.prettyPrinted())" } ?? ""
                messages.append(Message(role: status == .inProgress ? "Running" : "Tool", text: title + detail))
            case .toolCallUpdate(_, let title, let status, let output):
                if let output { messages.append(Message(role: status == .failed ? "Tool failed" : "Result", text: (title.map { $0 + "\n" } ?? "") + output)) }
            case .title: await refreshSessions()
            case .configOptions(let options): applyConfig(options)
            case .unknown: break
            }
        case .request(let id, let method, let params) where method == ACPMethod.requestPermission:
            if let request = PermissionRequest(rpcId: id, params: params) { permissionQueue.append(request) }
            else { try? await engine?.connection.respond(id: id, error: .invalidParams(method)) }
        case .request(let id, let method, _):
            try? await engine?.connection.respond(id: id, error: .methodNotFound(method))
        default: break
        }
    }

    private func appendAssistant(_ text: String) {
        if let last = messages.last, last.role == "RustCode" { messages[messages.count - 1].text += text }
        else { messages.append(Message(role: "RustCode", text: text)) }
    }
}

struct ContentView: View {
    @ObservedObject var model: DesktopModel

    var body: some View {
        NavigationSplitView {
            VStack(spacing: 0) {
                HStack {
                    Text("RustCode").font(.headline)
                    Spacer()
                    Button { model.chooseWorkspace() } label: { Image(systemName: "folder") }
                        .help("Choose project folder")
                }.padding()
                Text(model.workspace.path).font(.caption).lineLimit(1).truncationMode(.middle).padding(.horizontal)
                Button { Task { await model.newSession() } } label: {
                    Label("New Chat", systemImage: "plus").frame(maxWidth: .infinity)
                }.buttonStyle(.borderedProminent).padding()
                List(model.sessions) { session in
                    Button { Task { await model.select(session) } } label: {
                        VStack(alignment: .leading) {
                            HStack {
                                Text(session.title ?? URL(fileURLWithPath: session.cwd).lastPathComponent)
                                if model.permissionQueue.contains(where: { $0.sessionId == session.id }) {
                                    Image(systemName: "questionmark.circle.fill").foregroundStyle(.orange)
                                        .help("Waiting for your answer")
                                }
                            }
                            Text(session.updatedAt?.formatted() ?? session.id.prefix(8).description)
                                .font(.caption).foregroundStyle(.secondary)
                        }
                    }.buttonStyle(.plain)
                }
            }
            .navigationSplitViewColumnWidth(min: 220, ideal: 260)
        } detail: {
            VStack(spacing: 0) {
                HStack {
                    Text("\(modelsTitle)").font(.headline)
                    Spacer()
                    if !model.models.choices.isEmpty {
                        Picker("Model", selection: Binding(get: { model.selectedModel ?? "" }, set: { value in Task { await model.setModel(value) } })) {
                            ForEach(model.models.choices) { Text($0.name).tag($0.id) }
                        }.frame(width: 210)
                    }
                }.padding()
                if let error = model.error {
                    HStack { Image(systemName: "exclamationmark.triangle"); Text(error); Spacer(); Button("Dismiss") { model.error = nil } }
                        .padding().background(.red.opacity(0.1))
                }
                ScrollViewReader { proxy in
                    ScrollView {
                        LazyVStack(alignment: .leading, spacing: 14) {
                            ForEach(model.messages) { message in
                                VStack(alignment: .leading, spacing: 5) {
                                    Text(message.role).font(.caption.bold()).foregroundStyle(message.role == "RustCode" ? .blue : .secondary)
                                    Text(message.text).textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading)
                                }.padding(12).background(.quaternary.opacity(0.35), in: RoundedRectangle(cornerRadius: 10)).id(message.id)
                            }
                        }.padding()
                    }.onChange(of: model.messages.count) { _, _ in
                        if let last = model.messages.last { proxy.scrollTo(last.id, anchor: .bottom) }
                    }
                }
                if let permission = model.pendingPermission {
                    if permission.question != nil {
                        QuestionCard(model: model, request: permission).id(permission.id)
                    } else {
                        permissionCard(permission)
                    }
                }
                HStack(alignment: .bottom) {
                    TextField("Message RustCode…", text: $model.draft, axis: .vertical)
                        .lineLimit(1...6).textFieldStyle(.roundedBorder)
                        .onSubmit { Task { await model.submit() } }
                        .disabled(model.busy)
                    if model.busy {
                        Button { Task { await model.cancel() } } label: { Image(systemName: "stop.fill") }
                            .buttonStyle(.bordered).help("Stop turn")
                    } else {
                        Button { Task { await model.submit() } } label: { Image(systemName: "arrow.up") }
                            .buttonStyle(.borderedProminent).disabled(model.draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                            .keyboardShortcut(.return, modifiers: .command)
                    }
                }.padding()
            }
        }
        .alert("RustCode", isPresented: Binding(get: { model.error != nil && model.error!.isEmpty }, set: { if !$0 { model.error = nil } })) { Button("OK") { model.error = nil } }
    }

    private var modelsTitle: String { model.selectedSession == nil ? "Choose a project" : "RustCode" }

    @ViewBuilder private func permissionCard(_ request: PermissionRequest) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            Label("Permission requested", systemImage: "hand.raised").font(.headline)
            Text(request.title).textSelection(.enabled)
            if let input = request.input { Text(input.prettyPrinted()).font(.system(.caption, design: .monospaced)).textSelection(.enabled).lineLimit(8) }
            HStack {
                ForEach(request.options) { option in
                    if option.kind?.isAllow == true {
                        Button(option.name) { Task { await model.answer(request, with: option) } }
                            .buttonStyle(.borderedProminent)
                    } else {
                        Button(option.name) { Task { await model.answer(request, with: option) } }
                            .buttonStyle(.bordered)
                    }
                }
            }
        }.padding().frame(maxWidth: .infinity, alignment: .leading).background(.orange.opacity(0.12))
    }
}

/// An `ask_question` from the engine: one button per option, ticks and a
/// Submit for multi-select questions, and a free-text answer.
struct QuestionCard: View {
    @ObservedObject var model: DesktopModel
    let request: PermissionRequest
    @State private var ticked: Set<String> = []
    @State private var custom = ""

    var body: some View {
        let question = request.question!
        VStack(alignment: .leading, spacing: 10) {
            Label(question.header.isEmpty ? "RustCode has a question" : question.header, systemImage: "questionmark.circle")
                .font(.headline)
            Text(question.text).textSelection(.enabled)
            if question.multiple {
                ForEach(request.answerOptions) { option in
                    Toggle(option.name, isOn: Binding(
                        get: { ticked.contains(option.id) },
                        set: { if $0 { ticked.insert(option.id) } else { ticked.remove(option.id) } }))
                }
            }
            HStack {
                if question.multiple {
                    Button("Submit") {
                        let chosen = request.answerOptions.filter { ticked.contains($0.id) }
                        Task { await model.answer(request, choosing: chosen) }
                    }.buttonStyle(.borderedProminent).disabled(ticked.isEmpty)
                } else {
                    ForEach(request.answerOptions) { option in
                        Button(option.name) { Task { await model.answer(request, with: option) } }
                            .buttonStyle(.borderedProminent)
                    }
                }
                if let reject = request.rejectOption {
                    Button(reject.name) { Task { await model.answer(request, with: reject) } }
                        .buttonStyle(.bordered)
                }
            }
            HStack {
                TextField("Or write your own answer…", text: $custom)
                    .textFieldStyle(.roundedBorder)
                    .onSubmit { Task { await model.answer(request, text: custom) } }
                Button("Send") { Task { await model.answer(request, text: custom) } }
                    .disabled(custom.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }
        }.padding().frame(maxWidth: .infinity, alignment: .leading).background(.orange.opacity(0.12))
    }
}
