import AppKit
import RustCodeKit
import SwiftUI

/// The window has no title bar: a gray sidebar runs to the top edge beside the
/// chat, with no split-view divider between them.
struct ContentView: View {
    @ObservedObject var model: DesktopModel

    var body: some View {
        HStack(spacing: 0) {
            if model.showsSidebar {
                SessionSidebar(model: model, updater: model.updater)
                    .frame(width: 270)
                    .transition(.move(edge: .leading))
            }
            ChatView(model: model)
                .background(Color.chatBackground)
        }
        .background(Color.sidebarBackground)
        .ignoresSafeArea()
        .tint(.brand)
        .background(WindowChrome())
        .sheet(isPresented: $model.showsSettings) { SettingsView(model: model) }
    }
}

/// Lets the window be dragged from any empty area, as it has no title bar.
private struct WindowChrome: NSViewRepresentable {
    func makeNSView(context: Context) -> NSView {
        let view = NSView()
        DispatchQueue.main.async { view.window?.isMovableByWindowBackground = true }
        return view
    }

    func updateNSView(_ view: NSView, context: Context) {}
}

/// A borderless icon button for the window's top bar.
struct ChromeButton: View {
    let systemImage: String
    let help: String
    let action: () -> Void
    @State private var isHovered = false

    var body: some View {
        Button(action: action) {
            Image(systemName: systemImage)
                .font(.system(size: 15))
                .foregroundStyle(.secondary)
                .frame(width: 30, height: 28)
                .background(isHovered ? AnyShapeStyle(.fill.tertiary) : AnyShapeStyle(.clear), in: .rect(cornerRadius: 7))
                .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .onHover { isHovered = $0 }
        .help(help)
    }
}

private struct SessionSidebar: View {
    @ObservedObject var model: DesktopModel
    @ObservedObject var updater: Updater

    /// The hidden title bar's height, so buttons line up with the traffic lights.
    static let barHeight: CGFloat = 28

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            // Room for the traffic lights, then the bar's own buttons.
            HStack(spacing: 2) {
                Spacer()
                ChromeButton(systemImage: "sidebar.left", help: "Hide sidebar (⌃⌘S)") { model.toggleSidebar() }
                ChromeButton(systemImage: "square.and.pencil", help: "New chat (⌘N)") { Task { await model.newSession() } }
            }
            .padding(.leading, 80)
            .padding(.trailing, 10)
            .frame(height: Self.barHeight)
            .padding(.top, 1)

            HStack(spacing: 6) {
                Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
                TextField("Search chats", text: $model.sessionFilter)
                    .textFieldStyle(.plain)
                    .onKeyPress(.escape) {
                        guard !model.sessionFilter.isEmpty else { return .ignored }
                        model.sessionFilter = ""
                        return .handled
                    }
                if !model.sessionFilter.isEmpty {
                    Button { model.sessionFilter = "" } label: {
                        Image(systemName: "xmark.circle.fill").foregroundStyle(.tertiary)
                    }
                    .buttonStyle(.plain)
                }
            }
            .font(.callout)
            .padding(.horizontal, 10)
            .frame(height: 30)
            .background(Color.primary.opacity(0.06), in: .rect(cornerRadius: 10))
            .padding(.horizontal, 12)
            .padding(.top, 8)

            Text("Chats")
                .font(.caption.weight(.semibold))
                .foregroundStyle(.secondary)
                .padding(.horizontal, 20)
                .padding(.top, 12)
                .padding(.bottom, 4)

            ScrollView {
                LazyVStack(spacing: 2) {
                    ForEach(model.visibleSessions) { session in
                        SessionRow(session: session,
                                   isSelected: session.id == model.selectedSession,
                                   needsAnswer: model.permissionQueue.contains { $0.sessionId == session.id },
                                   isRunning: model.busy && model.turnSession == session.id) {
                            guard session.id != model.selectedSession else { return }
                            Task { await model.select(session) }
                        }
                    }
                }
                .padding(.horizontal, 10)
            }
            .overlay {
                if model.visibleSessions.isEmpty {
                    Text(model.sessions.isEmpty ? "No chats in this project yet." : "No matching chats.")
                        .font(.callout)
                        .foregroundStyle(.secondary)
                }
            }

            if let release = updater.available {
                Button { Task { await updater.install() } } label: {
                    HStack(spacing: 8) {
                        Image(systemName: "arrow.down.circle.fill").foregroundStyle(Color.brand)
                        Text(updater.canInstallInPlace ? "Update to \(release.version)" : "Download \(release.version)")
                            .font(.callout.weight(.medium))
                        Spacer(minLength: 0)
                        if case .downloading(_, let progress) = updater.state {
                            ProgressView(value: progress).frame(width: 50)
                        } else if case .installing = updater.state {
                            ProgressView().controlSize(.small)
                        }
                    }
                    .padding(.horizontal, 12)
                    .frame(height: 34)
                    .background(Color.brand.opacity(0.14), in: .capsule)
                    .contentShape(.capsule)
                }
                .buttonStyle(.plain)
                .padding(.horizontal, 12)
                .padding(.top, 8)
                .help("Install the new version and restart RustCode")
            }
            HStack(spacing: 4) {
                Button { model.chooseWorkspace() } label: {
                    HStack(spacing: 8) {
                        Image(systemName: "folder.fill").foregroundStyle(Color.brand)
                        VStack(alignment: .leading, spacing: 1) {
                            Text(model.workspace.lastPathComponent).font(.callout.weight(.medium)).lineLimit(1)
                            Text(model.workspace.deletingLastPathComponent().path.abbreviatingHome)
                                .font(.caption).foregroundStyle(.secondary)
                                .lineLimit(1).truncationMode(.middle)
                        }
                        Spacer(minLength: 0)
                    }
                    .padding(.horizontal, 10)
                    .padding(.vertical, 8)
                    .contentShape(.rect)
                }
                .buttonStyle(.plain)
                .help("Choose project folder (⌘O)")
                ChromeButton(systemImage: "gearshape", help: "Settings (⌘,)") { model.showsSettings = true }
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 10)
        }
    }
}

private struct SessionRow: View {
    let session: SessionSummary
    let isSelected: Bool
    let needsAnswer: Bool
    let isRunning: Bool
    let action: () -> Void
    @State private var isHovered = false

    var body: some View {
        Button(action: action) {
            HStack(spacing: 8) {
                VStack(alignment: .leading, spacing: 2) {
                    Text(session.title ?? "New chat")
                        .lineLimit(1)
                    if let updated = session.updatedAt {
                        Text(updated, format: .relative(presentation: .named))
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }
                Spacer(minLength: 4)
                if needsAnswer {
                    Image(systemName: "questionmark.bubble.fill")
                        .foregroundStyle(Color.attention)
                        .help("Waiting for your answer")
                } else if isRunning {
                    ProgressView().controlSize(.small)
                }
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 7)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(rowFill, in: .rect(cornerRadius: 9))
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .onHover { isHovered = $0 }
    }

    private var rowFill: Color {
        isSelected ? Color.primary.opacity(0.09) : isHovered ? Color.primary.opacity(0.04) : .clear
    }
}

private struct ChatView: View {
    @ObservedObject var model: DesktopModel
    @State private var composerHeight: CGFloat = 60

    private var title: String {
        model.sessions.first { $0.id == model.selectedSession }?.title ?? "RustCode"
    }

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 18) {
                    ForEach(model.transcript.items) { item in
                        TranscriptRow(item: item, isCurrentMatch: model.showsFind && item.id == model.currentMatch).id(item.id)
                    }
                    if model.busy, model.turnSession == model.selectedSession, !isAgentWriting {
                        WorkingIndicator()
                    }
                    Color.clear.frame(height: 1).id(bottomAnchor)
                }
                .frame(maxWidth: 760)
                .padding(.horizontal, 28)
                .padding(.vertical, 20)
                .frame(maxWidth: .infinity)
            }
            .overlay {
                if model.transcript.isEmpty, !model.busy { EmptyChat(workspace: model.workspace) }
            }
            .onChange(of: model.transcript) { _, _ in
                if !model.showsFind { proxy.scrollTo(bottomAnchor, anchor: .bottom) }
            }
            .onChange(of: model.currentMatch) { _, match in
                guard model.showsFind, let match else { return }
                withAnimation(.snappy(duration: 0.25)) { proxy.scrollTo(match, anchor: .center) }
            }
        }
        .environment(\.findQuery, model.showsFind ? model.findQuery : "")
        .safeAreaInset(edge: .top, spacing: 0) {
            VStack(spacing: 0) {
                header
                if model.showsFind {
                    FindBar(model: model)
                        .transition(.move(edge: .top).combined(with: .opacity))
                }
            }
            .animation(.snappy(duration: 0.2), value: model.showsFind)
        }
        .safeAreaInset(edge: .bottom, spacing: 0) {
            VStack(spacing: 10) {
                if let error = model.error, !error.isEmpty { ErrorBanner(text: error) { model.error = nil } }
                if let permission = model.pendingPermission {
                    if permission.question != nil {
                        QuestionCard(model: model, request: permission).id(permission.id)
                    } else {
                        PermissionCard(model: model, request: permission).id(permission.id)
                    }
                }
                Composer(model: model)
                    .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { composerHeight = $0 }
            }
            .frame(maxWidth: 760)
            .padding(.horizontal, 28)
            .padding(.bottom, 16)
            .frame(maxWidth: .infinity)
        }
        .overlay { composerPanel }
        .animation(.snappy(duration: 0.18), value: model.composerPanel)
    }

    /// The + or model panel, drawn over the whole chat so nothing clips it, with
    /// its bottom just above the composer. A click outside or Esc closes it.
    @ViewBuilder
    private var composerPanel: some View {
        if let panel = model.composerPanel {
            ZStack(alignment: .bottom) {
                Color.black.opacity(0.001)
                    .onTapGesture { model.composerPanel = nil }
                HStack {
                    if panel == .model { Spacer(minLength: 0) }
                    Group {
                        switch panel {
                        case .add: AddPanel(model: model)
                        case .model: ModelListPanel(model: model) { model.composerPanel = nil }
                        }
                    }
                    .padding(6)
                    .frame(width: panel == .model ? 340 : 260, alignment: .leading)
                    .background {
                        RoundedRectangle(cornerRadius: 20)
                            .fill(.regularMaterial)
                            .shadow(color: .black.opacity(0.28), radius: 20, y: 8)
                    }
                    .clipShape(.rect(cornerRadius: 20))
                    if panel == .add { Spacer(minLength: 0) }
                }
                .frame(maxWidth: 760)
                .padding(.horizontal, 28)
                .padding(.bottom, composerHeight + 16 + 8)
                .frame(maxWidth: .infinity)
            }
            .transition(.opacity.combined(with: .offset(y: 6)))
            .background { Button("") { model.composerPanel = nil }.keyboardShortcut(.cancelAction).hidden() }
        }
    }

    /// The chat's title bar: the title and project, and the sidebar buttons
    /// beside the traffic lights while the sidebar is hidden.
    private var header: some View {
        HStack(spacing: 10) {
            if !model.showsSidebar {
                HStack(spacing: 2) {
                    ChromeButton(systemImage: "sidebar.left", help: "Show sidebar (⌃⌘S)") { model.toggleSidebar() }
                    ChromeButton(systemImage: "square.and.pencil", help: "New chat (⌘N)") { Task { await model.newSession() } }
                }
                .padding(.leading, 70)
            }
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                Text(title).font(.headline).lineLimit(1)
                Text(model.workspace.lastPathComponent).font(.callout).foregroundStyle(.secondary).lineLimit(1)
            }
            Spacer()
        }
        .padding(.horizontal, 18)
        .frame(height: SessionSidebar.barHeight)
        .padding(.top, 1)
        .padding(.bottom, 8)
        .background(Color.chatBackground.opacity(0.94))
    }

    private let bottomAnchor = "bottom"

    private var isAgentWriting: Bool {
        if case .assistant = model.transcript.items.last?.content { return true }
        return false
    }
}

/// ⌘F: find in the chat. ⏎ and ⌘G go to the next match, ⇧⏎ and ⇧⌘G back, Esc closes.
private struct FindBar: View {
    @ObservedObject var model: DesktopModel
    @FocusState private var focused: Bool

    var body: some View {
        let count = model.findMatches.count
        HStack(spacing: 8) {
            Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
            TextField("Find in chat", text: $model.findQuery)
                .textFieldStyle(.plain)
                .focused($focused)
                .onKeyPress(.return, phases: .down) { press in
                    model.findNext(press.modifiers.contains(.shift) ? -1 : 1)
                    return .handled
                }
                .onKeyPress(.escape) {
                    if model.findQuery.isEmpty { close() } else { model.findQuery = "" }
                    return .handled
                }
            if !model.findQuery.isEmpty {
                Text(count == 0 ? "No matches" : "\(min(model.findIndex, count - 1) + 1) of \(count)")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .monospacedDigit()
            }
            ChromeButton(systemImage: "chevron.up", help: "Previous (⇧⌘G)") { model.findNext(-1) }
                .disabled(count == 0)
            ChromeButton(systemImage: "chevron.down", help: "Next (⌘G)") { model.findNext() }
                .disabled(count == 0)
            Button("Done") { close() }
                .buttonStyle(.plain)
                .font(.callout.weight(.medium))
                .padding(.horizontal, 10)
                .frame(height: 26)
                .background(.fill.tertiary, in: .capsule)
                .keyboardShortcut(.cancelAction)
        }
        .padding(.leading, 12)
        .padding(.trailing, 6)
        .frame(height: 38)
        .floatingSurface(in: .rect(cornerRadius: 14))
        .frame(maxWidth: 560)
        .padding(.horizontal, 28)
        .padding(.bottom, 8)
        .onAppear { focused = true }
        .onChange(of: model.showsFind) { _, shown in if shown { focused = true } }
    }

    private func close() {
        model.showsFind = false
    }
}

private struct EmptyChat: View {
    let workspace: URL

    var body: some View {
        VStack(spacing: 14) {
            Image(nsImage: NSApp.applicationIconImage)
                .resizable()
                .frame(width: 88, height: 88)
            Text("What should we build?")
                .font(.title2.weight(.semibold))
            Label(workspace.path.abbreviatingHome, systemImage: "folder")
                .font(.callout)
                .foregroundStyle(.secondary)
        }
        .padding(.bottom, 80)
        .allowsHitTesting(false)
    }
}

private struct ErrorBanner: View {
    let text: String
    let dismiss: () -> Void

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(Color.attention)
            Text(text).font(.callout).textSelection(.enabled).lineLimit(4)
            Spacer(minLength: 8)
            Button("Dismiss", action: dismiss).buttonStyle(.borderless)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .floatingSurface(in: .rect(cornerRadius: 14))
    }
}

/// The floating message field, laid out like the iOS composer: the text on
/// top, and below it the + menu, the model and Send. ⏎ sends, ⇧⏎ breaks the line.
private struct Composer: View {
    @ObservedObject var model: DesktopModel
    @FocusState private var focused: Bool
    @State private var isDropTarget = false

    private var canSend: Bool {
        !model.busy && (!model.draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || !model.attachments.isEmpty)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            if !model.attachments.isEmpty { attachmentChips }
            TextField(model.busy ? "RustCode is working…" : "Message RustCode…", text: $model.draft, axis: .vertical)
                .textFieldStyle(.plain)
                .font(.body)
                .lineLimit(1...10)
                .focused($focused)
                .padding(.horizontal, 6)
                .padding(.top, 4)
                .onKeyPress(.return, phases: .down) { press in
                    if press.modifiers.contains(.shift) { return .ignored }
                    Task { await model.submit() }
                    return .handled
                }
                .onKeyPress(.escape) {
                    guard model.composerPanel != nil else { return .ignored }
                    model.composerPanel = nil
                    return .handled
                }
            HStack(spacing: 8) {
                addMenu
                Spacer(minLength: 0)
                modelMenu
                trailingAction
            }
        }
        .padding(10)
        .floatingSurface(in: .rect(cornerRadius: 22))
        .overlay {
            if isDropTarget {
                RoundedRectangle(cornerRadius: 22).strokeBorder(Color.brand, style: StrokeStyle(lineWidth: 2, dash: [6, 4]))
            }
        }
        .dropDestination(for: URL.self) { urls, _ in
            let files = urls.filter(\.isFileURL)
            model.attach(files)
            return !files.isEmpty
        } isTargeted: { isDropTarget = $0 }
        .onAppear { focused = true }
    }

    private func toggle(_ next: ComposerPanel) {
        model.composerPanel = model.composerPanel == next ? nil : next
    }

    private var addMenu: some View {
        Button { toggle(.add) } label: {
            Image(systemName: "plus")
                .font(.system(size: 14, weight: .medium))
                .rotationEffect(.degrees(model.composerPanel == .add ? 45 : 0))
                .frame(width: 30, height: 30)
                .background(.fill.tertiary, in: .circle)
                .contentShape(.circle)
        }
        .buttonStyle(.plain)
        .help("Add files, start a chat or switch project")
    }

    @ViewBuilder
    private var modelMenu: some View {
        if !model.models.choices.isEmpty {
            Button { toggle(.model) } label: {
                HStack(spacing: 5) {
                    Text(modelLabel).lineLimit(1)
                    Image(systemName: "chevron.up")
                        .font(.caption2.weight(.semibold))
                        .rotationEffect(.degrees(model.composerPanel == .model ? 180 : 0))
                }
                .font(.callout)
                .foregroundStyle(.secondary)
                .padding(.horizontal, 12)
                .frame(height: 30)
                .background(model.composerPanel == .model ? AnyShapeStyle(.fill.tertiary) : AnyShapeStyle(.clear), in: .capsule)
                .contentShape(.capsule)
            }
            .buttonStyle(.plain)
            .help("Model")
        }
    }

    /// `provider/model` reads as just the model in the composer.
    private var modelLabel: String {
        let current = model.models.choices.first { $0.id == model.selectedModel }
        let name = current?.name ?? model.selectedModel ?? "Model"
        return name.split(separator: "/").last.map(String.init) ?? name
    }

    @ViewBuilder
    private var trailingAction: some View {
        if model.busy {
            Button { Task { await model.cancel() } } label: {
                Image(systemName: "stop.fill")
                    .font(.system(size: 11, weight: .bold))
                    .frame(width: 30, height: 30)
                    .background(.primary.opacity(0.85), in: .circle)
                    .foregroundStyle(.background)
            }
            .buttonStyle(.plain)
            .help("Stop turn (⌘.)")
            .keyboardShortcut(".", modifiers: .command)
        } else {
            Button { Task { await model.submit() } } label: {
                Image(systemName: "arrow.up")
                    .font(.system(size: 13, weight: .bold))
                    .frame(width: 30, height: 30)
                    .background(canSend ? Color.brandFill : Color.secondary.opacity(0.25), in: .circle)
                    .foregroundStyle(.white)
            }
            .buttonStyle(.plain)
            .disabled(!canSend)
            .help("Send (⏎)")
        }
    }

    private var attachmentChips: some View {
        ScrollView(.horizontal) {
            HStack(spacing: 6) {
                ForEach(model.attachments, id: \.self) { url in
                    HStack(spacing: 5) {
                        Image(nsImage: NSWorkspace.shared.icon(forFile: url.path))
                            .resizable()
                            .frame(width: 16, height: 16)
                        Text(url.lastPathComponent).font(.callout).lineLimit(1)
                        Button { model.attachments.removeAll { $0 == url } } label: {
                            Image(systemName: "xmark.circle.fill").foregroundStyle(.secondary)
                        }
                        .buttonStyle(.plain)
                        .help("Remove")
                    }
                    .padding(.leading, 8)
                    .padding(.trailing, 6)
                    .padding(.vertical, 5)
                    .background(.fill.tertiary, in: .capsule)
                    .help(url.path.abbreviatingHome)
                }
            }
        }
        .scrollIndicators(.never)
    }
}

extension String {
    var abbreviatingHome: String { (self as NSString).abbreviatingWithTildeInPath }
}

struct AddPanel: View {
    @ObservedObject var model: DesktopModel

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            PopoverRow("Attach Files or Folders…", systemImage: "paperclip") {
                model.composerPanel = nil
                model.chooseAttachments()
            }
            Divider().padding(.vertical, 4).padding(.horizontal, 10).opacity(0.5)
            PopoverRow("New Chat", systemImage: "square.and.pencil") {
                model.composerPanel = nil
                Task { await model.newSession() }
            }
            PopoverRow("Choose Project…", systemImage: "folder") {
                model.composerPanel = nil
                model.chooseWorkspace()
            }
        }
    }
}

/// The model list with a filter, used by the composer panel and in Settings.
/// Esc clears the filter, then closes.
struct ModelListPanel: View {
    @ObservedObject var model: DesktopModel
    var maxHeight: CGFloat = 340
    let close: () -> Void
    @State private var filter = ""
    @FocusState private var focused: Bool

    private var choices: [ModelChoice] {
        let filter = filter.trimmingCharacters(in: .whitespaces)
        guard !filter.isEmpty else { return model.models.choices }
        return model.models.choices.filter {
            $0.name.localizedCaseInsensitiveContains(filter) || $0.id.localizedCaseInsensitiveContains(filter)
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 6) {
                Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
                TextField("Filter models", text: $filter)
                    .textFieldStyle(.plain)
                    .focused($focused)
                    .onSubmit { if let first = choices.first { pick(first) } }
                    .onKeyPress(.escape) {
                        if filter.isEmpty { close() } else { filter = "" }
                        return .handled
                    }
            }
            .padding(.horizontal, 12)
            .frame(height: 34)
            .background(Color.primary.opacity(0.06), in: .capsule)
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    ForEach(choices) { choice in
                        PopoverRow(choice.name, detail: choice.detail == choice.name ? nil : choice.detail,
                                   isSelected: choice.id == model.selectedModel) { pick(choice) }
                    }
                    if choices.isEmpty {
                        Text("No matching models").font(.callout).foregroundStyle(.secondary).padding(10)
                    }
                }
            }
            .frame(maxHeight: maxHeight)
            .fixedSize(horizontal: false, vertical: true)
        }
        .onAppear { focused = true }
    }

    private func pick(_ choice: ModelChoice) {
        close()
        Task { await model.setModel(choice.id) }
    }
}

struct PopoverRow: View {
    let title: String
    var systemImage: String?
    var detail: String?
    var isSelected = false
    let action: () -> Void
    @State private var isHovered = false

    init(_ title: String, systemImage: String? = nil, detail: String? = nil, isSelected: Bool = false,
         action: @escaping () -> Void) {
        self.title = title
        self.systemImage = systemImage
        self.detail = detail
        self.isSelected = isSelected
        self.action = action
    }

    var body: some View {
        Button(action: action) {
            HStack(spacing: 10) {
                if let systemImage {
                    Image(systemName: systemImage).frame(width: 18).foregroundStyle(.secondary)
                }
                VStack(alignment: .leading, spacing: 1) {
                    Text(title).lineLimit(1)
                    if let detail, !detail.isEmpty {
                        Text(detail).font(.caption).foregroundStyle(.secondary).lineLimit(2)
                    }
                }
                Spacer(minLength: 12)
                if isSelected {
                    Image(systemName: "checkmark").font(.callout.weight(.semibold)).foregroundStyle(Color.brand)
                }
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 7)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(isHovered ? AnyShapeStyle(.fill.tertiary) : AnyShapeStyle(.clear), in: .rect(cornerRadius: 14))
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .onHover { isHovered = $0 }
    }
}
