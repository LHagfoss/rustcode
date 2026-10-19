import AppKit
import RustCodeKit
import SwiftUI

/// The window has no title bar: a gray sidebar runs to the top edge beside the
/// chat, with no split-view divider between them.
struct ContentView: View {
    @ObservedObject var model: DesktopModel
    @AppStorage("previewWidth") private var previewWidth: Double = 480

    var body: some View {
        HStack(spacing: 0) {
            if model.showsSidebar {
                SessionSidebar(model: model, updater: model.updater)
                    .frame(width: 270)
                    .transition(.move(edge: .leading))
            }
            ChatView(model: model, chat: model.chat)
                .background(Color.chatBackground)
            if model.showsPreview {
                HStack(spacing: 0) {
                    PreviewResizeHandle(width: $previewWidth)
                    PreviewPanel(preview: model.preview) { model.togglePreview() }
                        .frame(width: CGFloat(previewWidth))
                }
                .background(Color.chatBackground)
                .transition(.move(edge: .trailing))
            }
        }
        .background(Color.sidebarBackground)
        .ignoresSafeArea()
        .tint(.brand)
        .background(WindowChrome())
        .sheet(isPresented: $model.showsSettings) { SettingsView(model: model) }
        .sheet(item: $model.renamingSession) { RenameSheet(model: model, session: $0) }
        .sheet(item: $model.contextSheet) { ContextSheet(context: $0) }
        .sheet(item: $model.commandResult) { CommandResultSheet(result: $0) }
        .overlay { ToastStack(model: model) }
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

/// Toolbar buttons grouped in one Liquid Glass capsule, as in Finder.
struct ToolbarCluster<Content: View>: View {
    @ViewBuilder let content: Content

    var body: some View {
        HStack(spacing: 0) { content }
            .padding(.horizontal, 3)
            .frame(height: 28)
            .glassSurface(in: .capsule)
    }
}

/// A button inside a `ToolbarCluster`. Symbols are centred on their visible
/// shape, so a pencil reaching above its square does not sit lower than its
/// neighbours.
struct ClusterButton: View {
    let systemImage: String
    let help: String
    var isOn = false
    let action: () -> Void
    @State private var isHovered = false

    var body: some View {
        Button(action: action) {
            Image(systemName: systemImage)
                .font(.system(size: 14, weight: .medium))
                .foregroundStyle(isOn ? AnyShapeStyle(Color.brand) : AnyShapeStyle(.secondary))
                .offset(y: Self.opticalOffset[systemImage] ?? 0)
                .frame(width: 32, height: 24)
                .background(isHovered || isOn ? AnyShapeStyle(.fill.tertiary) : AnyShapeStyle(.clear), in: .capsule)
                .contentShape(.capsule)
        }
        .buttonStyle(.plain)
        .onHover { isHovered = $0 }
        .help(help)
        .accessibilityLabel(help)
    }

    private static let opticalOffset: [String: CGFloat] = ["square.and.pencil": -1]
}

private struct SessionSidebar: View {
    @ObservedObject var model: DesktopModel
    @ObservedObject var updater: Updater
    @FocusState private var searchFocused: Bool
    /// The one chat row whose swipe actions are showing.
    @State var openSwipeRow: String?
    /// The one chat under the pointer. Kept here, not per row: a row that
    /// moves out from under a still pointer never hears it leave, which left
    /// several rows lit at once.
    @State var hoveredSession: String?

    /// The hidden title bar's height, so buttons line up with the traffic lights.
    static let barHeight: CGFloat = 28
    /// Room above the toolbar clusters so their glass does not touch the
    /// window's top edge.
    static let barTopInset: CGFloat = 6

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            // Room for the traffic lights, then the bar's own buttons.
            HStack(spacing: 2) {
                Spacer()
                ToolbarCluster {
                    ClusterButton(systemImage: "sidebar.left", help: "Hide sidebar (⌘B)") { model.toggleSidebar() }
                    ClusterButton(systemImage: "archivebox", help: model.showsArchived ? "Back to chats" : "Archived chats",
                                  isOn: model.showsArchived) {
                        withAnimation(.snappy(duration: 0.2)) { model.showsArchived.toggle() }
                    }
                    ClusterButton(systemImage: "square.and.pencil", help: "New chat (⌘N)") { Task { await model.newSession() } }
                }
            }
            .padding(.leading, 80)
            .padding(.trailing, 10)
            .frame(height: Self.barHeight)
            .padding(.top, Self.barTopInset)

            HStack(spacing: 6) {
                Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
                TextField("Search chats", text: $model.sessionFilter)
                    .textFieldStyle(.plain)
                    .focused($searchFocused)
                    .onChange(of: model.focusSessionSearch) { _, focus in
                        if focus { searchFocused = true; model.focusSessionSearch = false }
                    }
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
            .padding(.horizontal, 12)
            .frame(height: 30)
            .glassSurface(in: .capsule)
            .padding(.horizontal, 12)
            .padding(.top, 8)

            HStack {
                Text(model.showsArchived ? "Archived" : "Chats")
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(.secondary)
                Spacer()
                ScopeToggle(showsAll: $model.showsAllProjects)
            }
            .padding(.leading, 20)
            .padding(.trailing, 12)
            .padding(.top, 12)
            .padding(.bottom, 4)

            ScrollView {
                LazyVStack(alignment: .leading, spacing: 2) {
                    let pinned = model.visibleSessions.filter { model.pinnedSessions.contains($0.id) }
                    if !pinned.isEmpty {
                        Label("Pinned", systemImage: "pin.fill")
                            .font(.caption.weight(.semibold))
                            .foregroundStyle(.secondary)
                            .padding(.horizontal, 10)
                            .padding(.top, 2)
                        ForEach(pinned) { row($0) }
                        Divider().opacity(0.4).padding(.vertical, 6).padding(.horizontal, 10)
                    }
                    ForEach(model.visibleSessions.filter { !model.pinnedSessions.contains($0.id) }) { row($0) }
                }
                .padding(.horizontal, 10)
            }
            .overlay {
                if !model.sessionsLoaded {
                    ProgressView().controlSize(.small)
                } else if model.visibleSessions.isEmpty {
                    Text(model.showsArchived ? "No archived chats." : model.sessions.isEmpty ? "No chats in this project yet." : "No matching chats.")
                        .font(.callout)
                        .foregroundStyle(.secondary)
                }
            }

            if !model.archivedSessions.isEmpty || model.showsArchived {
                Button { withAnimation(.snappy(duration: 0.2)) { model.showsArchived.toggle() } } label: {
                    HStack(spacing: 8) {
                        Image(systemName: model.showsArchived ? "chevron.left" : "archivebox")
                            .frame(width: 16)
                        Text(model.showsArchived ? "Back to Chats" : "Archived")
                        Spacer(minLength: 0)
                        if !model.showsArchived {
                            Text("\(model.archivedSessions.count)").foregroundStyle(.tertiary).monospacedDigit()
                        }
                    }
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .padding(.horizontal, 12)
                    .frame(height: 30)
                    .contentShape(.rect)
                }
                .buttonStyle(.plain)
                .padding(.horizontal, 10)
                .padding(.top, 4)
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

extension SessionSidebar {
    @ViewBuilder
    func row(_ session: SessionSummary) -> some View {
        let isPinned = model.pinnedSessions.contains(session.id)
        SwipeRow(id: session.id, openRow: $openSwipeRow,
                 leading: [SwipeAction(title: isPinned ? "Unpin" : "Pin", systemImage: isPinned ? "pin.slash.fill" : "pin.fill",
                                       tint: .brand) { model.togglePin(session) }],
                 trailing: [
                    SwipeAction(title: "Rename", systemImage: "pencil", tint: .secondary) { model.renamingSession = session },
                    SwipeAction(title: model.archivedSessions.contains(session.id) ? "Unarchive" : "Archive",
                                systemImage: model.archivedSessions.contains(session.id) ? "tray.and.arrow.up.fill" : "archivebox.fill",
                                tint: .orange) { model.toggleArchive(session) },
                    SwipeAction(title: "Trash", systemImage: "trash.fill", tint: .red) { Task { await model.trash(session) } },
                 ]) {
        SessionRow(session: session,
                   showsProject: model.showsAllProjects,
                   isSelected: session.id == model.selectedSession,
                   isPinned: model.pinnedSessions.contains(session.id),
                   needsAnswer: model.permissionQueue.contains { $0.sessionId == session.id },
                   isRunning: model.runningSessions.contains(session.id),
                   searchSnippet: model.searchSnippet(for: session),
                   searchQuery: model.sessionFilter.trimmingCharacters(in: .whitespaces),
                   isHovered: hoveredSession == session.id,
                   onHover: { inside in
                       if inside { hoveredSession = session.id } else if hoveredSession == session.id { hoveredSession = nil }
                   }) {
            guard session.id != model.selectedSession else { return }
            Task { await model.select(session) }
        }
        }
        .contextMenu { SessionMenu(model: model, session: session) }
    }
}

/// Right-click on a chat in the sidebar.
private struct SessionMenu: View {
    @ObservedObject var model: DesktopModel
    let session: SessionSummary

    var body: some View {
        Button("Open", systemImage: "bubble.left") { Task { await model.select(session) } }
        Button("Rename…", systemImage: "pencil") { model.renamingSession = session }
        Button(model.pinnedSessions.contains(session.id) ? "Unpin" : "Pin", systemImage: "pin") { model.togglePin(session) }
        Divider()
        Button("Copy Title", systemImage: "doc.on.doc") { Clipboard.copy(session.title ?? "New chat") }
        Button("Copy Session ID", systemImage: "number") { Clipboard.copy(session.id) }
        Button("Show in Finder", systemImage: "folder") { Task { await model.revealInFinder(session) } }
        if !session.cwd.isEmpty {
            Button("Open Project in Finder", systemImage: "folder.badge.gearshape") {
                NSWorkspace.shared.open(URL(fileURLWithPath: session.cwd, isDirectory: true))
            }
            Button("Search Chats in \(session.projectName)", systemImage: "line.3.horizontal.decrease") {
                model.sessionFilter = session.projectName
            }
        }
        Divider()
        Button(model.archivedSessions.contains(session.id) ? "Unarchive" : "Archive",
               systemImage: model.archivedSessions.contains(session.id) ? "tray.and.arrow.up" : "archivebox") {
            model.toggleArchive(session)
        }
        Button("Move to Trash", systemImage: "trash", role: .destructive) { Task { await model.trash(session) } }
    }
}

/// Edits a chat's title.
struct RenameSheet: View {
    @ObservedObject var model: DesktopModel
    let session: SessionSummary
    @State private var title: String
    @FocusState private var focused: Bool
    @Environment(\.dismiss) private var dismiss

    init(model: DesktopModel, session: SessionSummary) {
        self.model = model
        self.session = session
        _title = State(initialValue: session.title ?? "")
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Rename Chat").font(.headline)
            TextField("Title", text: $title)
                .textFieldStyle(.plain)
                .padding(.horizontal, 12)
                .frame(height: 34)
                .background(Color.primary.opacity(0.06), in: .rect(cornerRadius: 10))
                .focused($focused)
                .onSubmit(save)
            HStack {
                Spacer()
                Button("Cancel") { dismiss() }
                    .buttonStyle(PillButtonStyle())
                    .keyboardShortcut(.cancelAction)
                Button("Rename", action: save)
                    .buttonStyle(PillButtonStyle(prominent: true))
                    .keyboardShortcut(.defaultAction)
                    .disabled(title.trimmingCharacters(in: .whitespaces).isEmpty)
            }
        }
        .padding(20)
        .frame(width: 380)
        .onAppear { focused = true }
    }

    private func save() {
        let title = title
        dismiss()
        Task { await model.rename(session, to: title) }
    }
}

/// "All" or "Project": which chats the sidebar lists. The native segmented
/// control, which is Liquid Glass on macOS 26.
private struct ScopeToggle: View {
    @Binding var showsAll: Bool
    @Namespace private var selection

    var body: some View {
        HStack(spacing: 0) {
            segment("All", isOn: showsAll) { showsAll = true }
            segment("Project", isOn: !showsAll) { showsAll = false }
        }
        .padding(2)
        .glassSurface(in: .capsule)
        .animation(.snappy(duration: 0.2), value: showsAll)
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Chats")
    }

    private func segment(_ title: String, isOn: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(title)
                .font(.caption.weight(.medium))
                .foregroundStyle(isOn ? .primary : .secondary)
                .padding(.horizontal, 10)
                .frame(height: 20)
                .background {
                    if isOn {
                        Capsule().fill(Color.primary.opacity(0.14))
                            .matchedGeometryEffect(id: "selection", in: selection)
                    }
                }
                .contentShape(.capsule)
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(isOn ? .isSelected : [])
    }
}

private struct SessionRow: View {
    let session: SessionSummary
    var showsProject = false
    let isSelected: Bool
    var isPinned = false
    let needsAnswer: Bool
    let isRunning: Bool
    /// Set for a chat found by its contents: the text around the match.
    var searchSnippet: String?
    var searchQuery = ""
    var isHovered = false
    var onHover: (Bool) -> Void = { _ in }
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 8) {
                VStack(alignment: .leading, spacing: 2) {
                    Text(session.title ?? "New chat")
                        .lineLimit(1)
                    if let searchSnippet {
                        Text(highlighted(searchSnippet))
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .lineLimit(2)
                    } else {
                        // Where and with which model on the left, when on the right.
                        HStack(spacing: 4) {
                            if showsProject, !session.projectName.isEmpty {
                                Label(session.projectName, systemImage: "folder")
                                    .labelStyle(.titleAndIcon)
                                    .lineLimit(1)
                                if session.modelName != nil { Text("·") }
                            }
                            if let model = session.modelName {
                                Text(model).lineLimit(1).truncationMode(.middle)
                            }
                            Spacer(minLength: 6)
                            if let updated = session.updatedAt {
                                Text(updated, format: .relative(presentation: .named))
                                    .lineLimit(1)
                                    .layoutPriority(1)
                            }
                        }
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
        .onContinuousHover { phase in
            if case .active = phase { onHover(true) } else { onHover(false) }
        }
        // A context menu or a pin that moves the row never reports the pointer
        // leaving; the next move over a row lights that one instead.
        .onChange(of: isPinned) { _, _ in onHover(false) }
        .onReceive(NotificationCenter.default.publisher(for: NSMenu.didEndTrackingNotification)) { _ in
            onHover(false)
        }
    }

    /// The snippet with every occurrence of the query emphasized.
    private func highlighted(_ snippet: String) -> AttributedString {
        var text = AttributedString(snippet)
        guard !searchQuery.isEmpty else { return text }
        var rest = text.startIndex..<text.endIndex
        while let match = text[rest].range(of: searchQuery, options: [.caseInsensitive, .diacriticInsensitive]) {
            text[match].foregroundColor = .primary
            text[match].inlinePresentationIntent = .stronglyEmphasized
            rest = match.upperBound..<text.endIndex
        }
        return text
    }

    private var rowFill: Color {
        isSelected ? Color.primary.opacity(0.09) : isHovered ? Color.primary.opacity(0.04) : .clear
    }
}

private struct ChatView: View {
    @ObservedObject var model: DesktopModel
    /// The transcript publishes on its own, so streaming redraws the chat
    /// but not the sidebar or the composer.
    @ObservedObject var chat: LiveTranscript
    @State private var composerHeight: CGFloat = 60
    /// The chat keeps the newest output in view. Scrolling up hands control to
    /// the reader and shows the Latest button; scrolling back down follows again.
    @State private var followsLatest = true

    private var selected: SessionSummary? { model.sessions.first { $0.id == model.selectedSession } }

    private var title: String { model.selectedSession == nil ? "New Chat" : selected?.title ?? "RustCode" }

    /// A chat from another project shows that project, not the open one.
    private var projectName: String {
        if let name = selected?.projectName, !name.isEmpty { return name }
        return model.workspace.lastPathComponent
    }

    var body: some View {
        let transcript = chat.transcript
        // Found once per redraw: asked per row, every row searched the chat.
        let currentMatch = model.showsFind ? model.currentMatch : nil
        let lastId = transcript.items.last?.id
        ScrollViewReader { proxy in
            ScrollView {
                // The Latest button is a pinned footer, inside the scroll view,
                // so the wheel still scrolls the chat while the pointer is on it.
                LazyVStack(alignment: .leading, spacing: 0, pinnedViews: .sectionFooters) {
                    Section {
                        // Rows are laid out eagerly: lazy rows took their real
                        // height only on screen, so scrolling up made the chat
                        // jump and the scroller resize under the pointer. Rows
                        // are equatable, so only the one that changed redraws.
                        VStack(spacing: 0) {
                            VStack(alignment: .leading, spacing: 18) {
                                ForEach(transcript.items) { item in
                                    TranscriptRow(item: item, isCurrentMatch: item.id == currentMatch,
                                                  isStreaming: isReplyStreaming && item.id == lastId)
                                        .equatable()
                                        .id(item.id)
                                }
                                // Up for the whole turn, also while a reply streams or the
                                // model plans its next step, so a live turn always says so.
                                if isReplyStreaming {
                                    WorkingIndicator()
                                }
                            }
                            .frame(maxWidth: 760)
                            .padding(.horizontal, 28)
                            .padding(.vertical, 20)
                            .frame(maxWidth: .infinity)
                            // The very end of the chat, below its padding: a jump
                            // here leaves nothing further to scroll, so one press
                            // of Latest always lands where following resumes.
                            Color.clear.frame(height: 1).id(bottomAnchor)
                        }
                    } footer: {
                        // No height of its own: the button floats over the
                        // chat's end, so showing or hiding it never changes how
                        // far the chat scrolls or moves its end under the reader.
                        Color.clear
                            .frame(maxWidth: .infinity)
                            .frame(height: 0)
                            .overlay(alignment: .bottomTrailing) {
                                if !followsLatest, !transcript.isEmpty {
                                    LatestButton { jumpToLatest(proxy) }
                                        .padding(.bottom, 12)
                                        .transition(.opacity.combined(with: .scale(scale: 0.9)))
                                }
                            }
                            .frame(maxWidth: 760)
                            .padding(.horizontal, 28)
                            .frame(maxWidth: .infinity)
                    }
                }
            }
            .overlay {
                if model.loadingSession != nil, model.loadingSession == model.selectedSession {
                    ProgressView("Loading chat…").controlSize(.small).foregroundStyle(.secondary)
                } else if transcript.isEmpty, !model.busy {
                    EmptyChat(workspace: model.workspace)
                }
            }
            .followsLatest($followsLatest)
            .onGeometryChange(for: CGRect.self) { $0.frame(in: .global) } action: { ChatScrollInput.shared.chatFrame = $0 }
            .animation(.snappy(duration: 0.2), value: followsLatest)
            .onChange(of: chat.transcript) { old, new in
                // Sending a message always brings the chat back to its end.
                if new.items.count > old.items.count, case .user = new.items.last?.content { followsLatest = true }
                if followsLatest, !model.showsFind { scrollToEnd(proxy) }
            }
            .onChange(of: isReplyStreaming) { _, _ in
                if followsLatest, !model.showsFind { scrollToEnd(proxy) }
            }
            .onChange(of: model.selectedSession) { _, _ in
                followsLatest = true
                scrollToEnd(proxy)
            }
            .onChange(of: currentMatch) { _, match in
                guard let match else { return }
                followsLatest = false
                withAnimation(.snappy(duration: 0.25)) { proxy.scrollTo(match, anchor: .center) }
            }
        }
        .environment(\.findQuery, model.showsFind ? model.findQuery : "")
        .environment(\.projectRoot, selected.map(\.cwd).flatMap { $0.isEmpty ? nil : $0 } ?? model.workspace.path)
        .environment(\.messageActions, model.messageActions)
        .safeAreaInset(edge: .top, spacing: 0) {
            VStack(spacing: 0) {
                header
                if model.showsFind {
                    FindBar(model: model, chat: chat)
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
                Composer(model: model, composer: model.composer)
                    .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { composerHeight = $0 }
            }
            .frame(maxWidth: 760)
            .padding(.horizontal, 28)
            .padding(.bottom, 16)
            .frame(maxWidth: .infinity)
        }
        .overlay { composerPanel }
        .overlay { CommandSuggestions(model: model, composer: model.composer, composerHeight: composerHeight) }
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
                    if panel != .add { Spacer(minLength: 0) }
                    Group {
                        switch panel {
                        case .add: AddPanel(model: model)
                        case .model: ModelListPanel(model: model) { model.composerPanel = nil }
                        case .effort: EffortPanel(model: model)
                        }
                    }
                    .padding(6)
                    .frame(width: panel == .model ? 340 : panel == .effort ? 300 : 260, alignment: .leading)
                    .glassSurface(in: .rect(cornerRadius: 20))
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
                ToolbarCluster {
                    ClusterButton(systemImage: "sidebar.left", help: "Show sidebar (⌘B)") { model.toggleSidebar() }
                    ClusterButton(systemImage: "square.and.pencil", help: "New chat (⌘N)") { Task { await model.newSession() } }
                }
                .padding(.leading, 70)
            }
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                Text(title).font(.headline).lineLimit(1)
                Text(projectName).font(.callout).foregroundStyle(.secondary).lineLimit(1)
            }
            Spacer()
            if let usage = chat.transcript.usage {
                Button { model.showContext() } label: { UsageCapsule(usage: usage) }
                    .buttonStyle(.plain)
                    .accessibilityHint("Shows how the context window is used")
            }
            if let session = model.sessions.first(where: { $0.id == model.selectedSession }) {
                ToolbarCluster {
                    ClusterButton(systemImage: "magnifyingglass", help: "Find in chat (⌘F)", isOn: model.showsFind) {
                        model.showsFind.toggle()
                    }
                    ClusterButton(systemImage: model.pinnedSessions.contains(session.id) ? "pin.fill" : "pin",
                                  help: model.pinnedSessions.contains(session.id) ? "Unpin chat" : "Pin chat",
                                  isOn: model.pinnedSessions.contains(session.id)) { model.togglePin(session) }
                    ClusterButton(systemImage: "folder", help: "Show chat in Finder") {
                        Task { await model.revealInFinder(session) }
                    }
                }
            }
            if !model.showsPreview {
                ToolbarCluster {
                    ClusterButton(systemImage: "globe", help: "Show localhost preview (⇧⌘P)") { model.togglePreview() }
                }
            }
        }
        .padding(.horizontal, 18)
        .frame(height: SessionSidebar.barHeight)
        .padding(.top, SessionSidebar.barTopInset)
        .padding(.bottom, 8)
        .background(Color.chatBackground.opacity(0.94))
    }

    private let bottomAnchor = "bottom"

    /// Rows below the fold get their real height only once laid out, so one
    /// jump can land short of the end; a second one after layout reaches it.
    private func scrollToEnd(_ proxy: ScrollViewProxy) {
        proxy.scrollTo(bottomAnchor, anchor: .bottom)
        Task { @MainActor in
            await Task.yield()
            if followsLatest { proxy.scrollTo(bottomAnchor, anchor: .bottom) }
        }
    }

    private func jumpToLatest(_ proxy: ScrollViewProxy) {
        followsLatest = true
        scrollToEnd(proxy)
    }

    /// The shown chat has a turn in progress, so its last reply may still grow.
    private var isReplyStreaming: Bool {
        model.isTurnRunning
    }
}

/// Slash commands matching what is typed, above the composer. ↑↓ choose,
/// Tab or ⏎ completes, Esc clears. Its own view, so typing redraws it and
/// not the chat behind it.
private struct CommandSuggestions: View {
    @ObservedObject var model: DesktopModel
    @ObservedObject var composer: ComposerDraft
    let composerHeight: CGFloat

    var body: some View {
        let suggestions = model.commandSuggestions
        if model.composerPanel == nil, !suggestions.isEmpty {
            VStack {
                Spacer()
                ScrollViewReader { proxy in
                    ScrollView {
                        VStack(alignment: .leading, spacing: 0) {
                            ForEach(Array(suggestions.enumerated()), id: \.element.id) { index, command in
                                CommandRow(command: command, isSelected: index == min(model.commandSelection, suggestions.count - 1)) {
                                    model.commandSelection = index
                                    model.completeCommand()
                                }
                                .id(command.id)
                            }
                        }
                        .padding(6)
                    }
                    .frame(maxHeight: 280)
                    .fixedSize(horizontal: false, vertical: true)
                    .onChange(of: model.commandSelection) { _, index in
                        guard index < suggestions.count else { return }
                        proxy.scrollTo(suggestions[index].id)
                    }
                }
                .clipShape(.rect(cornerRadius: 20))
                .glassSurface(in: .rect(cornerRadius: 20))
                .frame(maxWidth: 760)
                .padding(.horizontal, 28)
                .padding(.bottom, composerHeight + 16 + 8)
                .frame(maxWidth: .infinity)
            }
            .transition(.opacity)
        }
    }
}

/// Jumps back to the newest message after the reader scrolled up.
private struct LatestButton: View {
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Label("Latest", systemImage: "arrow.down")
                .font(.callout.weight(.medium))
                .padding(.horizontal, 12)
                .frame(height: 30)
                .background(.regularMaterial, in: .capsule)
                .overlay(Capsule().strokeBorder(.separator))
                .shadow(color: .black.opacity(0.15), radius: 6, y: 2)
                .contentShape(.capsule)
        }
        .buttonStyle(.plain)
        .help("Scroll to the latest message")
    }
}

private extension View {
    /// Sets `follows` the way chat apps do: any scroll up by the reader lets
    /// go of the end at once, scrolling back down to it holds on again. While
    /// held, the chat stays anchored to its end as output grows; once let go,
    /// output growing below never moves what the reader is looking at.
    @ViewBuilder
    func followsLatest(_ follows: Binding<Bool>) -> some View {
        if #available(macOS 15, *) {
            defaultScrollAnchor(follows.wrappedValue ? .bottom : .top, for: .sizeChanges)
            .onScrollGeometryChange(for: ScrollFollowGeometry.self) { geometry in
                ScrollFollowGeometry(
                    offset: geometry.contentOffset.y,
                    containerHeight: geometry.containerSize.height,
                    distanceFromBottom: geometry.contentSize.height + geometry.contentInsets.bottom
                        - (geometry.contentOffset.y + geometry.containerSize.height),
                    canScroll: geometry.contentSize.height + geometry.contentInsets.top + geometry.contentInsets.bottom
                        > geometry.containerSize.height + 1)
            } action: { old, new in
                let input = ChatScrollInput.shared
                input.canScroll = new.canScroll
                // Only the reader's own input decides: output growing, rows
                // settling, a resize or a jump to a find match move the
                // geometry too, and must neither stop nor restart following.
                guard input.isActive, old.containerHeight == new.containerHeight else { return }
                let atEnd = new.distanceFromBottom <= ScrollFollowGeometry.threshold
                if atEnd, !input.scrolledUpJustNow {
                    if !follows.wrappedValue { follows.wrappedValue = true }
                } else if new.offset < old.offset - 0.5, follows.wrappedValue {
                    // Dragging the scroller up.
                    follows.wrappedValue = false
                }
            }
            .onAppear {
                ChatScrollInput.shared.start()
                ChatScrollInput.shared.onScrollUp = {
                    if follows.wrappedValue, ChatScrollInput.shared.canScroll { follows.wrappedValue = false }
                }
            }
        } else {
            self
        }
    }
}

/// The reader's own scrolling of the chat: wheel and trackpad events, and
/// drags of its scroller, over the chat's frame.
@MainActor
private final class ChatScrollInput {
    static let shared = ChatScrollInput()
    /// The chat's scroll view in window coordinates, top-left origin.
    var chatFrame: CGRect = .zero
    var canScroll = false
    var onScrollUp: () -> Void = {}
    private var lastInput = Date.distantPast
    private var lastScrollUp = Date.distantPast
    private var monitor: Any?

    var isActive: Bool { Date().timeIntervalSince(lastInput) < 0.3 }
    var scrolledUpJustNow: Bool { Date().timeIntervalSince(lastScrollUp) < 0.15 }

    func start() {
        guard monitor == nil else { return }
        monitor = NSEvent.addLocalMonitorForEvents(matching: [.scrollWheel, .leftMouseDragged]) { event in
            MainActor.assumeIsolated { ChatScrollInput.shared.observe(event) }
            return event
        }
    }

    private func observe(_ event: NSEvent) {
        guard let content = event.window?.contentView else { return }
        let location = event.locationInWindow
        guard chatFrame.contains(CGPoint(x: location.x, y: content.bounds.height - location.y)) else { return }
        lastInput = Date()
        // Positive is toward the top of the chat, with natural scrolling or without.
        if event.type == .scrollWheel, event.scrollingDeltaY > 0 {
            lastScrollUp = Date()
            onScrollUp()
        }
    }
}

private struct ScrollFollowGeometry: Equatable {
    /// How close to the end still counts as following the newest message.
    static let threshold: CGFloat = 24
    var offset: CGFloat
    var containerHeight: CGFloat
    var distanceFromBottom: CGFloat
    var canScroll: Bool
}

/// ⌘F: find in the chat. ⏎ and ⌘G go to the next match, ⇧⏎ and ⇧⌘G back, Esc closes.
private struct FindBar: View {
    @ObservedObject var model: DesktopModel
    /// Matches change as the transcript does.
    @ObservedObject var chat: LiveTranscript
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
        .glassSurface(in: .capsule)
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
    @ObservedObject var composer: ComposerDraft
    @FocusState private var focused: Bool
    @State private var isDropTarget = false
    @State private var pasteMonitor: Any?

    private var canSend: Bool {
        !model.draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            || !model.attachments.isEmpty || !model.images.isEmpty || model.draftCommand != nil
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            if !model.selectedQueue.isEmpty { QueuedPromptList(model: model) }
            if !model.images.isEmpty { imageChips }
            if !model.attachments.isEmpty { attachmentChips }
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                if let chip = model.draftCommand {
                    Text(chip.name)
                        .font(.body.weight(.semibold))
                        .foregroundStyle(Color.brand)
                        .padding(.horizontal, 8)
                        .padding(.vertical, 2)
                        .background(Color.brand.opacity(0.15), in: .capsule)
                        .help(chip.description)
                        .transition(.scale(scale: 0.9).combined(with: .opacity))
                }
                messageField
            }
            .animation(.snappy(duration: 0.15), value: model.draftCommand)
                .padding(.horizontal, 6)
                .padding(.top, 4)
                .onKeyPress(.return, phases: .down) { press in
                    // ⇧⏎ (and ⌥⏎) start a new line at the cursor.
                    if press.modifiers.contains(.shift) || press.modifiers.contains(.option) {
                        NSApp.sendAction(#selector(NSText.insertNewlineIgnoringFieldEditor(_:)), to: nil, from: nil)
                        return .handled
                    }
                    // ⏎ on a suggestion takes it: a command that wants arguments
                    // waits for them, any other runs at once.
                    let suggestions = model.commandSuggestions
                    if !suggestions.isEmpty {
                        let takesArguments = suggestions[min(model.commandSelection, suggestions.count - 1)].takesArguments
                        model.completeCommand()
                        if takesArguments { return .handled }
                    }
                    Task { await model.submit() }
                    return .handled
                }
                .onKeyPress(.tab) {
                    guard !model.commandSuggestions.isEmpty else { return .ignored }
                    model.completeCommand()
                    return .handled
                }
                .onKeyPress(.upArrow) {
                    guard !model.commandSuggestions.isEmpty else { return .ignored }
                    model.moveCommandSelection(-1)
                    return .handled
                }
                .onKeyPress(.downArrow) {
                    guard !model.commandSuggestions.isEmpty else { return .ignored }
                    model.moveCommandSelection(1)
                    return .handled
                }
                .onKeyPress(.escape) {
                    if model.composerPanel != nil {
                        model.composerPanel = nil
                        return .handled
                    }
                    if !model.commandSuggestions.isEmpty || model.draftCommand != nil {
                        model.draft = ""
                        model.draftCommand = nil
                        return .handled
                    }
                    return .ignored
                }
                .onKeyPress(.delete) { model.unwrapCommandChip() ? .handled : .ignored }
                .onChange(of: composer.text) { _, _ in
                    model.commandSelection = 0
                    model.promoteTypedCommand()
                }
            HStack(spacing: 8) {
                addMenu
                Spacer(minLength: 0)
                effortMenu
                modelMenu
                trailingAction
            }
        }
        .padding(10)
        .glassSurface(in: .rect(cornerRadius: 22))
        // A click anywhere on the composer but its buttons puts the cursor in it.
        .contentShape(.rect(cornerRadius: 22))
        .onTapGesture { focused = true }
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
        .onAppear {
            focused = true
            installPasteMonitor()
        }
        .onDisappear {
            if let pasteMonitor { NSEvent.removeMonitor(pasteMonitor) }
            pasteMonitor = nil
        }
    }

    /// The text field pastes text only, so ⌘V with images on the pasteboard is
    /// caught before it and the images are attached instead.
    private func installPasteMonitor() {
        guard pasteMonitor == nil else { return }
        pasteMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [model] event in
            guard focused, event.modifierFlags.intersection(.deviceIndependentFlagsMask) == .command,
                  event.charactersIgnoringModifiers == "v", model.pasteImages() else { return event }
            return nil
        }
    }

    private var imageChips: some View {
        ScrollView(.horizontal) {
            HStack(spacing: 8) {
                ForEach(model.images) { image in
                    ImageChip(image: image) { model.images.removeAll { $0.id == image.id } }
                }
            }
            .padding(.top, 2)
        }
        .scrollIndicators(.never)
    }

    private func toggle(_ next: ComposerPanel) {
        model.composerPanel = model.composerPanel == next ? nil : next
    }

    private var placeholder: String {
        if model.draftCommand?.takesArguments == true { return "Arguments" }
        if model.draftCommand != nil { return "" }
        return model.busy ? "Queue a message…" : "Message RustCode…"
    }

    /// The text, with the rest of a half-typed command shown in gray after it.
    private var messageField: some View {
        TextField(placeholder, text: $composer.text, axis: .vertical)
            .textFieldStyle(.plain)
            .font(.body)
            .lineLimit(1...10)
            .focused($focused)
            .overlay(alignment: Alignment(horizontal: .leading, vertical: .firstTextBaseline)) {
                if let suffix = model.commandCompletionSuffix {
                    Text(ghost(suffix))
                        .font(.body)
                        .lineLimit(1)
                        .allowsHitTesting(false)
                }
            }
    }

    private func ghost(_ suffix: String) -> AttributedString {
        var typed = AttributedString(model.draft)
        typed.foregroundColor = .clear
        var rest = AttributedString(suffix)
        rest.foregroundColor = Color.secondary.opacity(0.6)
        return typed + rest
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
                .background(model.composerPanel == .model ? AnyShapeStyle(.fill.secondary) : AnyShapeStyle(.fill.tertiary), in: .capsule)
                .contentShape(.capsule)
            }
            .buttonStyle(.plain)
            .help("Model")
        }
    }

    @ViewBuilder
    private var effortMenu: some View {
        if model.models.efforts.count > 1 {
            let current = model.models.efforts.first { $0.id == model.models.effort }
            let isOn = (model.models.effort ?? "default") != "default"
            Button { toggle(.effort) } label: {
                HStack(spacing: 5) {
                    Image(systemName: "brain").font(.caption.weight(.semibold))
                    Text(isOn ? current?.name ?? "On" : "Off").lineLimit(1)
                }
                .font(.callout)
                .foregroundStyle(isOn ? Color.brand : .secondary)
                .padding(.horizontal, 11)
                .frame(height: 30)
                .background(model.composerPanel == .effort ? AnyShapeStyle(.fill.secondary) : AnyShapeStyle(.fill.tertiary), in: .capsule)
                .contentShape(.capsule)
            }
            .buttonStyle(.plain)
            .help("Reasoning effort")
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
            if canSend {
                Button { Task { await model.submit() } } label: {
                    Image(systemName: "text.line.last.and.arrowtriangle.forward")
                        .font(.system(size: 12, weight: .bold))
                        .frame(width: 30, height: 30)
                        .background(Color.brandFill, in: .circle)
                        .foregroundStyle(.white)
                }
                .buttonStyle(.plain)
                .help("Queue for when this turn ends (⏎)")
                .transition(.scale.combined(with: .opacity))
            }
            Button { Task { await model.cancel() } } label: {
                Image(systemName: "stop.fill")
                    .font(.system(size: 11, weight: .bold))
                    .frame(width: 30, height: 30)
                    .background(Color.red.gradient, in: .circle)
                    .foregroundStyle(.white)
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

/// Messages waiting for the running turn to end, sent in order. Each can be
/// taken back into the composer or removed.
private struct QueuedPromptList: View {
    @ObservedObject var model: DesktopModel

    var body: some View {
        let queue = model.selectedQueue
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 6) {
                Image(systemName: "text.line.last.and.arrowtriangle.forward")
                Text("Queued").fontWeight(.semibold)
                Text("· sent when this turn ends").foregroundStyle(.secondary)
            }
            .font(.caption)
            .foregroundStyle(Color.brand)
            .padding(.horizontal, 6)
            ForEach(Array(queue.enumerated()), id: \.element.id) { index, prompt in
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text("\(index + 1)").font(.caption.monospacedDigit()).foregroundStyle(.tertiary)
                    Text(prompt.typed.isEmpty ? "Image" : prompt.typed)
                        .font(.callout)
                        .lineLimit(2)
                        .frame(maxWidth: .infinity, alignment: .leading)
                    if !prompt.images.isEmpty {
                        Image(systemName: "photo").font(.caption).foregroundStyle(.secondary)
                    }
                    Button { model.editQueued(prompt) } label: { Image(systemName: "pencil") }
                        .buttonStyle(.plain)
                        .foregroundStyle(.secondary)
                        .help("Edit")
                    Button { model.removeQueued(prompt) } label: { Image(systemName: "xmark.circle.fill") }
                        .buttonStyle(.plain)
                        .foregroundStyle(.secondary)
                        .help("Remove from queue")
                }
                .padding(.horizontal, 8)
                .padding(.vertical, 6)
                .background(.fill.tertiary, in: .rect(cornerRadius: 12))
            }
        }
        .animation(.snappy(duration: 0.2), value: queue)
    }
}

private struct CommandRow: View {
    let command: SlashCommand
    let isSelected: Bool
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 12) {
                Text(command.name)
                    .font(.callout.weight(.semibold))
                    .foregroundStyle(Color.brand)
                    .frame(minWidth: 110, alignment: .leading)
                Text(command.description)
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                Spacer(minLength: 8)
                if command.action != .execute {
                    Image(systemName: "macwindow").font(.caption).foregroundStyle(.tertiary)
                        .help("Handled by the app")
                }
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 7)
            .background(isSelected ? Color.brand.opacity(0.18) : .clear, in: .rect(cornerRadius: 14))
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
    }
}

/// ↑↓ move, ⏎ chooses, Esc closes.
struct AddPanel: View {
    @ObservedObject var model: DesktopModel
    @State private var highlighted = 0
    @FocusState private var focused: Bool

    private var items: [(title: String, image: String, run: () -> Void)] {
        [
            ("Attach Files or Folders…", "paperclip", { model.chooseAttachments() }),
            ("Attach Image…", "photo", { model.chooseImages() }),
            ("New Chat", "square.and.pencil", { Task { await model.newSession() } }),
            ("Choose Project…", "folder", { model.chooseWorkspace() }),
        ]
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            ForEach(Array(items.enumerated()), id: \.offset) { index, item in
                if index == 2 { Divider().padding(.vertical, 4).padding(.horizontal, 10).opacity(0.5) }
                PopoverRow(item.title, systemImage: item.image, isHighlighted: index == highlighted) { run(index) }
            }
        }
        .panelKeys(focused: $focused, count: items.count, highlighted: $highlighted,
                   choose: { run(highlighted) }, close: { model.composerPanel = nil })
    }

    private func run(_ index: Int) {
        model.composerPanel = nil
        items[index].run()
    }
}

extension View {
    /// Arrow keys, ⏎ and Esc for a panel without a text field.
    func panelKeys(focused: FocusState<Bool>.Binding, count: Int, highlighted: Binding<Int>,
                   choose: @escaping () -> Void, close: @escaping () -> Void) -> some View {
        focusable()
            .focusEffectDisabled()
            .focused(focused)
            .onAppear { focused.wrappedValue = true }
            .onKeyPress(.downArrow) {
                highlighted.wrappedValue = (highlighted.wrappedValue + 1) % max(count, 1)
                return .handled
            }
            .onKeyPress(.upArrow) {
                highlighted.wrappedValue = (highlighted.wrappedValue - 1 + max(count, 1)) % max(count, 1)
                return .handled
            }
            .onKeyPress(.return) { choose(); return .handled }
            .onKeyPress(.escape) { close(); return .handled }
    }
}

/// Reasoning effort: a switch for thinking at all and a slider over the levels.
/// ←→ move the slider, Space toggles, ⏎ or Esc closes.
struct EffortPanel: View {
    @ObservedObject var model: DesktopModel
    @FocusState private var focused: Bool

    private var levels: [ModelChoice] { model.models.efforts.filter { $0.id != "default" } }
    private var isOn: Bool { (model.models.effort ?? "default") != "default" }
    private var index: Int { levels.firstIndex { $0.id == model.models.effort } ?? 0 }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Label("Reasoning", systemImage: "brain")
                    .font(.callout.weight(.semibold))
                Spacer()
                Toggle("", isOn: Binding(get: { isOn }, set: { on in
                    set(on ? (levels.first { $0.id == "medium" } ?? levels.last)?.id : "default")
                }))
                .toggleStyle(.switch)
                .controlSize(.small)
                .tint(.brandFill)
                .labelsHidden()
            }
            if isOn, levels.count > 1 {
                VStack(spacing: 6) {
                    Slider(value: Binding(get: { Double(index) }, set: { set(levels[Int($0.rounded())].id) }),
                           in: 0...Double(levels.count - 1), step: 1)
                        .tint(.brandFill)
                    HStack {
                        ForEach(Array(levels.enumerated()), id: \.offset) { position, level in
                            Text(level.name)
                                .font(.caption.weight(position == index ? .semibold : .regular))
                                .foregroundStyle(position == index ? Color.brand : .secondary)
                                .frame(maxWidth: .infinity, alignment: position == 0 ? .leading
                                       : position == levels.count - 1 ? .trailing : .center)
                                .onTapGesture { set(level.id) }
                        }
                    }
                }
                .transition(.opacity)
            } else if !isOn {
                Text("The model answers without extra reasoning.")
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
        .padding(8)
        .animation(.snappy(duration: 0.2), value: isOn)
        .focusable()
        .focusEffectDisabled()
        .focused($focused)
        .onAppear { focused = true }
        .onKeyPress(.leftArrow) { step(-1) }
        .onKeyPress(.rightArrow) { step(1) }
        .onKeyPress(.space) { set(isOn ? "default" : (levels.first { $0.id == "medium" } ?? levels.last)?.id); return .handled }
        .onKeyPress(.return) { model.composerPanel = nil; return .handled }
        .onKeyPress(.escape) { model.composerPanel = nil; return .handled }
    }

    private func step(_ delta: Int) -> KeyPress.Result {
        guard isOn, !levels.isEmpty else { return .ignored }
        set(levels[min(max(index + delta, 0), levels.count - 1)].id)
        return .handled
    }

    private func set(_ effort: String?) {
        guard let effort, effort != model.models.effort else { return }
        Task { await model.setEffort(effort) }
    }
}

/// The model list with a filter, used by the composer panel and in Settings.
/// Esc clears the filter, then closes.
struct ModelListPanel: View {
    @ObservedObject var model: DesktopModel
    var maxHeight: CGFloat = 340
    let close: () -> Void
    @State private var filter = ""
    @State private var highlighted = 0
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
                    .onSubmit { if choices.indices.contains(highlighted) { pick(choices[highlighted]) } }
                    .onKeyPress(.downArrow) {
                        if !choices.isEmpty { highlighted = (highlighted + 1) % choices.count }
                        return .handled
                    }
                    .onKeyPress(.upArrow) {
                        if !choices.isEmpty { highlighted = (highlighted - 1 + choices.count) % choices.count }
                        return .handled
                    }
                    .onKeyPress(.escape) {
                        if filter.isEmpty { close() } else { filter = "" }
                        return .handled
                    }
                    .onChange(of: filter) { _, _ in highlighted = 0 }
            }
            .padding(.horizontal, 12)
            .frame(height: 34)
            .background(Color.primary.opacity(0.06), in: .capsule)
            ScrollViewReader { proxy in
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    ForEach(Array(choices.enumerated()), id: \.element.id) { index, choice in
                        PopoverRow(choice.name, detail: choice.detail == choice.name ? nil : choice.detail,
                                   isSelected: choice.id == model.selectedModel,
                                   isHighlighted: index == highlighted) { pick(choice) }
                            .id(choice.id)
                    }
                    if choices.isEmpty {
                        Text("No matching models").font(.callout).foregroundStyle(.secondary).padding(10)
                    }
                }
            }
            .frame(maxHeight: maxHeight)
            .fixedSize(horizontal: false, vertical: true)
            .onChange(of: highlighted) { _, index in
                if choices.indices.contains(index) { proxy.scrollTo(choices[index].id) }
            }
            }
        }
        .onAppear {
            focused = true
            highlighted = choices.firstIndex { $0.id == model.selectedModel } ?? 0
        }
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
    /// The row the arrow keys are on.
    var isHighlighted = false
    let action: () -> Void
    @State private var isHovered = false

    init(_ title: String, systemImage: String? = nil, detail: String? = nil, isSelected: Bool = false,
         isHighlighted: Bool = false, action: @escaping () -> Void) {
        self.title = title
        self.systemImage = systemImage
        self.detail = detail
        self.isSelected = isSelected
        self.isHighlighted = isHighlighted
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
            .background(isHighlighted ? AnyShapeStyle(Color.brand.opacity(0.2))
                        : isHovered ? AnyShapeStyle(.fill.tertiary) : AnyShapeStyle(.clear), in: .rect(cornerRadius: 14))
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .onHover { isHovered = $0 }
    }
}
