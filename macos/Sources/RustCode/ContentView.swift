import AppKit
import RustCodeKit
import class SwiftTerm.TerminalView
import SwiftUI
import WebKit

/// The window has no title bar: a gray sidebar runs to the top edge beside the
/// chat, with no split-view divider between them.
struct ContentView: View {
    @ObservedObject var model: DesktopModel
    @AppStorage("previewWidth") private var previewWidth: Double = 480
    /// The sidebar width, remembered across launches. An out-of-range stored
    /// value is clamped on read, so a bad value never strands the sidebar.
    @AppStorage(SidebarResizeHandle.key) private var sidebarWidth: Double = SidebarResizeHandle.defaultWidth
    private var sidebarClampedWidth: CGFloat { SidebarResizeHandle.clamp(sidebarWidth) }

    var body: some View {
        HStack(spacing: 0) {
            // Kept alive while hidden and slid out of a closing frame:
            // built afresh on each show, its rows cost the slide's first
            // frames.
            SessionSidebar(model: model, updater: model.updater)
                .frame(width: sidebarClampedWidth)
                .frame(width: model.showsSidebar ? sidebarClampedWidth : 0, alignment: .trailing)
                .clipped()
                .allowsHitTesting(model.showsSidebar)
                .accessibilityHidden(!model.showsSidebar)
                // The line sits on the sidebar's edge, its grab area
                // either side of it; laid out beside the sidebar, the
                // handle's own width showed as a strip of sidebar past
                // the line (#2433). Above the chat, so the half over it
                // still takes the drag.
                .overlay(alignment: .trailing) {
                    if model.showsSidebar {
                        SidebarResizeHandle(width: $sidebarWidth)
                            .offset(x: SidebarResizeHandle.width / 2)
                    }
                }
                .zIndex(1)
            ChatView(model: model, chat: model.chat, find: model.find)
                .background(Color.chatBackground)
                // A subagent opens in the chat's place, the chat kept alive
                // under it so its scroll position is there on the way back
                // (#2492).
                .overlay {
                    if let agent = model.viewedAgent {
                        AgentPanel(model: model, agent: agent)
                            .background(Color.chatBackground)
                            .environment(\.closePanel) { model.viewedAgent = nil }
                            .transition(.opacity)
                    }
                }
                .animation(.snappy(duration: 0.2), value: model.viewedAgent?.id)
                .terminalDrawer(model.terminal, keybindings: model.keybindings)
            if model.showsPreview {
                HStack(spacing: 0) {
                    PreviewResizeHandle(width: $previewWidth)
                    PreviewPanel(preview: model.preview, keybindings: model.keybindings) { model.togglePreview() }
                        .frame(width: CGFloat(previewWidth))
                }
                .background(Color.chatBackground)
                .transition(.move(edge: .trailing))
            } else if model.showsChanges {
                HStack(spacing: 0) {
                    PreviewResizeHandle(width: $previewWidth)
                    ChangesPanel(model: model) { model.toggleChanges() }
                        .frame(width: CGFloat(previewWidth))
                }
                .background(Color.chatBackground)
                .transition(.move(edge: .trailing))
            } else if model.showsPullRequests {
                HStack(spacing: 0) {
                    PreviewResizeHandle(width: $previewWidth)
                    PullRequestsSidePanel(model: model) {
                        withAnimation(.snappy(duration: 0.25)) { model.showsPullRequests = false }
                    }
                    .frame(width: CGFloat(previewWidth))
                }
                .background(Color.chatBackground)
                .transition(.move(edge: .trailing))
            }
        }
        .background(Color.sidebarBackground)
        .ignoresSafeArea()
        .tint(.brand)
        // Settings is a page in the window's place, over the chats and the
        // chat, which stay alive under it (#2553). It opens and closes at
        // once, with no transition, even inside an animated change (#2593).
        .overlay {
            if model.showsSettings {
                SettingsView(model: model)
                    .environment(\.closePanel) { model.closeSettings() }
                    .transition(.identity)
            }
        }
        .transaction(value: model.showsSettings) { $0.disablesAnimations = true }
        .clickAwayPanel(item: $model.renamingSession) { RenameSheet(model: model, session: $0) }
        .clickAwayPanel(item: $model.contextSheet) { ContextSheet(context: $0, planUsage: model.planUsage) }
        .clickAwayPanel(item: $model.commandResult) { CommandResultSheet(result: $0) }
        .clickAwayPanel(isPresented: $model.showsRemote) { RemotePanel(model: model) }
        .clickAwayPanel(isPresented: $model.showsVisualizationGuide) { VisualizationGuideView(model: model) }
        .modifier(WhatsNewPresenter(whatsNew: model.whatsNew))
        .overlay { SessionSwitcherOverlay(model: model) }
        .overlay {
            if let image = model.viewedImage {
                ImageViewer(image: image) { model.viewedImage = nil }
                    .id(image.id)
                    .transition(.opacity)
            }
        }
        .animation(.snappy(duration: 0.2), value: model.viewedImage)
        .environment(\.openImage) { [model] in model.viewedImage = $0 }
        // Centered on the chat, not the window: the sidebar and the preview
        // beside it are left out.
        // A thread asks to message another one (#2637). Each choice answers
        // the request once; closing the alert any other way denies it.
        .alert(model.threadGrant.map { "Let “\($0.from)” message “\($0.to)”?" } ?? "",
               isPresented: Binding(get: { model.threadGrant != nil }, set: { shown in
                   guard !shown, let grant = model.threadGrant else { return }
                   model.threadGrant = nil
                   grant.resolve(.deny)
               }),
               presenting: model.threadGrant) { grant in
            Button("Allow for This Thread") { model.threadGrant = nil; grant.resolve(.always) }
            Button("Allow Once") { model.threadGrant = nil; grant.resolve(.once) }
            Button("Don’t Allow", role: .cancel) { model.threadGrant = nil; grant.resolve(.deny) }
        } message: { grant in
            Text("Its agent wants to send this as a message there, where it runs as a turn with that thread's own history and approvals:\n\n\(String(grant.message.prefix(600)))")
        }
        .overlay {
            ToastStack(model: model,
                       leadingInset: model.showsSidebar ? sidebarClampedWidth : 0,
                       trailingInset: model.showsPreview || model.showsChanges || model.showsPullRequests ? CGFloat(previewWidth) + PreviewResizeHandle.width : 0)
        }
    }
}

/// The sidebar's top bar, shared with Settings' sidebar so its Back button
/// sits where the chat sidebar's buttons do.
enum SidebarMetrics {
    /// The hidden title bar's height, so buttons line up with the traffic lights.
    static let barHeight: CGFloat = 28
    /// Room above the toolbar clusters so their glass does not touch the
    /// window's top edge.
    static let barTopInset: CGFloat = 6
    /// Where the bar's first cluster starts, clear of the traffic lights.
    static let barLeading: CGFloat = 80
}

/// The line between the sidebar and the chat, dragged to resize the sidebar.
/// The chosen width is stored (`sidebarWidth`), so it comes back next launch;
/// `clamp` keeps it in range, so an invalid stored value is safe.
struct SidebarResizeHandle: View {
    @Binding var width: Double
    @State private var startWidth: Double?

    /// The sidebar stays usable and the chat keeps enough room.
    static let range: ClosedRange<Double> = 200...460
    /// The line and the padding either side that can be grabbed.
    static let width: CGFloat = 7
    /// Where the chosen width is stored; Settings' sidebar reads it too.
    static let key = "sidebarWidth"
    static let defaultWidth = Double(SessionSidebar.width)

    static func clamp(_ width: Double) -> CGFloat {
        CGFloat(min(max(width, range.lowerBound), range.upperBound))
    }

    var body: some View {
        Rectangle()
            .fill(Color.primary.opacity(0.1))
            .frame(width: 1)
            .padding(.horizontal, (Self.width - 1) / 2)
            .contentShape(.rect)
            .onHover { inside in
                if inside { NSCursor.resizeLeftRight.push() } else { NSCursor.pop() }
            }
            .gesture(
                DragGesture(minimumDistance: 1, coordinateSpace: .global)
                    .onChanged { drag in
                        let start = startWidth ?? width
                        startWidth = start
                        // The handle is on the sidebar's trailing edge, so
                        // dragging right (positive translation) widens it.
                        width = min(max(start + drag.translation.width, Self.range.lowerBound), Self.range.upperBound)
                    }
                    .onEnded { _ in startWidth = nil }
            )
            .accessibilityHidden(true)
    }
}

/// Moves the window from the empty parts of a top bar, as the title bar it
/// lacks would, and acts on a double-click as the system setting says.
///
/// Not `isMovableByWindowBackground`: with it set, AppKit worked out the
/// window's draggable region again on every frame, asking each hosting view
/// whether it takes focus, which walked the whole chat. While a reply
/// streamed that was a third of the main thread, and scrolling lagged (#2260).
struct WindowDragArea: NSViewRepresentable {
    func makeNSView(context: Context) -> NSView { DragView() }

    func updateNSView(_ view: NSView, context: Context) {}

    private final class DragView: NSView {
        override func mouseDown(with event: NSEvent) {
            guard let window else { return }
            guard event.clickCount == 2 else { return window.performDrag(with: event) }
            switch UserDefaults.standard.string(forKey: "AppleActionOnDoubleClick") {
            case "Minimize": window.performMiniaturize(nil)
            case "None": break
            default: window.performZoom(nil)
            }
        }
    }
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
        .animation(.easeOut(duration: 0.12), value: isHovered)
        .help(help)
        .accessibilityLabel(help)
    }

    private static let opticalOffset: [String: CGFloat] = ["square.and.pencil": -1]
}

/// The sidebar's buttons, in one glass cluster: at the sidebar bar's
/// trailing end while it shows, beside the traffic lights in the chat's
/// title bar while it is hidden.
struct SidebarBarButtons: View {
    @ObservedObject var model: DesktopModel

    var body: some View {
        ToolbarCluster {
            ClusterButton(systemImage: "sidebar.left",
                          help: model.keybindings.help(model.showsSidebar ? "Hide sidebar" : "Show sidebar", .toggleSidebar)) {
                model.toggleSidebar()
            }
            AttentionInboxButton(model: model)
            if model.showsSidebar, model.showsArchived || !model.archivedSessions.isEmpty {
                ClusterButton(
                    systemImage: model.showsArchived ? "chevron.left" : "archivebox",
                    help: model.showsArchived ? "Back to threads" : "Archived threads",
                    isOn: model.showsArchived
                ) {
                    withAnimation(.snappy(duration: 0.2)) { model.showsArchived.toggle() }
                }
            }
            ClusterButton(systemImage: "square.and.pencil", help: model.keybindings.help("New thread", .newChat)) { Task { await model.newSession() } }
        }
    }
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
    /// The Done and Snoozed shelves the reader unfolded.
    @State private var openShelves: Set<SidebarGroups.Kind> = []
    @AppStorage(DesktopModel.groupsByAttentionKey) private var groupsByAttention = true

    static let barHeight = SidebarMetrics.barHeight
    /// How wide the sidebar is beside the chat.
    static let width: CGFloat = 270
    static let barTopInset = SidebarMetrics.barTopInset

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            // Room for the traffic lights, then the bar's own buttons.
            HStack(spacing: 2) {
                Spacer()
                SidebarBarButtons(model: model)
            }
            .padding(.leading, SidebarMetrics.barLeading)
            .padding(.trailing, 10)
            .frame(height: Self.barHeight)
            .padding(.top, Self.barTopInset)
            .background(WindowDragArea())

            HStack(spacing: 6) {
                Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
                TextField("Search threads", text: $model.sessionFilter)
                    .textFieldStyle(.plain)
                    .focused($searchFocused)
                    .onChange(of: model.focusSessionSearch) { _, focus in
                        if focus { searchFocused = true; model.focusSessionSearch = false }
                    }
                    // ⌘K's switcher and the composer's panels take the
                    // keyboard: typing must not keep filtering the sidebar
                    // underneath them.
                    .onChange(of: model.showsSwitcher || model.composerPanel != nil) { _, shown in
                        if shown { searchFocused = false }
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

            HStack(spacing: 6) {
                Text(model.showsArchived ? "Archived" : "Threads")
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(.secondary)
                Spacer()
                ScopeToggle(showsAll: $model.showsAllProjects)
            }
            .padding(.leading, 20)
            .padding(.trailing, 12)
            .padding(.top, 12)
            .padding(.bottom, 4)

            let groups = model.sidebarGroups(byAttention: groupsByAttention)
            ScrollView {
                // One list keyed by chat: as two lists, a chat pinned while
                // open moved into the pinned one and kept its selected fill
                // after another chat was opened.
                let byId = Dictionary(model.visibleSessions.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
                // A search shows what is on the shelves too.
                let open = model.sessionFilter.isEmpty ? openShelves : Set(SidebarGroups.Kind.allCases)
                let entries = SidebarEntry.entries(groups, sessions: byId, openShelves: open)
                // The row whose appearance reads the next page of chats.
                let prefetch = entries.indices.last.flatMap { last in
                    entries.indices.first { SessionPages.isNearEnd($0, of: last + 1) }.map { entries[$0].id }
                }
                LazyVStack(alignment: .leading, spacing: 2) {
                    ForEach(entries) { entry in
                        switch entry {
                        case .header(let kind, let count):
                            GroupHeader(kind: kind, count: count, isOpen: open.contains(kind)) {
                                withAnimation(.snappy(duration: 0.2)) {
                                    if openShelves.contains(kind) { openShelves.remove(kind) } else { openShelves.insert(kind) }
                                }
                            }
                        case .divider:
                            Divider().opacity(0.4).padding(.vertical, 6).padding(.horizontal, 10)
                        case .session(let session):
                            VStack(alignment: .leading, spacing: 2) {
                                row(session)
                                    .onAppear {
                                        if session.id == prefetch { Task { await model.loadMoreSessions() } }
                                    }
                                if session.id == model.selectedSession
                                    || !model.backgroundAgents.agents(of: session.id).isEmpty {
                                    SubagentChildRows(model: model, session: session)
                                }
                            }
                        }
                    }
                    if model.hasMoreSessions || model.moreSessionsFailed {
                        MoreSessionsRow(model: model)
                    }
                }
                .padding(.horizontal, 10)
            }
            // A pin (⌘P), a refresh or a filter moves rows under a still
            // pointer, and none of them hears it leave: forget the hover so
            // only the row the pointer next moves over lights up.
            .onChange(of: groups.ids) { _, _ in hoveredSession = nil }
            // Something new in a done or snoozed chat brings it back, and a
            // snooze ends on time, even after a restart (#2634, #2635).
            .onChange(of: model.runningSessions) { _, _ in model.settleLifecycle() }
            .onChange(of: model.permissionQueue.map(\.sessionId)) { _, _ in model.settleLifecycle() }
            .task(id: model.lifecycle) {
                while !Task.isCancelled {
                    let next = model.settleLifecycle()
                    let wait = min(next.map { $0.timeIntervalSinceNow } ?? 60, 60)
                    try? await Task.sleep(for: .seconds(max(wait, 1)))
                }
            }
            // A chat opened from the keyboard, a toast or a notification
            // leaves the pointer's row lit beside the new selection.
            .onChange(of: model.selectedSession) { _, _ in hoveredSession = nil }
            // Room under the last chat, so the project card and update pill
            // floating over the list's foot never cover it.
            .contentMargins(.bottom, updater.available == nil ? 72 : 116, for: .scrollContent)
            .overlay(alignment: .bottom) {
                VStack(spacing: 0) {
                    UpdatePill(updater: updater)
                    projectCard
                }
            }
            .overlay {
                if !model.sessionsLoaded {
                    ProgressView().controlSize(.small)
                } else if model.visibleSessions.isEmpty {
                    Text(model.showsArchived ? "No archived threads."
                         : !model.sessions.isEmpty ? "No matching threads."
                         : model.showsAllProjects ? "No threads yet." : "No threads in this project yet.")
                        .font(.callout)
                        .foregroundStyle(.secondary)
                }
            }
        }
    }

    /// The default project and Settings, on a glass card floating over the
    /// list's foot, so the chats scroll on under it.
    private var projectCard: some View {
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
                .padding(.vertical, 7)
                .contentShape(.rect(cornerRadius: 10))
            }
            .buttonStyle(ProjectButtonStyle())
            .pointingHandCursor()
            .help(model.keybindings.help("Choose the default project for new threads. Open threads keep their own folder", .chooseProject))
            ChromeButton(systemImage: "gearshape", help: model.keybindings.help("Settings", .settings)) { model.openSettings() }
        }
        .padding(4)
        .frame(maxWidth: .infinity, alignment: .leading)
        .floatingSurface(in: .rect(cornerRadius: 16))
        .overlay(RoundedRectangle(cornerRadius: 16).strokeBorder(.separator.opacity(0.6), lineWidth: 0.5))
        .padding(.horizontal, 10)
        .padding(.bottom, 10)
    }
}

/// A new version, floating over the foot of the chat list in glass, so it
/// neither takes a row of its own nor leaves a gap above the project bar.
private struct UpdatePill: View {
    @ObservedObject var updater: Updater

    var body: some View {
        if let release = updater.available {
            Button { Task { await updater.install() } } label: {
                HStack(spacing: 7) {
                    Image(systemName: "arrow.down.circle.fill").foregroundStyle(Color.brand)
                    Text(updater.canInstallInPlace ? "Update to \(release.version)" : "Download \(release.version)")
                        .font(.callout.weight(.medium))
                        .lineLimit(1)
                    if case .downloading(_, let progress) = updater.state {
                        ProgressView(value: progress).frame(width: 44)
                    } else if case .installing = updater.state {
                        ProgressView().controlSize(.small)
                    }
                }
                .foregroundStyle(.primary)
                .padding(.horizontal, 14)
                .frame(height: 32)
                .contentShape(.capsule)
            }
            .buttonStyle(QuietPressStyle())
            .interactiveGlassSurface(in: .capsule)
            .pointingHandCursor()
            .padding(.bottom, 12)
            .help("Install the new version and restart RustCode")
            .transition(.move(edge: .bottom).combined(with: .opacity))
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
                   // While one of its agents is read, the agent's row is
                   // the selected one, not both (#2596).
                   isSelected: SubagentInfo.chatRowSelected(session.id, selected: model.selectedSession,
                                                            viewingAgent: model.viewedAgent?.id),
                   needsAnswer: model.permissionQueue.contains { $0.sessionId == session.id },
                   waitingFor: model.permissionQueue.first { $0.sessionId == session.id }.map(SessionRow.waiting),
                   isRunning: model.runningSessions.contains(session.id),
                   hasUnseenReply: model.unseenReplies.contains(session.id),
                   searchSnippet: model.searchSnippet(for: session),
                   searchQuery: model.sessionFilter.trimmingCharacters(in: .whitespaces),
                   isHovered: hoveredSession == session.id,
                   onHover: { inside in
                       if inside { hoveredSession = session.id } else if hoveredSession == session.id { hoveredSession = nil }
                   }) {
            Task { await model.selectFromSidebar(session) }
        }
        }
        .contextMenu { SessionMenu(model: model, session: session) }
    }
}

private extension View {
    /// Start at the end of the content without pinning it there as it grows,
    /// so a reader scrolled up stays put; the chat's own follow logic does
    /// the rest. macOS 14 has only the anchor for every change, so it opens
    /// at the top and is scrolled down when shown.
    @ViewBuilder
    func opensAtBottom() -> some View {
        if #available(macOS 15, *) {
            defaultScrollAnchor(.bottom, for: .initialOffset)
        } else {
            self
        }
    }
}

/// The chat's text selection, which a click outside any text does not end
/// on its own: the selected text keeps the keyboard until something else
/// takes it.
enum TextSelection {
    /// Drop the selection for a click on the chat that lands outside any
    /// text (#2440): the blank space beside and between messages. Only when
    /// the keyboard is with selected text, not a field the reader types in.
    /// Returns whether the click was outside any text.
    @discardableResult
    @MainActor static func endIfOutsideText(_ event: NSEvent) -> Bool {
        guard let window = event.window, let content = window.contentView,
              let hit = content.hitTest(content.convert(event.locationInWindow, from: nil))
        else { return false }
        // Selectable text is a text field of its own; a click in it moves
        // the selection, and one in another field starts there.
        var view: NSView? = hit
        while let current = view {
            if current is NSTextField || current is NSTextView { return false }
            view = current.superview
        }
        if holdsSelection(window) { window.makeFirstResponder(nil) }
        // A run of prose redrawn with its highlight restored holds a
        // selection without the keyboard.
        ProseSelection.shared.clear()
        return true
    }

    /// Esc drops a selection in the chat, wherever the pointer is (#2440).
    /// False when the keyboard is anywhere else, so Esc keeps its meaning there.
    @MainActor static func endOnEscape(in window: NSWindow) -> Bool {
        guard holdsSelection(window) else { return false }
        ProseSelection.shared.clear()
        window.makeFirstResponder(nil)
        return true
    }

    /// The keyboard is with selectable text in the chat, not a field the
    /// reader types in.
    @MainActor private static func holdsSelection(_ window: NSWindow) -> Bool {
        if ProseSelection.shared.holds(window.firstResponder) { return true }
        guard let editor = window.firstResponder as? NSTextView, editor.isFieldEditor,
              let field = editor.delegate as? NSTextField else { return false }
        return !field.isEditable
    }
}

/// A drag that starts in the blank space beside the chat's text, such as the
/// margin left of a reply or beside a bubble, selects from the text's edge
/// on that line, as if it had started there (#2440). The press is moved to
/// the column's edge once the pointer moves, so the chat's own text
/// selection takes over; a click without a drag only ends the selection,
/// so controls in the chat keep their clicks.
@MainActor
final class GutterSelection {
    static let shared = GutterSelection()

    /// Where a press outside any text went down, in window coordinates.
    private var start: (window: NSWindow, point: NSPoint, chat: CGRect)?
    /// The press was moved into the column; the drag now belongs to it.
    private var moved = false
    /// How far the pointer must move before a press becomes a selection.
    static let threshold: CGFloat = 4

    func begin(_ event: NSEvent, in chat: CGRect) {
        start = nil
        moved = false
        guard let window = event.window else { return }
        // Not on the timeline rail at the left, when the window is wide
        // enough to show it, or the scroller at the right: dragging those
        // moves through the chat.
        let x = event.locationInWindow.x
        let left = chat.minX + (ChatColumnLayout(width: chat.width).rail.map { $0.x + $0.width + 3 } ?? 0)
        guard x > left, x < chat.maxX - 24 else { return }
        start = (window, event.locationInWindow, chat)
    }

    /// Returns whether the event was used up, so it is not delivered.
    func drag(_ event: NSEvent) -> Bool {
        guard let start, !moved, event.window === start.window else { return false }
        let point = event.locationInWindow
        guard hypot(point.x - start.point.x, point.y - start.point.y) >= Self.threshold else { return false }
        moved = true
        let column = Self.column(in: start.chat)
        // Only a press beside the column moves; one inside it, between
        // lines, has no text to start at.
        let x = min(max(start.point.x, column.lowerBound + 1), column.upperBound - 1)
        guard x != start.point.x else {
            self.start = nil
            return false
        }
        let press = NSPoint(x: x, y: start.point.y)
        for (type, location) in [(NSEvent.EventType.leftMouseDown, press), (.leftMouseDragged, point)] {
            if let synthetic = NSEvent.mouseEvent(with: type, location: location, modifierFlags: event.modifierFlags,
                                                  timestamp: event.timestamp, windowNumber: start.window.windowNumber,
                                                  context: nil, eventNumber: 0, clickCount: 1, pressure: 1) {
                start.window.sendEvent(synthetic)
            }
        }
        return true
    }

    func end() {
        start = nil
        moved = false
    }

    /// The chat column's horizontal span in window coordinates, as
    /// `ChatView` lays it out: centred, at most its maximum width, with its
    /// padding either side.
    static func column(in chat: CGRect) -> ClosedRange<CGFloat> {
        let width = CGFloat(ChatColumnLayout(width: chat.width).column)
        return (chat.midX - width / 2)...(chat.midX + width / 2)
    }
}

/// The sidebar's project folder: a quiet filled control at rest, so it reads
/// as pressable, a little stronger under the pointer and while pressed.
private struct ProjectButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        Styled(configuration: configuration)
    }

    private struct Styled: View {
        let configuration: Configuration
        @State private var isHovered = false

        var body: some View {
            configuration.label
                .background(fill, in: .rect(cornerRadius: 10))
                .onHover { isHovered = $0 }
                .animation(.easeOut(duration: 0.12), value: isHovered)
        }

        private var fill: Color {
            Color.primary.opacity(configuration.isPressed ? 0.12 : isHovered ? 0.08 : 0)
        }
    }
}

/// A line of the sidebar's chat list: the pinned chats under their header
/// and a divider, then the rest.
enum SidebarEntry: Identifiable {
    /// A group's heading; only shelves, the attention groups and Pinned have one.
    case header(SidebarGroups.Kind, count: Int)
    case divider(SidebarGroups.Kind)
    case session(SessionSummary)

    var id: String {
        switch self {
        case .header(let kind, _): "\u{0}header-\(kind.rawValue)"
        case .divider(let kind): "\u{0}divider-\(kind.rawValue)"
        case .session(let session): session.id
        }
    }

    /// The rows for `groups`: a heading over each group but Chats, a divider
    /// between groups, and a folded shelf as just its heading.
    static func entries(_ groups: SidebarGroups, sessions: [String: SessionSummary],
                        openShelves: Set<SidebarGroups.Kind>) -> [SidebarEntry] {
        var entries: [SidebarEntry] = []
        for (index, group) in groups.groups.enumerated() {
            if index > 0 { entries.append(.divider(group.kind)) }
            if group.kind != .chats { entries.append(.header(group.kind, count: group.ids.count)) }
            if group.kind.isShelf, !openShelves.contains(group.kind) { continue }
            entries += group.ids.compactMap { sessions[$0] }.map(SidebarEntry.session)
        }
        return entries
    }
}

/// A group's heading: what it holds and why, and for a shelf a fold button
/// with its count.
private struct GroupHeader: View {
    let kind: SidebarGroups.Kind
    let count: Int
    let isOpen: Bool
    let toggle: () -> Void

    var body: some View {
        if kind.isShelf {
            Button(action: toggle) {
                HStack(spacing: 6) {
                    Label(kind.title, systemImage: kind.systemImage)
                    Text("\(count)").monospacedDigit().foregroundStyle(.tertiary)
                    Spacer(minLength: 0)
                    Image(systemName: "chevron.right")
                        .font(.caption2.weight(.semibold))
                        .rotationEffect(.degrees(isOpen ? 90 : 0))
                }
                .contentShape(.rect)
            }
            .buttonStyle(.plain)
            .font(.caption.weight(.semibold))
            .foregroundStyle(.secondary)
            .padding(.horizontal, 10)
            .padding(.top, 2)
            .accessibilityLabel("\(kind.title), \(count) thread\(count == 1 ? "" : "s")")
            .accessibilityValue(isOpen ? "Shown" : "Hidden")
            .accessibilityHint(isOpen ? "Hides them" : "Shows them")
        } else {
            Label(kind.title, systemImage: kind.systemImage)
                .font(.caption.weight(.semibold))
                .foregroundStyle(kind == .needsYou ? AnyShapeStyle(Color.orange) : AnyShapeStyle(.secondary))
                .padding(.horizontal, 10)
                .padding(.top, 2)
                .accessibilityAddTraits(.isHeader)
        }
    }
}

/// The end of the chats read so far: a spinner while the next page loads,
/// or a retry when it failed. While it is in view it keeps reading pages, so
/// a short list, such as a search or the archive, fills from the whole history.
private struct MoreSessionsRow: View {
    @ObservedObject var model: DesktopModel

    var body: some View {
        HStack(spacing: 8) {
            if model.moreSessionsFailed {
                Text("Could not load more threads.").foregroundStyle(.secondary)
                Button("Retry") { Task { await model.retryMoreSessions() } }
                    .buttonStyle(PillButtonStyle())
            } else {
                ProgressView().controlSize(.small)
                Text("Loading more threads…").foregroundStyle(.secondary)
            }
        }
        .font(.caption)
        .frame(maxWidth: .infinity)
        .padding(.vertical, 8)
        .task(id: model.sessions.count) { await model.loadMoreSessions() }
        .accessibilityElement(children: .combine)
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
        if !model.archivedSessions.contains(session.id) {
            Divider()
            // Done and Snooze move the row only (#2634, #2635); a chat that
            // waits for an answer or is working cannot be put away.
            let busy = SidebarLifecycle.refusal(model.attention(of: session.id)) != nil
            if model.lifecycle.isDone(session.id) {
                Button("Reopen", systemImage: "arrow.uturn.backward.circle") { model.reopen(session) }
            } else {
                Button("Mark as Done", systemImage: "checkmark.circle") { model.markDone(session) }.disabled(busy)
            }
            if model.lifecycle.isSnoozed(session.id, now: Date()) {
                Button("Wake Now", systemImage: "sun.max") { model.wakeNow(session) }
            } else {
                Menu("Snooze", systemImage: "moon.zzz") {
                    ForEach(SnoozeChoice.allCases) { choice in
                        Button(choice.title) { model.snooze(session, until: choice.wake(from: Date())) }
                    }
                }
                .disabled(busy)
            }
        }
        Divider()
        Button("Copy Title", systemImage: "doc.on.doc") { Clipboard.copy(session.title ?? "New thread") }
        Button("Copy Thread ID", systemImage: "number") { Clipboard.copy(session.id) }
        Button("Show in Finder", systemImage: "folder") { Task { await model.revealInFinder(session) } }
        if !session.cwd.isEmpty {
            Button("Open Project in Finder", systemImage: "folder.badge.gearshape") {
                NSWorkspace.shared.open(URL(fileURLWithPath: session.cwd, isDirectory: true))
            }
            Button("Search Threads in \(session.projectName)", systemImage: "line.3.horizontal.decrease") {
                model.sessionFilter = session.projectName
            }
            Button("New Thread in \(session.projectName)", systemImage: "square.and.pencil") {
                Task { await model.newSession(in: URL(fileURLWithPath: session.cwd, isDirectory: true)) }
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
    @Environment(\.closePanel) private var dismiss

    init(model: DesktopModel, session: SessionSummary) {
        self.model = model
        self.session = session
        _title = State(initialValue: session.title ?? "")
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Rename Thread").font(.headline)
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
                Button("Rename", action: save)
                    .buttonStyle(PillButtonStyle(prominent: true))
                    .keyboardShortcut(.defaultAction)
                    .disabled(title.trimmingCharacters(in: .whitespaces).isEmpty)
            }
        }
        .padding(22)
        .frame(width: 420)
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
struct ScopeToggle: View {
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
        .accessibilityLabel("Threads")
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

struct SessionRow: View {
    let session: SessionSummary
    var showsProject = false
    let isSelected: Bool
    let needsAnswer: Bool
    /// What the chat waits for, said in the row so the sidebar tells which
    /// approval or question it is without opening the chat.
    var waitingFor: String?
    let isRunning: Bool
    /// A turn finished while another chat was on screen.
    var hasUnseenReply = false
    /// Set for a chat found by its contents: the text around the match.
    var searchSnippet: String?
    var searchQuery = ""
    var isHovered = false
    var onHover: (Bool) -> Void = { _ in }
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 6) {
                VStack(alignment: .leading, spacing: 2) {
                    // The state sits at the title's trailing end, so every
                    // title starts on the same edge with nothing reserved
                    // before it, and its metadata never moves (#2412).
                    HStack(spacing: 6) {
                        Text(session.title ?? "New thread")
                            .lineLimit(1)
                        Spacer(minLength: 0)
                        indicator
                    }
                    if searchSnippet == nil, let waitingFor {
                        Text(waitingFor)
                            .font(.caption.weight(.medium))
                            .foregroundStyle(Color.attention)
                            .lineLimit(1)
                            .truncationMode(.tail)
                    } else if let searchSnippet {
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
                                // Redrawn each minute: formatted once, a chat
                                // read "now" for as long as its row stood still.
                                TimelineView(.periodic(from: .now, by: 30)) { context in
                                    Text(SessionRow.age(of: updated, now: context.date))
                                        .lineLimit(1)
                                }
                                .layoutPriority(1)
                            }
                        }
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    }
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
        // A context menu never reports the pointer leaving; the next move
        // over a row lights that one instead.
        .onReceive(NotificationCenter.default.publisher(for: NSMenu.didEndTrackingNotification)) { _ in
            onHover(false)
        }
    }

    /// The chat's state, when it has one, in a slot as tall as the title
    /// line so the row's height does not change as it comes and goes.
    @ViewBuilder
    private var indicator: some View {
        if needsAnswer || isRunning || hasUnseenReply {
            ZStack {
                if needsAnswer {
                    Image(systemName: "questionmark.bubble.fill")
                        .font(.caption)
                        .foregroundStyle(Color.attention)
                        .help("Waiting for your answer")
                } else if isRunning {
                    RowSpinner()
                } else {
                    Circle()
                        .fill(.blue)
                        .frame(width: 7, height: 7)
                        .help("New reply")
                        .accessibilityLabel("New reply")
                }
            }
            .frame(width: 16, height: 14)
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

    /// "Approve: Edit calc.py", or the question a chat asks.
    static func waiting(_ request: PermissionRequest) -> String {
        if let question = request.question { return "Asks: " + question.text }
        return "Approve: " + request.preview.headline
    }

    /// "now" for the last minute, then "5 min. ago", "3 hours ago"…
    static func age(of date: Date, now: Date) -> String {
        guard now.timeIntervalSince(date) >= 60 else { return "now" }
        return date.formatted(.relative(presentation: .named, unitsStyle: .abbreviated))
    }

    /// Selection is the one strong fill; hover is a faint wash so a hovered
    /// row never reads as a second selection.
    private var rowFill: Color {
        isSelected ? Color.primary.opacity(0.1) : isHovered ? Color.primary.opacity(0.035) : .clear
    }
}

/// A chat's running turn in the sidebar. Drawn by SwiftUI rather than an
/// AppKit progress indicator, which a lazy list could leave spinning on a row
/// after it was taken away.
private struct RowSpinner: View {
    var body: some View {
        TimelineView(.animation) { context in
            let turn = context.date.timeIntervalSinceReferenceDate.truncatingRemainder(dividingBy: 1)
            Circle()
                .trim(from: 0, to: 0.7)
                .stroke(.secondary, style: StrokeStyle(lineWidth: 1.75, lineCap: .round))
                .rotationEffect(.degrees(turn * 360))
        }
        .frame(width: 12, height: 12)
        .padding(2)
        .accessibilityLabel("Working")
    }
}

private struct ChatView: View {
    /// How wide the chat column (transcript, composer, panels) may grow. Wide
    /// windows used to cap the conversation at 760pt and leave the rest empty
    /// (#2410); a larger cap keeps line length comfortable while using more of
    /// the window. Narrow windows are constrained by the width they have.
    static let columnMaxWidth = CGFloat(ChatColumnLayout.maxColumn)

    @ObservedObject var model: DesktopModel
    /// The transcript publishes on its own, so streaming redraws the chat
    /// but not the sidebar or the composer.
    @ObservedObject var chat: LiveTranscript
    /// Publishes once the matches for a query arrive, not per keystroke.
    @ObservedObject var find: ChatFind
    @State private var composerHeight: CGFloat = 60
    /// The chat's width, which picks its `ChatColumnLayout`.
    @State private var chatWidth: CGFloat = 1000
    @State private var chatHeight: CGFloat = 800
    /// A new thread's composer sits in the middle of the chat under its
    /// banner, and moves down to its place when the first message is sent.
    @State private var centersComposer = false
    /// The banner and composer together, measured while centered.
    @State private var centeredHeight: CGFloat = 0
    /// Where this thread's work mostly runs, when that is not its folder.
    @State private var suggestedFolder: URL?
    @AppStorage(NewThreadBanner.key) private var newThreadBanner = ""
    @AppStorage(NewThreadBanner.styleKey) private var newThreadBannerStyle = "ascii"
    /// The chat keeps the newest output in view. Scrolling up hands control to
    /// the reader and shows the Latest button; scrolling back down follows again.
    @StateObject private var following = FollowState()
    private var follow: ChatFollow {
        get { following.value }
        nonmutating set { following.value = newValue }
    }
    /// Real row heights for the chat's own scroller. Held, not observed:
    /// measuring redraws only the scroller.
    @State private var scroll = ChatScrollState()
    /// Which rows came in while the chat was open, so only those animate in.
    @State private var arrivals = RowArrivals()
    /// How much of a long chat is laid out: its newest items, and older ones
    /// as the reader scrolls back (`TranscriptWindow`).
    @State private var window = TranscriptWindow()
    /// The row in view when older rows were asked for, and where it was.
    @State private var pendingTop: (row: Int, minY: Double)?
    /// Opens or folds a long message where it is (#2303).
    @State private var messageToggle = MessageToggle()
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private var selected: SessionSummary? { model.sessions.first { $0.id == model.selectedSession } }

    private var title: String { model.selectedSession == nil ? "New Thread" : selected?.title ?? "RustCode" }

    /// The title's natural width, so a short title is not padded out to the
    /// header's minimum for it.
    private var titleWidth: CGFloat {
        let font = NSFont.preferredFont(forTextStyle: .headline)
        return ceil((title as NSString).size(withAttributes: [.font: font]).width) + 2
    }

    /// The folder this chat works in: its own, which may not be the default.
    private var projectName: String { model.chatProject.lastPathComponent }

    @AppStorage(ToolVerbosity.key) private var verbosity = ToolVerbosity.terminalDefault

    /// The rows the chat draws once the verbosity folds its tool calls: the
    /// lazy older ones, the eager newer ones, and the ids the scroller maps
    /// (the earlier-messages row, the shown items and the working row).
    private struct ShownRows {
        var folding: ToolFolding
        var older: [TranscriptItem]
        var newer: [TranscriptItem]
        var scroll: [Int]
        /// The agents each group's first spawn row draws (#2327).
        var agents: [Int: [AgentSpawn]]
    }

    /// Why the shown chat's last turn stopped, when it can be continued.
    private var stoppedReason: String? {
        model.selectedSession.flatMap { model.stoppedTurns[$0] }
    }

    /// What `shownRows` was last asked, and its answer.
    private struct ShownRowsKey: Equatable {
        var identity: UUID
        var revision: Int
        var first: Int
        var split: Int
        var verbosity: ToolVerbosity
        var open: Set<Int>
        var running: Bool
        var stopped: Bool
        var subagents: [SubagentInfo]
    }

    private final class ShownRowsCache {
        var key: ShownRowsKey?
        var rows: ShownRows?
    }

    /// Kept across passes: a resize or the sidebar sliding redraws the chat
    /// every frame, and folding a long thread's tool calls each time
    /// dropped frames.
    @State private var shownRowsCache = ShownRowsCache()

    private func cachedShownRows(_ transcript: Transcript, first: Int, split: Int) -> ShownRows {
        let key = ShownRowsKey(identity: transcript.identity, revision: transcript.revision, first: first, split: split,
                               verbosity: verbosity, open: model.openToolRuns, running: isReplyStreaming,
                               stopped: stoppedReason != nil, subagents: model.subagents)
        if key == shownRowsCache.key, let rows = shownRowsCache.rows { return rows }
        let rows = shownRows(transcript, first: first, split: split)
        shownRowsCache.key = key
        shownRowsCache.rows = rows
        return rows
    }

    private func shownRows(_ transcript: Transcript, first: Int, split: Int) -> ShownRows {
        let folding = ToolFolding(items: transcript.items, verbosity: verbosity,
                                  open: model.openToolRuns, running: isReplyStreaming)
        let shown = { (item: TranscriptItem) in !folding.hidden.contains(item.id) }
        let older = transcript.items[first..<split].filter(shown)
        let newer = transcript.items[split...].filter(shown)
        var scroll: [Int] = first > 0 ? [Self.earlierRow] : []
        scroll += (older + newer).map(\.id)
        if isReplyStreaming || stoppedReason != nil { scroll.append(Self.workingRow) }
        var agents: [Int: [AgentSpawn]] = [:]
        if !folding.agentGroups.isEmpty {
            var spawns: [Int: ToolItem] = [:]
            for item in transcript.items {
                if case .tool(let tool) = item.content, tool.isAgentSpawn { spawns[item.id] = tool }
            }
            agents = folding.agentGroups.mapValues { ids in
                ids.compactMap { spawns[$0] }.map { AgentSpawn(tool: $0, agents: model.subagents) }
            }
        }
        return ShownRows(folding: folding, older: older, newer: newer, scroll: scroll, agents: agents)
    }

    var body: some View {
        let transcript = chat.transcript
        let currentMatch = model.showsFind ? find.currentMatch : nil
        let lastId = transcript.items.last?.id
        let first = window.firstShown(identity: transcript.identity, count: transcript.items.count)

        // Older rows are drawn in full only near the view (`OlderRows`): laid
        // out in full, every resize re-measured the whole chat (170 ms a step
        // in a 250-message session). The newest rows are always drawn in full,
        // so the part being read never changes height under the reader while
        // scrolling near the end.
        let split = ChatLayout.split(count: transcript.items.count, firstShown: first)
        let rows = cachedShownRows(transcript, first: first, split: split)
        let expanded = model.expandedInChat
        let openTurns = Self.openTurns(rows)
        ScrollViewReader { proxy in
            ScrollView {
                // The newest rows and the end of the chat sit below the lazy
                // stack, not inside it (#2209, #2212). The lazy stack sizes
                // itself from guessed heights for rows it has not laid out,
                // and one very tall row (a long reply, a wide code block)
                // threw the guess off by thousands of points: the chat's end
                // was scrolled into empty space past its real rows, and the
                // stack laid out nothing there, so the chat stayed blank
                // while the reply streamed and after a message was sent. Out
                // here the end is always laid out at its real height.
                VStack(alignment: .leading, spacing: 18) {
                    // Home's target: always laid out, so the jump finds it
                    // however far down the chat is.
                    Color.clear.frame(height: 0).id(topAnchor)
                    if first > 0 || split > first {
                        OlderRows(items: rows.older, earlier: first, earlierRow: Self.earlierRow,
                                  folding: rows.folding, agents: rows.agents, expanded: expanded,
                                  currentMatch: currentMatch,
                                  scroll: scroll, drawn: scroll.drawn)
                    }
                    VStack(alignment: .leading, spacing: 18) {
                        ForEach(rows.newer) { item in
                            TranscriptRow(item: item, isCurrentMatch: item.id == currentMatch,
                                          isStreaming: isReplyStreaming && item.id == lastId,
                                          fold: rows.folding.fold(for: item.id), turnFold: rows.folding.turn(for: item.id),
                                          isNested: rows.folding.isNested(item.id), isExpanded: expanded.contains(item.id),
                                          agents: rows.agents[item.id], completion: rows.folding.completion(for: item.id))
                                .equatable()
                                .transition(.rowReveal)
                                .arrives(arrivals.isNew(item.id, in: transcript.identity, last: lastId),
                                         from: item.isUser ? .trailing : .leading)
                                .reportsChatRow(item.id, to: scroll)
                                .id(item.id)
                        }
                        // Up for the whole turn, also while a reply streams or the
                        // model plans its next step, so a live turn always says so.
                        if isReplyStreaming {
                            WorkingIndicator(startedAt: chat.transcript.turnStartedAt, activity: rows.folding.activity)
                                .transition(.opacity.combined(with: .offset(y: 6)))
                                .reportsChatRow(Self.workingRow, to: scroll)
                        } else if let stoppedReason {
                            StoppedTurnRow(reason: stoppedReason) {
                                Task { await model.continueStoppedTurn() }
                            }
                            .reportsChatRow(Self.workingRow, to: scroll)
                        }
                    }
                    .padding(.bottom, 2)
                    // The very end of the chat, below its padding (with the stack
                    // spacing, 20 points). Outside the lazy stack, so it is
                    // always laid out and a jump from far up finds it.
                    Color.clear.frame(height: 1).id(bottomAnchor)
                        // The probe for the AppKit scroll view sits under this
                        // last, empty row. Behind the whole stack, every row
                        // drawn over it went into one layer as tall as the
                        // chat, and a layer much taller than the window draws
                        // nothing: the chat showed blank until scrolled.
                        .background(EnclosingScrollView { [scroll] in scroll.scrollView = $0 })
                }
                #if DEBUG
                .coordinateSpace(.named(ChatScrollState.contentSpace))
                #endif
                .frame(maxWidth: Self.columnMaxWidth)
                .padding(.horizontal, layout.padding)
                .padding(.top, 20)
                .frame(maxWidth: .infinity)
            }
            // A new scroll view for each chat, laid out from its end: the
            // lazy stack keeps row heights by id, and every chat's ids start at 0.
            .opensAtBottom()
            .id(transcript.identity)
            .chatScroller(scroll) { delta, animated in
                // Paging or dragging up is the reader leaving the end.
                if delta < 0 { follow.letGo() }
                scroll.scroll(by: delta, animated: animated)
            }
            // In the gutter left of the column; only it observes the
            // markers, so scrolling redraws the rail and not the chat.
            .overlay(alignment: .topLeading) {
                TimelineRail(state: scroll.rail) { id in jumpToTurn(id, proxy) }
            }
            // An opened turn's header, kept in reach once it scrolls away
            // (#2462). Over the chat, like Latest, not pinned inside it.
            .overlay(alignment: .top) {
                StickyTurnBar(state: scroll.stickyTurn) { foldTurn($0, proxy) }
                    .frame(maxWidth: Self.columnMaxWidth)
                    .padding(.horizontal, layout.padding)
            }
            .onChange(of: openTurns, initial: true) { _, turns in scroll.setOpenTurns(turns) }
            // Over the chat rather than a pinned footer inside it: a pinned
            // view re-lays out its whole section on every scrolled frame.
            .overlay(alignment: .bottom) {
                let showsLatest = follow.showsLatest && !transcript.isEmpty
                ZStack {
                    if showsLatest {
                        HStack {
                            Spacer(minLength: 0)
                            LatestButton { jumpToLatest(proxy) }
                        }
                        .frame(maxWidth: Self.columnMaxWidth)
                        .padding(.horizontal, layout.padding)
                        .padding(.bottom, 12)
                        .transition(.opacity.combined(with: .scale(scale: 0.9, anchor: .bottomTrailing)))
                    }
                }
                // Animates the button only. On the scroll view it also animated
                // the scroll anchor and the jump itself, landing short of the end.
                .animation(.snappy(duration: 0.2), value: showsLatest)
            }
            .overlay {
                if model.isLoadingSelected {
                    ProgressView("Loading thread…").controlSize(.small).foregroundStyle(.secondary)
                } else if transcript.isEmpty, !model.busy, !centersComposer {
                    emptyChat.padding(.bottom, 80)
                }
            }
            .followsLatest(Binding { following.value } set: { following.value = $0 }, scroll: scroll) { scrollToEnd(proxy) }
            .onAppear {
                scroll.earlierRow = Self.earlierRow
                scroll.nearTop = { showEarlier(proxy) }
                #if DEBUG
                scroll.debugLetGo = { follow.letGo() }
                scroll.debugFollows = { follow.follows }
                scroll.debugToggleMessage = { [messageToggle] in messageToggle.action($0) }
                scroll.debugJumpToTurn = { jumpToTurn($0, proxy) }
                #endif
                messageToggle.action = { [model, scroll] id in
                    // The message keeps its place rather than the chat its end.
                    follow.letGo()
                    scroll.holdToggled(id, folding: model.expandedInChat.contains(id))
                    model.toggleMessage(id)
                }
                TranscriptKeyInput.shared.start()
                TranscriptKeyInput.shared.isCovered = { [model] in model.coversChat }
                ChatScrollInput.shared.isReplaced = { [model] in model.replacesChat }
                TranscriptKeyInput.shared.perform = { action in perform(action, proxy) }
            }
            .onGeometryChange(for: [CGRect].self) { proxy in
                // Clicks: only the part of the chat between the title bar and
                // the composer. Wheels: also the band beside and below the
                // composer, which the scroll view reaches under and scrolls
                // (#2554); the composer itself is left out where it is drawn.
                let frame = proxy.frame(in: .global)
                let insets = proxy.safeAreaInsets
                return [CGRect(x: frame.minX, y: frame.minY + insets.top, width: frame.width,
                               height: max(frame.height - insets.top - insets.bottom, 0)),
                        CGRect(x: frame.minX, y: frame.minY + insets.top, width: frame.width,
                               height: max(frame.height - insets.top, 0))]
            } action: { frames in
                ChatScrollInput.shared.chatFrame = frames[0]
                ChatScrollInput.shared.wheelFrame = frames[1]
            }
            .onChange(of: first) { _, _ in restoreTop(proxy) }
            .onChange(of: transcript.identity, initial: true) { _, identity in
                scroll.chatChanged()
                window.open(identity: identity, count: chat.transcript.items.count)
            }
            .onChange(of: rows.scroll, initial: true) { _, rows in
                scroll.setRows(rows)
                scroll.setTurns(TimelineTurn.turns(in: chat.transcript.items))
            }
            .onChange(of: following.follows, initial: true) { _, follows in scroll.anchoredToTop = !follows }
            // Back from an agent or Settings: the rows in view are drawn
            // now, not at the reader's next scroll (#2598).
            .onChange(of: model.replacesChat) { _, replaced in
                if !replaced { scroll.uncovered() }
            }
            .onChange(of: chat.transcript) { old, new in
                find.transcriptChanged(new)
                // A reply watched while it was written stays whole once done,
                // rather than folding under the reader.
                if isReplyStreaming, let last = new.items.last, case .assistant = last.content {
                    model.keepMessageOpen(last.id)
                }
                // Sending a message always brings the chat back to its end.
                if new.items.count > old.items.count, case .user = new.items.last?.content { follow.jumpToLatest() }
                if follow.transcriptChanged(readerScrolling: ChatScrollInput.shared.isActive), !model.showsFind {
                    scrollToEnd(proxy)
                }
            }
            .onChange(of: isReplyStreaming) { _, _ in
                if follow.follows, !model.showsFind { scrollToEnd(proxy) }
            }
            .onChange(of: model.selectedSession) { _, _ in
                find.transcriptChanged(chat.transcript, sessionChanged: true)
                follow.jumpToLatest()
                scrollToEnd(proxy)
            }
            // A plan step asks for the row where the agent last updated it.
            .onChange(of: model.revealRequest) { _, target in
                guard let target else { return }
                model.revealRequest = nil
                unfold(to: target)
                model.keepMessageOpen(target)
                follow.letGo()
                scroll.willJump()
                let items = chat.transcript.items
                if let index = items.firstIndex(where: { $0.id == target }),
                   window.reveal(index, identity: chat.transcript.identity, count: items.count) {
                    DispatchQueue.main.async { proxy.scrollTo(target, anchor: .center) }
                } else {
                    withAnimation(reduceMotion ? nil : .snappy(duration: 0.25)) { proxy.scrollTo(target, anchor: .center) }
                }
            }
            // The plan is read again when the thread changes and after each
            // update the agent makes to it.
            .task(id: PlanReadKey(session: model.selectedSession,
                                  updates: TaskPlan.finishedUpdates(in: chat.transcript.items),
                                  running: model.isTurnRunning)) {
                await model.refreshPlan()
            }
            .onChange(of: currentMatch) { _, match in
                guard let match else { return }
                // A match in a folded message opens it, and it stays open.
                model.keepMessageOpen(match)
                follow.letGo()
                scroll.willJump()
                // A match older than the window joins it first, then is
                // scrolled to once its row exists.
                let items = chat.transcript.items
                if let index = items.firstIndex(where: { $0.id == match }),
                   window.reveal(index, identity: chat.transcript.identity, count: items.count) {
                    DispatchQueue.main.async { proxy.scrollTo(match, anchor: .center) }
                } else {
                    withAnimation(.snappy(duration: 0.25)) { proxy.scrollTo(match, anchor: .center) }
                }
            }
        }
        .environment(\.findQuery, model.showsFind ? find.appliedQuery : "")
        .environment(\.projectRoot, model.chatProject.path)
        .environment(\.editsOpenByDefault, verbosity.editsOpen)
        .environment(\.toggleToolRun, { [model] run in withAnimation(.rowReveal) { model.toggleToolRun(run) } })
        .environment(\.openAgent, { [model] id in model.openAgent(id: id) })
        .environment(\.revealItem, { [model] id in model.revealRequest = id })
        .environment(\.toggleMessage, messageToggle)
        .environment(\.messageActions, model.messageActions)
        .safeAreaInset(edge: .top, spacing: 0) {
            VStack(spacing: 0) {
                header
                if model.showsFind {
                    FindBar(model: model, find: find)
                        .transition(.move(edge: .top).combined(with: .opacity))
                }
            }
            .animation(.snappy(duration: 0.2), value: model.showsFind)
        }
        .safeAreaInset(edge: .bottom, spacing: 0) {
            VStack(spacing: 10) {
                if centersComposer {
                    emptyChat
                        .padding(.bottom, 28)
                        .transition(.opacity.combined(with: .offset(y: -30)))
                }
                if let error = model.error, !error.isEmpty { ErrorBanner(text: error) { model.error = nil } }
                if let folder = suggestedFolder, let session = model.selectedSession,
                   !model.dismissedFolderSuggestions.contains("\(session)|\(folder.path)") {
                    FolderSuggestionCard(folder: folder, current: model.chatProject, canMove: !model.isTurnRunning) {
                        Task { await model.moveChatProject(to: folder) }
                    } dismiss: {
                        model.dismissFolderSuggestion(folder, for: session)
                    }
                    .transition(.opacity.combined(with: .move(edge: .bottom)))
                }
                if let permission = model.pendingPermission {
                    if permission.question != nil {
                        QuestionCard(model: model, request: permission).id(permission.id)
                    } else {
                        PermissionCard(model: model, request: permission).id(permission.id)
                    }
                }
                VStack(spacing: -Self.trayTuck) {
                    Composer(model: model, composer: model.composer)
                        .zIndex(1)
                    // A tray tucked under the composer, a little narrower,
                    // so the footer reads as the composer's own; its top
                    // runs under the composer, so the row is centered in
                    // what shows.
                    ComposerFooter(model: model)
                        .padding(.top, Self.trayTuck + 5)
                        .padding(.bottom, 5)
                        .background(Color.primary.opacity(0.06),
                                    in: UnevenRoundedRectangle(bottomLeadingRadius: 18, bottomTrailingRadius: 18))
                        .padding(.horizontal, 12)
                }
                .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { composerHeight = $0 }
            }
            .frame(maxWidth: Self.columnMaxWidth)
            // The composer and the cards above it scroll their own text.
            .onGeometryChange(for: CGRect.self) { $0.frame(in: .global) } action: {
                ChatScrollInput.shared.composerFrame = $0
            }
            .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { centeredHeight = $0 }
            .padding(.horizontal, layout.padding)
            .padding(.bottom, 16 + composerLift)
            .frame(maxWidth: .infinity)
            // The chat fades out under the composer, so text scrolled behind
            // it does not show around its tray or below it.
            .background(alignment: .bottom) {
                if !centersComposer {
                    LinearGradient(stops: [.init(color: Color.chatBackground.opacity(0), location: 0),
                                           .init(color: Color.chatBackground, location: 0.4)],
                                   startPoint: .top, endPoint: .bottom)
                        .frame(height: composerHeight + 16 + 36)
                        .allowsHitTesting(false)
                        .ignoresSafeArea()
                }
            }
        }
        // A new thread's banner over the top half of the chat, behind its
        // title bar, fading toward the composer. Only a new thread has one:
        // it fades away as the first message is sent.
        .background(alignment: .top) {
            ZStack {
                if showsBanner {
                    NewThreadBannerView(name: newThreadBanner, style: newThreadBannerStyle, height: chatHeight / 2)
                        .transition(.opacity)
                }
            }
            .ignoresSafeArea()
            .animation(reduceMotion ? nil : .easeOut(duration: 0.4), value: showsBanner)
        }
        .task { NewThreadBanner.preload() }
        // Looked at again as the thread grows, never while a turn streams.
        .task(id: FolderSuggestionKey(session: model.selectedSession, folder: model.chatProject,
                                      count: model.isTurnRunning ? -1 : chat.transcript.items.count)) {
            guard model.canMoveChat, model.selectedSession != nil, !model.isTurnRunning else {
                if model.selectedSession == nil { suggestedFolder = nil }
                return
            }
            let items = chat.transcript.items, folder = model.chatProject
            let found = await Task.detached(priority: .utility) {
                FolderSuggestion.suggest(items: items, folder: folder) { FolderSuggestion.projectRoot(of: $0) }
            }.value
            guard !Task.isCancelled else { return }
            withAnimation(.snappy(duration: 0.2)) { suggestedFolder = found }
        }
        .onChange(of: isFresh, initial: true) { _, fresh in
            guard fresh != centersComposer else { return }
            // A thread opening or one already open does not move the
            // composer; a new thread and its first message do.
            let moves = fresh ? model.selectedSession == nil : sentFirstMessage
            if moves, !reduceMotion {
                withAnimation(.spring(duration: 0.55, bounce: 0.12)) { centersComposer = fresh }
            } else {
                centersComposer = fresh
            }
        }
        // Over the chat, not inside the composer, so its glass has the chat
        // to sample (#2551).
        .overlayPreferenceValue(PlanUsageAnchor.self) { anchor in
            GeometryReader { proxy in
                if model.showsPlanUsage, let anchor, let usage = model.planUsage {
                    PlanUsagePanel(usage: usage, meter: proxy[anchor], bounds: proxy.size)
                        .transition(.opacity.combined(with: .scale(scale: 0.96, anchor: .bottom)))
                }
            }
            .ignoresSafeArea()
        }
        .overlay { composerPanel }
        .overlay { CommandSuggestions(model: model, composer: model.composer, composerHeight: composerHeight + composerLift) }
        .animation(.snappy(duration: 0.18), value: model.composerPanel)
        // The column, its padding and the composer adapt to the chat's
        // width at set breakpoints (#2552).
        .environment(\.chatLayout, layout)
        .onGeometryChange(for: CGSize.self) { $0.size } action: {
            chatWidth = $0.width
            chatHeight = $0.height
        }
    }

    /// Open the finished turn and the run of calls `target` is folded
    /// into, so its row is drawn to be scrolled to.
    private func unfold(to target: Int) {
        let items = chat.transcript.items
        let before = items.prefix { $0.id != target }.reversed()
        var folding = ToolFolding(items: items, verbosity: verbosity, open: model.openToolRuns, running: isReplyStreaming)
        // A turn's work starts on its folded row: the target may be that row.
        let turn = folding.turn(for: target)
            ?? (folding.hidden.contains(target) ? before.lazy.compactMap({ folding.turn(for: $0.id) }).first : nil)
        if let turn, !turn.isOpen {
            model.toggleToolRun(turn.key)
            folding = ToolFolding(items: items, verbosity: verbosity, open: model.openToolRuns, running: isReplyStreaming)
        }
        if folding.hidden.contains(target),
           let run = ([target] + before.map(\.id)).lazy.compactMap({ folding.fold(for: $0) }).first, !run.isOpen {
            model.toggleToolRun(run.run)
        }
    }

    private struct PlanReadKey: Equatable {
        var session: String?
        var updates: Int
        var running: Bool
    }

    /// How the column fits the chat's width now.
    private var layout: ChatColumnLayout { ChatColumnLayout(width: chatWidth) }

    /// A thread with nothing in it yet, ready for its first message.
    private var isFresh: Bool {
        chat.transcript.isEmpty && !model.busy && !model.isLoadingSelected
    }

    /// The first message of the shown thread was just sent, as opposed to
    /// another thread opening.
    private var sentFirstMessage: Bool {
        if model.isLoadingSelected { return false }
        if case .user = chat.transcript.items.last?.content { return true }
        return model.busy && chat.transcript.items.count <= 1
    }

    /// How far the composer's footer tray runs up under the composer.
    static let trayTuck: CGFloat = 14

    /// The banner shows over a new thread only, not one sent or opened.
    private var showsBanner: Bool {
        centersComposer && isFresh && model.selectedSession == nil && !newThreadBanner.isEmpty
    }

    /// How far the composer is raised to sit, with the banner over it, in
    /// the middle of the chat (a little above, where the eye rests).
    private var composerLift: CGFloat {
        centersComposer ? max(0, (chatHeight - centeredHeight) / 2 - 16 - 24) : 0
    }

    private var emptyChat: some View {
        EmptyChat(folder: model.chatProject, isDefault: model.draftProject == nil,
                  isDraft: model.selectedSession == nil,
                  choose: { Task { await model.chooseChatProject() } },
                  showVisualizations: { model.showsVisualizationGuide = true })
            .id(chat.transcript.identity)
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
                        case .model: ModelEffortPanel(model: model)
                        case .agents: AgentsPanel(model: model)
                        }
                    }
                    .padding(6)
                    .frame(width: panel == .model ? min(380, max(CGFloat(layout.column), 290)) : 290, alignment: .leading)
                    .glassSurface(in: .rect(cornerRadius: 20))
                    if panel == .add { Spacer(minLength: 0) }
                }
                .frame(maxWidth: Self.columnMaxWidth)
                .padding(.horizontal, layout.padding)
                .padding(.bottom, composerHeight + 16 + composerLift + 8)
                .frame(maxWidth: .infinity)
            }
            .transition(.opacity.combined(with: .offset(y: 6)))
            .background { Button("") { model.composerPanel = nil }.keyboardShortcut(.cancelAction).hidden() }
        }
    }

    /// The title bar's side margin, near its top and bottom ones.
    private static let headerInset: CGFloat = 10

    private func titleLine(path width: CGFloat?) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 6) {
            ChatProjectMenu(model: model, name: projectName,
                            path: width == nil ? projectName : model.chatProject.path.abbreviatingHome,
                            maxPathWidth: width ?? ChatProjectMenu.maxPathWidth)
            Text("/")
                .font(.title3.weight(.light))
                .foregroundStyle(.quaternary)
                .accessibilityHidden(true)
            Text(title).font(.headline).lineLimit(1)
                .frame(minWidth: min(160, titleWidth), alignment: .leading)
        }
    }

    /// The chat's title bar: the title and project, and the sidebar buttons
    /// beside the traffic lights while the sidebar is hidden.
    private var header: some View {
        HStack(spacing: 10) {
            if !model.showsSidebar {
                SidebarBarButtons(model: model)
                    .padding(.leading, SidebarMetrics.barLeading - Self.headerInset)
            }
            // `~/code / Title`: the chat's folder, a divider, its title.
            // The title is what the bar names: when the bar is narrow, as
            // with a side panel open, the folder gives way first, to a
            // shorter path and then to its name alone.
            ViewThatFits(in: .horizontal) {
                titleLine(path: ChatProjectMenu.maxPathWidth)
                titleLine(path: 120)
                titleLine(path: nil)
            }
            Spacer()
            ForEach(model.pullWatches(in: model.selectedSession)) { watch in
                PullWatchChip(model: model, watch: watch)
            }
            TaskPlanButton(model: model, chat: chat)
            if let usage = chat.transcript.usage {
                Button { model.showContext() } label: { UsageCapsule(usage: usage) }
                    .buttonStyle(.plain)
                    .accessibilityHint("Shows how the context window is used")
            }
            if let session = model.sessions.first(where: { $0.id == model.selectedSession }) {
                ToolbarCluster {
                    ClusterButton(systemImage: "magnifyingglass", help: model.keybindings.help("Find in thread", .findInChat), isOn: model.showsFind) {
                        model.showsFind.toggle()
                    }
                    ClusterButton(systemImage: model.pinnedSessions.contains(session.id) ? "pin.fill" : "pin",
                                  help: model.keybindings.help(model.pinnedSessions.contains(session.id) ? "Unpin thread" : "Pin thread", .pinChat),
                                  isOn: model.pinnedSessions.contains(session.id)) { model.togglePin(session) }
                    ClusterButton(systemImage: "folder", help: "Show thread in Finder") {
                        Task { await model.revealInFinder(session) }
                    }
                }
            }
            // The side panels' buttons, in one cluster like the sidebar's.
            ToolbarCluster {
                if model.canReadPullRequests, !model.showsPullRequests { PullRequestsButton(model: model) }
                TerminalToolbarButton(terminal: model.terminal, keybindings: model.keybindings)
                if !model.showsChanges {
                    ClusterButton(systemImage: "plus.forwardslash.minus", help: "Show the project's changes") { model.toggleChanges() }
                }
                if !model.showsPreview {
                    ClusterButton(systemImage: "globe", help: model.keybindings.help("Show localhost preview", .togglePreview)) { model.togglePreview() }
                }
            }
        }
        .padding(.horizontal, Self.headerInset)
        .frame(height: SessionSidebar.barHeight)
        .padding(.top, SessionSidebar.barTopInset)
        .padding(.bottom, 8)
        .background(WindowDragArea())
        // Over a new thread's banner, which runs up behind it, a frosted
        // bar whose foot fades into the banner rather than a hard edge.
        .background {
            if showsBanner {
                Rectangle()
                    .fill(.ultraThinMaterial)
                    .mask(LinearGradient(stops: [.init(color: .black, location: 0), .init(color: .black, location: 0.6),
                                                 .init(color: .clear, location: 1)],
                                         startPoint: .top, endPoint: .bottom))
                    .padding(.bottom, -12)
                    .transition(.opacity)
            } else {
                Color.chatBackground.opacity(0.94)
            }
        }
    }

    private let bottomAnchor = "bottom"
    private let topAnchor = "top"
    /// The working indicator's place in the scroller's map, after every row.
    private static let workingRow = Int.max
    /// The earlier-messages row's place, before every row.
    private static let earlierRow = Int.min

    /// The reader neared the top of the rows shown: add the page before them,
    /// keeping the row in view where it is.
    /// The lazy stack places rows added above at guessed heights and moves
    /// the rows in view with every guess, so the row in view is found again
    /// by id once they are in, and put back where it was.
    private func showEarlier(_ proxy: ScrollViewProxy) {
        let transcript = chat.transcript
        let count = transcript.items.count
        guard window.hasEarlier(identity: transcript.identity, count: count) else { return }
        if #available(macOS 15, *) {
            // The rows that join are drawn as stand-ins at heights the map
            // fixes now, so how much they add above the reader is known
            // before they are in: the chat scrolls down by exactly that as
            // the document grows, in the same pass. Found again by id after,
            // the reader's row was a frame late, flashed away and back, and
            // was sometimes a row from far below (#2445).
            let old = window.firstShown(identity: transcript.identity, count: count)
            window.showEarlier(identity: transcript.identity, count: count)
            let new = window.firstShown(identity: transcript.identity, count: count)
            let folding = ToolFolding(items: transcript.items, verbosity: verbosity,
                                      open: model.openToolRuns, running: isReplyStreaming)
            let joining = transcript.items[new..<old].filter { !folding.hidden.contains($0.id) }.map(\.id)
            scroll.willPrepend(joining, dropsEarlierRow: new == 0)
            return
        }
        pendingTop = scroll.topRow()
        scroll.willJump()
        scroll.pauseNearTop()
        window.showEarlier(identity: transcript.identity, count: transcript.items.count)
    }

    /// The older rows are in: go back to the row that was in view.
    private func restoreTop(_ proxy: ScrollViewProxy) {
        guard let top = pendingTop else { return }
        pendingTop = nil
        DispatchQueue.main.async {
            proxy.scrollTo(top.row, anchor: .top)
            scroll.hold(top.row, at: top.minY)
        }
    }

    /// Rows below the fold get their real height only once laid out, so one
    /// jump can land short of the end. On macOS 15 the scroll geometry sees
    /// that and scrolls again (`ChatFollow.update`) until the end is reached;
    /// before it, a second jump after layout is the best there is.
    private func scrollToEnd(_ proxy: ScrollViewProxy) {
        proxy.scrollTo(bottomAnchor, anchor: .bottom)
        if #unavailable(macOS 15) {
            Task { @MainActor in
                await Task.yield()
                if follow.follows { proxy.scrollTo(bottomAnchor, anchor: .bottom) }
            }
        }
    }

    /// A marker on the session timeline was chosen: go to its message, at
    /// the top of the view, joining it to the rows shown first if it is
    /// older. Rows above it take their real height as it lands, which
    /// pushed it down by hundreds of points: it is held at the top until
    /// they have, as rows joining above the view hold the reader's.
    private func jumpToTurn(_ id: Int, _ proxy: ScrollViewProxy) {
        follow.letGo()
        scroll.willJump()
        let items = chat.transcript.items
        let land = { [scroll] in
            proxy.scrollTo(id, anchor: .top)
            scroll.hold(id, at: 0)
        }
        if let index = items.firstIndex(where: { $0.id == id }),
           window.reveal(index, identity: chat.transcript.identity, count: items.count) {
            DispatchQueue.main.async(execute: land)
        } else {
            land()
        }
        scroll.flash()
    }

    /// The turns opened to their work, each from its header row to its last step.
    private static func openTurns(_ rows: ShownRows) -> [OpenTurn] {
        var turns: [OpenTurn] = []
        var current: OpenTurn?
        for item in rows.older + rows.newer {
            if let fold = rows.folding.turn(for: item.id), fold.isOpen {
                current.map { turns.append($0) }
                current = OpenTurn(key: fold.key, first: item.id, last: item.id, title: fold.title)
            } else if current != nil, rows.folding.isNested(item.id) {
                current?.last = item.id
            } else if let open = current {
                turns.append(open)
                current = nil
            }
        }
        current.map { turns.append($0) }
        return turns
    }

    /// Fold an opened turn from its sticky bar, and land on its header: the
    /// reader was in its steps, which are gone.
    private func foldTurn(_ turn: OpenTurn, _ proxy: ScrollViewProxy) {
        follow.letGo()
        scroll.willJump()
        withAnimation(.snappy(duration: 0.2)) { model.toggleToolRun(turn.key) }
        DispatchQueue.main.async { [scroll] in
            proxy.scrollTo(turn.first, anchor: .top)
            scroll.hold(turn.first, at: 0)
        }
    }

    private func jumpToLatest(_ proxy: ScrollViewProxy) {
        follow.jumpToLatest()
        scrollToEnd(proxy)
    }

    /// Page Up, Page Down, Home or End over the chat (`TranscriptKeys`).
    /// False when there is nothing to scroll, so the key goes on as usual.
    private func perform(_ action: TranscriptKeys.Action, _ proxy: ScrollViewProxy) -> Bool {
        guard !chat.transcript.isEmpty, !model.isLoadingSelected else { return false }
        let visible = scroll.visible
        let page = TranscriptKeys.pageDistance(visible: visible)
        switch TranscriptKeys.resolved(action, distanceFromBottom: follow.geometry?.distanceFromBottom, visible: visible) {
        case .pageUp:
            follow.letGo()
            scroll.scroll(by: -page, animated: true)
        case .pageDown:
            scroll.scroll(by: page, animated: true)
        case .top:
            follow.letGo()
            scroll.willJump()
            if window.showAll(identity: chat.transcript.identity, count: chat.transcript.items.count) {
                DispatchQueue.main.async { proxy.scrollTo(topAnchor, anchor: .top) }
            } else {
                proxy.scrollTo(topAnchor, anchor: .top)
            }
        case .end:
            jumpToLatest(proxy)
        }
        scroll.flash()
        return true
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
    @Environment(\.chatLayout) private var layout

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
                .padding(.horizontal, layout.padding)
                .padding(.bottom, composerHeight + 16 + 8)
                .frame(maxWidth: .infinity)
            }
            .transition(.opacity)
        }
    }
}

/// The chat's older rows, above the newest ones. Rows near the view are
/// drawn in full and the rest stand in at the height the chat measured for
/// them (`DrawnRows`, #2310): a lazy stack here sized itself from the rows
/// it had laid out just now, and each new guess moved the rows below it,
/// the ones the reader was on. Stand-ins cost about what a lazy stack's
/// rows not laid out do. Without the scroll geometry of macOS 15 the chat
/// cannot tell what is near the view, so it keeps the lazy stack.
private struct OlderRows: View {
    let items: [TranscriptItem]
    /// Older messages not shown yet.
    let earlier: Int
    let earlierRow: Int
    let folding: ToolFolding
    let agents: [Int: [AgentSpawn]]
    /// The long messages opened.
    let expanded: Set<Int>
    let currentMatch: Int?
    let scroll: ChatScrollState
    @ObservedObject var drawn: DrawnRowsState

    var body: some View {
        if #available(macOS 15, *) {
            VStack(alignment: .leading, spacing: 18) { rows(all: false) }
        } else {
            LazyVStack(alignment: .leading, spacing: 18) { rows(all: true) }
        }
    }

    @ViewBuilder
    private func rows(all: Bool) -> some View {
        if earlier > 0 {
            EarlierMessages(count: earlier)
                .reportsChatRow(earlierRow, to: scroll)
        }
        ForEach(items) { item in
            if all || drawn.ids.contains(item.id) || item.id == currentMatch {
                TranscriptRow(item: item, isCurrentMatch: item.id == currentMatch,
                              fold: folding.fold(for: item.id), turnFold: folding.turn(for: item.id),
                              isNested: folding.isNested(item.id), isExpanded: expanded.contains(item.id),
                              agents: agents[item.id], completion: folding.completion(for: item.id))
                    .equatable()
                    .transition(.rowReveal)
                    .reportsChatRow(item.id, to: scroll)
                    .id(item.id)
            } else {
                Color.clear
                    .frame(height: scroll.standInHeight(of: item.id))
                    .id(item.id)
            }
        }
    }
}

/// Above the oldest message laid out while older ones wait: they join as
/// the reader scrolls up to here.
private struct EarlierMessages: View {
    let count: Int

    var body: some View {
        HStack(spacing: 8) {
            ProgressView().controlSize(.small)
            Text(count == 1 ? "1 earlier message" : "\(count) earlier messages")
        }
        .font(.callout)
        .foregroundStyle(.secondary)
        .frame(maxWidth: .infinity)
        .padding(.vertical, 6)
        .accessibilityElement(children: .combine)
    }
}

/// Jumps back to the newest message after the reader scrolled up.
/// An opened turn whose header scrolled away: its title and a control that
/// folds it again, floating at the top of the chat.
private struct StickyTurnBar: View {
    @ObservedObject var state: StickyTurnState
    let fold: (OpenTurn) -> Void
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        HStack {
            if let turn = state.turn {
                Button { fold(turn) } label: {
                    HStack(spacing: 6) {
                        Text(turn.title).foregroundStyle(.secondary)
                        Text("·").foregroundStyle(.tertiary)
                        Text("Hide the work")
                        Image(systemName: "chevron.up").font(.caption2.weight(.semibold))
                    }
                    .font(.callout)
                    .padding(.horizontal, 12)
                    .frame(height: 28)
                    .contentShape(.capsule)
                    .interactiveGlassSurface(in: .capsule)
                }
                .buttonStyle(.plain)
                .pointingHandCursor()
                .help("Fold this turn's work and go back to where it starts")
                .accessibilityLabel("\(turn.title). Hide the work")
                .transition(.opacity.combined(with: .move(edge: .top)))
            }
            Spacer(minLength: 0)
        }
        .padding(.top, 8)
        .animation(reduceMotion ? nil : .snappy(duration: 0.18), value: state.turn?.key)
    }
}

private struct LatestButton: View {
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Label("Latest", systemImage: "arrow.down")
                .font(.callout.weight(.medium))
                .padding(.horizontal, 12)
                .frame(height: 30)
                .contentShape(.capsule)
                .interactiveGlassSurface(in: .capsule)
        }
        .buttonStyle(.plain)
        .help("Scroll to the latest message")
    }
}

/// The chat view's `ChatFollow`. Its geometry changes on every scrolled
/// frame and with every streamed token; kept in view state, each change
/// redrew the whole chat view, so scrolling while a reply streamed lagged
/// (#2260). Only what the view draws from it publishes: whether it follows,
/// which picks the scroll anchor, and whether Latest shows.
@MainActor
final class FollowState: ObservableObject {
    var value = ChatFollow() {
        didSet {
            if follows != value.follows { follows = value.follows }
            if showsLatest != value.showsLatest { showsLatest = value.showsLatest }
        }
    }
    @Published private(set) var follows = true
    @Published private(set) var showsLatest = false
}

private extension View {
    /// Drives `follow` the way chat apps do: any scroll up by the reader lets
    /// go of the end at once, reaching the end again holds on. While held, the
    /// chat stays anchored to its end as output grows; once let go, output
    /// growing below never moves what the reader is looking at. The decisions
    /// are `ChatFollow`'s; this only measures and reports.
    @ViewBuilder
    func followsLatest(_ follow: Binding<ChatFollow>, scroll: ChatScrollState,
                       scrollToEnd: @escaping () -> Void) -> some View {
        if #available(macOS 15, *) {
            defaultScrollAnchor(follow.wrappedValue.follows ? .bottom : .top, for: .sizeChanges)
            .onScrollGeometryChange(for: ChatFollow.Geometry.self) { geometry in
                // The visible rect spans the insets under the title bar and the
                // composer; the container size leaves them out on current macOS,
                // which put the end of the chat `insets` short of where the
                // scroll view stops, so Latest never hid there by itself.
                let viewport = geometry.visibleRect.height
                return ChatFollow.Geometry(
                    offset: geometry.contentOffset.y,
                    containerHeight: viewport,
                    distanceFromBottom: geometry.contentSize.height + geometry.contentInsets.bottom
                        - geometry.visibleRect.maxY,
                    canScroll: geometry.contentSize.height + geometry.contentInsets.top + geometry.contentInsets.bottom
                        > viewport + 1,
                    insetTop: geometry.contentInsets.top,
                    insetBottom: geometry.contentInsets.bottom)
            } action: { _, new in
                scroll.geometryChanged(offset: new.offset, viewport: new.containerHeight,
                                       insetTop: new.insetTop, insetBottom: new.insetBottom)
                if follow.wrappedValue.update(new, readerScrolling: ChatScrollInput.shared.isActive,
                                              towardEnd: ChatScrollInput.shared.isTowardEnd) {
                    scrollToEnd()
                }
            }
            .onScrollPhaseChange { _, phase in
                // Momentum and the bounce at the end finish after the last
                // input event; check once more where the scroll came to rest.
                if phase == .idle { follow.wrappedValue.scrollSettled() }
            }
            .onAppear {
                ChatScrollInput.shared.start()
                ChatScrollInput.shared.onScrollUp = { follow.wrappedValue.readerScrolledUp() }
            }
        } else {
            self
        }
    }
}

private extension View {
    /// The chat's own scroller in place of the system one, whose knob follows
    /// the lazy stack's guessed content size. It needs the scroll geometry of
    /// macOS 15; before it the system scroller stays.
    @ViewBuilder
    func chatScroller(_ state: ChatScrollState, scrollBy: @escaping (Double, Bool) -> Void) -> some View {
        if #available(macOS 15, *) {
            scrollIndicators(.never)
                .overlay(alignment: .topTrailing) { ChatScroller(state: state, scrollBy: scrollBy) }
        } else {
            self
        }
    }
}

/// The reader's own scrolling of the chat: wheel and trackpad events, and
/// drags of its scroller, over the chat's frame.
@MainActor
final class ChatScrollInput {
    static let shared = ChatScrollInput()
    /// The chat's scroll view in window coordinates, top-left origin,
    /// between the title bar and the composer.
    var chatFrame: CGRect = .zero
    /// The chat's scroll view below the title bar, with the band beside and
    /// below the composer that a wheel over it scrolls.
    var wheelFrame: CGRect = .zero
    /// The composer and the cards above it.
    var composerFrame: CGRect = .zero
    var onScrollUp: () -> Void = {}
    /// A page is in the chat's place, as an agent's conversation: wheels
    /// and clicks over it are that page's, not the chat's under it. Taken
    /// for the chat's, they let go of its end while its turn kept writing,
    /// and the chat was left far up when the reader came back (#2598).
    var isReplaced: () -> Bool = { false }
    private var lastInput = Date.distantPast
    private var monitor: Any?

    /// The reader is scrolling the chat, or just was.
    var isActive: Bool { Date().timeIntervalSince(lastInput) < 0.3 }
    /// The reader's latest scroll or scroller drag went down the chat.
    private(set) var isTowardEnd = false

    func start() {
        guard monitor == nil else { return }
        monitor = NSEvent.addLocalMonitorForEvents(matching: [.scrollWheel, .leftMouseDragged, .leftMouseDown, .leftMouseUp]) { event in
            let consumed = MainActor.assumeIsolated { ChatScrollInput.shared.observe(event) }
            return consumed ? nil : event
        }
    }

    /// Whether a wheel at `point` scrolls the chat: anywhere over its scroll
    /// view, which reaches under the composer's band, except the composer and
    /// the cards above it. A wheel beside the composer scrolled the chat
    /// without counting as the reader's, so a streaming reply pulled the chat
    /// back to its end on every token (#2554).
    static func wheelScrollsChat(at point: CGPoint, chat: CGRect, composer: CGRect, replaced: Bool = false) -> Bool {
        !replaced && chat.contains(point) && !composer.contains(point)
    }

    #if DEBUG
    /// The debug driver's wheel event over the chat, as `observe` sees one.
    func debugObserve(deltaY: Double) {
        guard !isReplaced() else { return }
        lastInput = Date()
        if deltaY != 0 { isTowardEnd = deltaY < 0 }
        if deltaY > 0 { onScrollUp() }
    }
    #endif

    /// Returns whether the event was used up, so it is not delivered.
    private func observe(_ event: NSEvent) -> Bool {
        // A drag that began beside a message selects its text wherever the
        // pointer goes, inside the chat or not.
        switch event.type {
        case .leftMouseDragged where GutterSelection.shared.drag(event): return true
        case .leftMouseUp:
            GutterSelection.shared.end()
            return false
        default: break
        }
        guard let content = event.window?.contentView else { return false }
        let location = event.locationInWindow
        let point = CGPoint(x: location.x, y: content.bounds.height - location.y)
        if event.type == .scrollWheel {
            guard Self.wheelScrollsChat(at: point, chat: wheelFrame, composer: composerFrame,
                                        replaced: isReplaced()) else { return false }
        } else {
            guard !isReplaced(), chatFrame.contains(point) else { return false }
        }
        if event.type == .leftMouseDown {
            if TextSelection.endIfOutsideText(event) {
                GutterSelection.shared.begin(event, in: chatFrame)
            }
            return false
        }
        lastInput = Date()
        // Wheel deltas are positive toward the top; a scroller drag moves its
        // knob, so down is toward the end.
        switch event.type {
        case .scrollWheel where event.scrollingDeltaY != 0: isTowardEnd = event.scrollingDeltaY < 0
        case .leftMouseDragged where event.deltaY != 0: isTowardEnd = event.deltaY > 0
        default: break
        }
        // Positive is toward the top of the chat, with natural scrolling or without.
        // Momentum continues a scroll; only the reader's own push lets go.
        if event.type == .scrollWheel, event.scrollingDeltaY > 0, event.momentumPhase.isEmpty {
            onScrollUp()
        }
        return false
    }
}

/// What Esc does in the composer: an open panel closes, then a draft
/// (text, command chip, files and images) clears, and only an empty composer
/// stops the running turn, so Esc never throws away a draft and the turn
/// together (#2631).
enum ComposerEscape: Equatable {
    case closePanel, clearDraft, stopTurn, ignore

    static func action(panelOpen: Bool, hasDraft: Bool, busy: Bool) -> ComposerEscape {
        if panelOpen { return .closePanel }
        if hasDraft { return .clearDraft }
        return busy ? .stopTurn : .ignore
    }
}

/// Page Up, Page Down, Home and End for the chat, caught before the focused
/// view sees them. Which keys and when is `TranscriptKeys`; this only tells
/// it where the focus is.
@MainActor
private final class TranscriptKeyInput {
    static let shared = TranscriptKeyInput()
    /// A sheet or panel is over the chat.
    var isCovered: () -> Bool = { false }
    /// Runs an action; false when the chat had nothing to scroll.
    var perform: (TranscriptKeys.Action) -> Bool = { _ in false }
    private var monitor: Any?

    func start() {
        guard monitor == nil else { return }
        monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { event in
            MainActor.assumeIsolated { TranscriptKeyInput.shared.handle(event) } ? nil : event
        }
    }

    private func handle(_ event: NSEvent) -> Bool {
        let modified = !event.modifierFlags.intersection([.shift, .option, .control, .command]).isEmpty
        if event.keyCode == KeyRecorderInput.escapeKeyCode, !modified, let window = event.window, window.attachedSheet == nil,
           !isCovered(), TextSelection.endOnEscape(in: window) {
            return true
        }
        guard let window = event.window, window.attachedSheet == nil,
              let action = TranscriptKeys.action(keyCode: event.keyCode, modified: modified,
                                                 focus: focus(window.firstResponder)) else { return false }
        return perform(action)
    }

    private func focus(_ responder: NSResponder?) -> TranscriptKeys.Focus {
        if isCovered() { return .panel }
        var view = responder as? NSView
        while let current = view {
            if current is TerminalView { return .terminal }
            if current is WKWebView { return .web }
            view = current.superview
        }
        return responder is NSText || responder is NSTextField ? .textField : .chat
    }
}

/// ⌘F: find in the chat. ⏎ and ⌘G go to the next match, ⇧⏎ and ⇧⌘G back, Esc closes.
private struct FindBar: View {
    @ObservedObject var model: DesktopModel
    /// Matches change as the query and the transcript do.
    @ObservedObject var find: ChatFind
    /// The field's own text: a keystroke redraws only this bar, and the
    /// search for it runs off the main actor.
    @State private var text = ""
    @FocusState private var focused: Bool

    var body: some View {
        let count = find.matches.count
        HStack(spacing: 8) {
            Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
            TextField("Find in thread", text: $text)
                .textFieldStyle(.plain)
                .focused($focused)
                .onKeyPress(.return, phases: .down) { press in
                    model.findNext(press.modifiers.contains(.shift) ? -1 : 1)
                    return .handled
                }
                .onKeyPress(.escape) {
                    if text.isEmpty { close() } else { text = "" }
                    return .handled
                }
                .onChange(of: text) { _, text in find.setQuery(text) }
            if !find.appliedQuery.isEmpty {
                Text(count == 0 ? "No matches" : "\(min(find.index, count - 1) + 1) of \(count)")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .monospacedDigit()
            }
            ChromeButton(systemImage: "chevron.up", help: model.keybindings.help("Previous", .findPrevious)) { model.findNext(-1) }
                .disabled(count == 0)
            ChromeButton(systemImage: "chevron.down", help: model.keybindings.help("Next", .findNext)) { model.findNext() }
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
        .onAppear {
            text = find.query
            find.transcriptChanged(model.transcript)
            focused = true
        }
        .onChange(of: model.showsFind) { _, shown in if shown { focused = true } }
    }

    private func close() {
        model.showsFind = false
    }
}

private struct EmptyChat: View {
    let folder: URL
    /// The draft starts in the default project, not a folder of its own.
    let isDefault: Bool
    /// A new chat, whose folder can still be changed.
    let isDraft: Bool
    let choose: () -> Void
    /// Opens the visualization guide.
    let showVisualizations: () -> Void
    @AppStorage(NewThreadBanner.key) private var banner = ""

    private static let tips: [(text: String, image: String, opensGuide: Bool)] = [
        ("Use + to attach files, folders, or images", "paperclip", false),
        ("Choose a folder just for this thread", "folder", false),
        ("Ask for a chart, table, or tree", "chart.bar.xaxis", true),
        ("Use the preview to view a local site alongside your chat", "globe", false),
        ("Open the terminal alongside your conversation", "terminal", false),
        ("Search a thread to find an earlier message", "magnifyingglass", false),
    ]
    @State private var tip = Int.random(in: 0..<tips.count)

    var body: some View {
        VStack(spacing: 14) {
            VStack(spacing: 14) {
                // A banner is drawn behind the chat instead of the icon.
                if banner.isEmpty {
                    Image(nsImage: NSApp.applicationIconImage)
                        .resizable()
                        .frame(width: 88, height: 88)
                }
                Text("What should we build?")
                    .font(.title.weight(.semibold))
            }
            .allowsHitTesting(false)
            if isDraft {
                Button(action: choose) {
                    HStack(spacing: 6) {
                        Label(folder.path.abbreviatingHome, systemImage: isDefault ? "folder" : "folder.fill")
                            .lineLimit(1)
                            .truncationMode(.middle)
                        Text(isDefault ? "Change" : "This thread only")
                            .foregroundStyle(Color.brand)
                    }
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .contentShape(.rect)
                }
                .buttonStyle(.plain)
                .frame(maxWidth: 560)
                .help("Choose the folder this thread works in. New threads still start in the default project.")
            } else {
                Label(folder.path.abbreviatingHome, systemImage: "folder")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .allowsHitTesting(false)
            }
            if Self.tips[tip].opensGuide {
                Button(action: showVisualizations) {
                    Label(Self.tips[tip].text, systemImage: Self.tips[tip].image)
                        .font(.callout)
                }
                .buttonStyle(.link)
                .help("Examples of visualizations you can ask for (/visualize)")
            } else {
                Label(Self.tips[tip].text, systemImage: Self.tips[tip].image)
                    .font(.callout)
                    .foregroundStyle(Color.brand)
                    .multilineTextAlignment(.center)
            }
        }
    }
}

/// The project folder beside the chat's title: a new chat's can be changed
/// for that chat only; an open chat stays in its own and can start another
/// chat there or elsewhere.
private struct ChatProjectMenu: View {
    enum Style { case header, footer }
    @ObservedObject var model: DesktopModel
    let name: String
    /// What the header's label shows: the folder's path, from home as `~`.
    let path: String
    /// The footer's label is a folder and the folder's name.
    var style = Style.header
    var maxPathWidth = ChatProjectMenu.maxPathWidth

    /// The widest the header's path grows before it is cut from the front.
    static let maxPathWidth: CGFloat = 260

    var body: some View {
        Menu {
            let folder = model.chatProject
            if model.selectedSession == nil {
                Button("Choose Folder for This Thread…", systemImage: "folder.badge.gearshape") {
                    Task { await model.chooseChatProject() }
                }
                let recent = ProjectFolder.recent(model.sessions, excluding: folder)
                if !recent.isEmpty {
                    Section("Recent") {
                        ForEach(recent, id: \.path) { recentFolder in
                            Button(recentFolder.path.abbreviatingHome, systemImage: "folder") {
                                Task { await model.newSession(in: recentFolder) }
                            }
                        }
                    }
                }
                if model.draftProject != nil {
                    Button("Use Default Project (\(model.workspace.lastPathComponent))", systemImage: "arrow.uturn.backward") {
                        model.draftProject = nil
                    }
                }
            } else {
                Button("New Thread in \(name)", systemImage: "square.and.pencil") {
                    Task { await model.newSession(in: folder) }
                }
                Button("New Thread in Another Folder…", systemImage: "folder.badge.plus") {
                    Task { await model.chooseChatProject() }
                }
                if model.canMoveChat {
                    Button("Change Thread Folder…", systemImage: "folder.badge.gearshape") {
                        Task { await model.moveChatProject() }
                    }
                    .disabled(model.isTurnRunning)
                }
            }
            Divider()
            Button("Open in Finder", systemImage: "folder") { NSWorkspace.shared.open(folder) }
        } label: {
            switch style {
            case .header:
                HStack(spacing: 3) {
                    // Capped, so a deep folder such as one under /private/tmp
                    // shows its last folders rather than pushing the title
                    // across the bar; the tooltip holds the whole path.
                    Text(path).truncationMode(.head)
                        .frame(maxWidth: maxPathWidth, alignment: .trailing)
                    Image(systemName: "chevron.down").font(.caption2.weight(.semibold))
                }
                .font(.callout)
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .padding(.horizontal, 8)
                .frame(height: 26)
                .contentShape(.rect(cornerRadius: 8))
            case .footer:
                HStack(spacing: 5) {
                    Image(systemName: "folder")
                    Text(name)
                    Image(systemName: "chevron.down").font(.caption2.weight(.semibold)).foregroundStyle(.tertiary)
                }
                .footerControl()
            }
        }
        .menuStyle(.button)
        .buttonStyle(.plain)
        .menuIndicator(.hidden)
        .fixedSize()
        .hoverHighlight(cornerRadius: style == .footer ? FooterHover.radius : 8)
        .help(ProjectFolder.help(chat: model.chatProject, default: model.workspace))
        .accessibilityLabel("Project folder \(name)")
    }
}

/// Under the composer: the chat's project and files on the left, the agents
/// at work on the right, so the composer's own row keeps to writing and
/// sending. Agents here cover neither the chat's end nor the text (#2555).
private struct ComposerFooter: View {
    @ObservedObject var model: DesktopModel

    var body: some View {
        HStack(spacing: 4) {
            ChatProjectMenu(model: model, name: model.chatProject.lastPathComponent,
                            path: model.chatProject.path.abbreviatingHome, style: .footer)
            FooterButton(title: "Files", systemImage: "paperclip", help: "Attach files or folders") {
                model.chooseAttachments()
            }
            Spacer(minLength: 0)
            AgentPill(model: model)
        }
        .padding(.horizontal, 4)
        .frame(minHeight: 26)
        .animation(.snappy(duration: 0.2), value: SubagentInfo.workingLabel(model.subagents))
    }
}

/// A quiet footer control: icon and label, a fill only on hover.
private struct FooterButton: View {
    let title: String
    let systemImage: String
    let help: String
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Label(title, systemImage: systemImage)
                .footerControl()
        }
        .buttonStyle(.plain)
        .hoverHighlight(cornerRadius: FooterHover.radius, padding: 0)
        .help(help)
    }
}

/// The footer's hover fill, on its folder menu as on its buttons.
/// The footer controls' rounding.
private enum FooterHover {
    static let radius: CGFloat = 12
}

extension View {
    /// The footer's controls: callout, secondary, a 24 pt row.
    fileprivate func footerControl() -> some View {
        font(.callout)
            .foregroundStyle(.secondary)
            .lineLimit(1)
            .padding(.horizontal, 10)
            .frame(height: 26)
            .contentShape(.rect(cornerRadius: FooterHover.radius))
    }
}

private struct FolderSuggestionKey: Equatable {
    var session: String?
    var folder: URL
    var count: Int
}

/// Over the composer when the thread works mostly in another folder: move
/// it there, so subagents and plain commands start where the work is.
private struct FolderSuggestionCard: View {
    let folder: URL
    let current: URL
    let canMove: Bool
    let move: () -> Void
    let dismiss: () -> Void

    var body: some View {
        HStack(alignment: .center, spacing: 10) {
            Image(systemName: "folder.badge.questionmark").foregroundStyle(Color.brand)
            VStack(alignment: .leading, spacing: 2) {
                Text("This thread works in \(folder.lastPathComponent)").font(.callout.weight(.medium))
                Text("Its tools ran in \(folder.path.abbreviatingHome), not \(current.path.abbreviatingHome). Subagents and commands without a folder still start in \(current.lastPathComponent).")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(2)
            }
            Spacer(minLength: 8)
            Button("Not Now", action: dismiss).buttonStyle(PillButtonStyle())
            Button("Move Thread", action: move)
                .buttonStyle(PillButtonStyle(prominent: true))
                .disabled(!canMove)
                .help(canMove ? "Move this thread to \(folder.path.abbreviatingHome)" : "Stop the running turn first")
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .floatingSurface(in: .rect(cornerRadius: 14))
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
            Button("Dismiss", action: dismiss).buttonStyle(PillButtonStyle())
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .floatingSurface(in: .rect(cornerRadius: 14))
    }
}

/// Text a field is still composing: the accent of a dead key, such as ` on
/// Norwegian and other European layouts, or an input method's candidate. ⏎
/// is handled before the text system sees it, so an accent still marked was
/// never committed: the draft was sent without it, and the field, still
/// composing, kept the sent prompt instead of clearing.
enum MarkedText {
    /// Commit the marked text in `responder`, if it is a text view composing
    /// any, as typed. Returns whether there was any.
    @discardableResult
    static func commit(in responder: NSResponder?) -> Bool {
        guard let editor = responder as? NSTextView, editor.hasMarkedText() else { return false }
        let range = editor.markedRange()
        let text = (editor.string as NSString).substring(with: range)
        editor.insertText(text, replacementRange: range)
        return true
    }

    /// Commit whatever a field in any window is composing. Only the field
    /// being typed in composes, so this is the composer when it is sending.
    @discardableResult
    static func commitInWindows() -> Bool {
        NSApp.windows.reduce(false) { committed, window in commit(in: window.firstResponder) || committed }
    }
}

/// The floating message field, laid out like the iOS composer: the text on
/// top, and below it the + menu, the model and Send. ⏎ sends, ⇧⏎ breaks the line.
/// Apple's on-device inline predictions in the composer: the next word in
/// grey ahead of the cursor, which Tab accepts. A window's text fields share
/// one field editor, so the others go back to the system default when the
/// composer lets go.
@MainActor
enum ComposerPredictions {
    static let key = "composerPredictions"

    static func apply(composerFocused: Bool) {
        let enabled = UserDefaults.standard.object(forKey: key) as? Bool ?? true
        // The field editor becomes first responder a moment after focus moves.
        DispatchQueue.main.async {
            let window = NSApp.keyWindow ?? NSApp.windows.first { $0.isVisible && $0.contentView != nil }
            guard let editor = window?.firstResponder as? NSTextView else { return }
            let type: NSTextInputTraitType = composerFocused && enabled ? .yes : .default
            if editor.inlinePredictionType != type { editor.inlinePredictionType = type }
        }
    }
}

/// Keeps the composer's caret in view. The field editor scrolls only its own
/// field, which now grows to its full text inside the composer's scroll
/// view, so typing past the tenth line or moving the caret off screen
/// scrolled nothing (#2526). Sits behind the field, inside that scroll view.
private struct ComposerCaretFollower: NSViewRepresentable {
    func makeNSView(context: Context) -> FollowerView { FollowerView() }
    func updateNSView(_ view: FollowerView, context: Context) {}

    final class FollowerView: NSView {
        private var observers: [Any] = []
        private weak var adjustedLayout: NSLayoutManager?
        private weak var adjustedField: NSTextField?
        private var originalFontLeading = true

        override func hitTest(_ point: NSPoint) -> NSView? { nil }

        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            observers.forEach(NotificationCenter.default.removeObserver)
            observers = []
            restoreFontLeading()
            guard window != nil else { return }
            observers.append(NotificationCenter.default.addObserver(forName: NSControl.textDidEndEditingNotification, object: nil, queue: .main) { [weak self] note in
                MainActor.assumeIsolated {
                    guard let self, let field = note.object as? NSTextField, field === self.adjustedField else { return }
                    self.restoreFontLeading()
                }
            })
            for name in [NSTextView.didChangeSelectionNotification, NSText.didChangeNotification] {
                observers.append(NotificationCenter.default.addObserver(forName: name, object: nil, queue: .main) { [weak self] note in
                    guard let editor = note.object as? NSTextView else { return }
                    // After SwiftUI has grown the field to the new text.
                    DispatchQueue.main.async { MainActor.assumeIsolated { self?.follow(editor) } }
                })
            }
        }

        private func follow(_ editor: NSTextView) {
            guard let scroll = enclosingScrollView, let document = scroll.documentView,
                  editor.window === window, window?.firstResponder === editor, editor.isDescendant(of: document),
                  let layout = editor.layoutManager, let container = editor.textContainer else { return }
            var ancestor: NSView? = editor.superview
            while ancestor != nil && !(ancestor is NSTextField) { ancestor = ancestor?.superview }
            guard let field = ancestor as? NSTextField else { return }
            if adjustedLayout !== layout {
                restoreFontLeading()
                adjustedLayout = layout
                adjustedField = field
                originalFontLeading = layout.usesFontLeading
                // The unfocused field lays out 16-point lines; the shared field
                // editor otherwise adds font leading and shifts every next line.
                layout.usesFontLeading = false
                layout.invalidateLayout(forCharacterRange: NSRange(location: 0, length: editor.string.utf16.count), actualCharacterRange: nil)
            }
            let caret = NSRange(location: editor.selectedRange().upperBound, length: 0)
            let glyphs = layout.glyphRange(forCharacterRange: caret, actualCharacterRange: nil)
            var rect = layout.boundingRect(forGlyphRange: glyphs, in: container)
            if rect.height == 0 { rect.size.height = layout.defaultLineHeight(for: editor.font ?? .systemFont(ofSize: NSFont.systemFontSize)) }
            rect = rect.offsetBy(dx: editor.textContainerOrigin.x, dy: editor.textContainerOrigin.y)
            document.scrollToVisible(editor.convert(rect.insetBy(dx: 0, dy: -2), to: document))
        }

        private func restoreFontLeading() {
            adjustedLayout?.usesFontLeading = originalFontLeading
            adjustedLayout = nil
            adjustedField = nil
        }

        deinit { observers.forEach(NotificationCenter.default.removeObserver) }
    }
}

private struct Composer: View {
    @ObservedObject var model: DesktopModel
    @ObservedObject var composer: ComposerDraft
    /// A narrow chat drops the labels the row can do without (#2552).
    @Environment(\.chatLayout) private var layout
    @FocusState private var focused: Bool
    @State private var isDropTarget = false
    @State private var pasteMonitor: Any?
    /// The message field's height at its full text, for its scroll view.
    @State private var fieldHeight: CGFloat = 0

    private var canSend: Bool {
        !model.draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            || !model.attachments.isEmpty || !model.images.isEmpty || model.draftCommand != nil
            || !model.codeReferences.isEmpty
    }

    /// What changes the composer's contents or size; the whole composer
    /// animates when it does.
    private var animationState: [String] {
        [String(model.busy), String(canSend), String(model.selectedQueue.count), model.draftCommand?.name ?? "",
         SubagentInfo.workingLabel(model.subagents) ?? "", String(model.images.count), String(model.attachments.count),
         String(model.codeReferences.count)]
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            LinkedThreads(model: model)
            if !model.selectedQueue.isEmpty {
                QueuedPromptList(model: model)
                    .transition(.opacity)
            }
            if !model.images.isEmpty { imageChips }
            if !model.attachments.isEmpty { attachmentChips }
            if !model.codeReferences.isEmpty { referenceChips }
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                if let chip = model.draftCommand {
                    Text(chip.name)
                        .font(.body.weight(.semibold))
                        .foregroundStyle(Color.brand)
                        .padding(.horizontal, 8)
                        .padding(.vertical, 2)
                        .background(Color.brand.opacity(0.15), in: .capsule)
                        .help(chip.description)
                        .transition(.opacity)
                }
                messageField
            }
                .padding(.horizontal, 6)
                .padding(.top, 4)
                .onKeyPress(.return, phases: .down) { press in
                    // A dead key's accent still waiting for its letter goes
                    // in as typed first, or the cleared field keeps the prompt.
                    MarkedText.commitInWindows()
                    // ⇧⏎ (and ⌥⏎) start a new line at the cursor; ⌘⏎ steers a
                    // running turn; ⌘⇧⏎ starts the message as a new chat in
                    // the background (#2652).
                    if press.modifiers.contains(.command), press.modifiers.contains(.shift) {
                        if model.canLaunchInBackground { Task { await model.launchInBackground() } }
                        return .handled
                    }
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
                    if press.modifiers.contains(.command) {
                        Task { await model.steerOrSubmit() }
                    } else {
                        Task { await model.submit() }
                    }
                    return .handled
                }
                .onKeyPress(.tab) {
                    guard !model.commandSuggestions.isEmpty else { return .ignored }
                    model.completeCommand()
                    return .handled
                }
                .onKeyPress(.upArrow) {
                    if !model.commandSuggestions.isEmpty {
                        model.moveCommandSelection(-1)
                        return .handled
                    }
                    // An empty composer: ↑ recalls the last message to edit
                    // and resend, as the terminal does. With text in it, ↑
                    // moves the cursor within the text.
                    return model.recallLastMessage() ? .handled : .ignored
                }
                .onKeyPress(.downArrow) {
                    guard !model.commandSuggestions.isEmpty else { return .ignored }
                    model.moveCommandSelection(1)
                    return .handled
                }
                .onKeyPress(.escape) {
                    switch ComposerEscape.action(panelOpen: model.composerPanel != nil,
                                                 hasDraft: !model.draft.isEmpty || model.draftCommand != nil
                                                     || !model.attachments.isEmpty || !model.images.isEmpty
                                                     || !model.codeReferences.isEmpty,
                                                 busy: model.busy) {
                    case .closePanel: model.composerPanel = nil
                    case .clearDraft:
                        model.draft = ""
                        model.draftCommand = nil
                        model.attachments = []
                        model.codeReferences = []
                        model.images = []
                    case .stopTurn: Task { await model.cancel() }
                    case .ignore: return .ignored
                    }
                    return .handled
                }
                .onKeyPress(.delete) { model.unwrapCommandChip() ? .handled : .ignored }
                .onChange(of: composer.text) { _, _ in
                    model.commandSelection = 0
                    model.promoteTypedCommand()
                    model.warmEngine()
                }
            HStack(spacing: 8) {
                addMenu
                Spacer(minLength: 0)
                if let usage = model.planUsage {
                    PlanUsageMeter(usage: usage, hovering: $model.showsPlanUsage) { Task { await model.refreshPlanUsage() } }
                }
                modelMenu
                trailingAction
            }
            .task(id: model.selectedSession) { await model.refreshPlanUsage() }
        }
        .padding(10)
        // The composer changes as one: what it holds and its height move
        // together in one subtle animation, not each part on its own.
        .animation(.snappy(duration: 0.2), value: animationState)
        // A plain material without the glass's rim: the field has no border.
        .floatingSurface(in: .rect(cornerRadius: 22))
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
        // An example inserted from the visualization guide is ready to send.
        .onChange(of: model.showsVisualizationGuide) { _, shown in if !shown { focused = true } }
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
        if model.canSteer { return "⏎ queues for the next turn, ⌘⏎ steers this one…" }
        return model.busy ? "Queue a message…" : "Message RustCode…"
    }

    private static let lineHeight = NSLayoutManager().defaultLineHeight(for: .systemFont(ofSize: NSFont.systemFontSize))
    /// Ten lines of body text: the composer grows to this, then scrolls.
    private static let maxFieldHeight = lineHeight * 10
    /// Two lines: an empty composer has room to start writing.
    private static let minFieldHeight = lineHeight * 2

    /// The text, with the rest of a half-typed command shown in gray after it.
    /// The field grows with its text inside a scroll view no taller than ten
    /// lines: a capped vertical field scrolled only to follow the caret, so
    /// the wheel could not reach the rest of a long draft (#2526). The scroll
    /// view lies outside the chat's frame, so the chat never takes its wheel.
    private var messageField: some View {
        ScrollView(.vertical) {
            field
                .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { fieldHeight = $0 }
                .background(ComposerCaretFollower())
        }
        .scrollBounceBehavior(.basedOnSize)
        .scrollIndicators(fieldHeight > Self.maxFieldHeight ? .automatic : .never)
        .frame(height: min(max(fieldHeight, Self.minFieldHeight), Self.maxFieldHeight), alignment: .top)
    }

    private var field: some View {
        TextField(placeholder, text: $composer.text, axis: .vertical)
            .textFieldStyle(.plain)
            .font(.body)
            .focused($focused)
            // AppKit's field editor draws text one point higher than the
            // unfocused field; keep the text and placeholder steady on focus.
            .offset(y: focused ? 1 : 0)
            .onChange(of: focused, initial: true) { _, on in ComposerPredictions.apply(composerFocused: on) }
            // Focus can settle before the field editor takes over: typing is
            // when the setting matters, so it is checked again then.
            .onChange(of: composer.text) { _, _ in if focused { ComposerPredictions.apply(composerFocused: true) } }
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
        .help("Add files, start a thread or switch project")
    }

    /// Model, reasoning effort and Fast mode in one pill that opens one
    /// picker, as the iOS app's Model & Effort sheet (#2595). The pill names
    /// the model, then the effort a shade dimmer (left out in a narrow
    /// composer), and a bolt while Fast is on.
    @ViewBuilder
    private var modelMenu: some View {
        if !model.models.choices.isEmpty {
            let effort = model.models.efforts.count > 1 ? model.models.effort ?? "default" : nil
            let effortName = effort.map { id in model.models.efforts.first { $0.id == id }?.name ?? ModelChoice.effortLabel(id) }
            let thinks = effort.map { $0 != "default" } ?? false
            let fast = model.models.fast == true
            Button { toggle(.model) } label: {
                HStack(spacing: 5) {
                    Text(layout.compactComposer ? Self.shortened(modelLabel) : modelLabel).lineLimit(1)
                    if thinks, !layout.compactComposer, let effortName {
                        Text(effortName).lineLimit(1).foregroundStyle(.tertiary)
                    }
                    if fast {
                        Image(systemName: "bolt.fill").font(.caption.weight(.semibold)).foregroundStyle(Color.brand)
                    }
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
            .help(ModelEffortSummary.help(model: modelLabel, effort: effort.map { _ in thinks ? effortName ?? "On" : "Off" },
                                          fast: model.models.fast))
            .accessibilityLabel("Model & Effort")
            .accessibilityValue(ModelEffortSummary.help(model: modelLabel, effort: effort.map { _ in thinks ? effortName ?? "On" : "Off" },
                                                        fast: model.models.fast))
        }
    }

    /// A model name cut to fit a narrow composer: `gemini-3.6-fl…`.
    static func shortened(_ name: String, to limit: Int = 14) -> String {
        name.count <= limit ? name : String(name.prefix(limit - 1)) + "…"
    }

    /// `provider/model` reads as just the model in the composer.
    private var modelLabel: String { ModelEffortPanel.label(for: model) }

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
                .transition(.opacity)
            }
            Button { Task { await model.cancel() } } label: {
                Image(systemName: "stop.fill")
                    .font(.system(size: 11, weight: .bold))
                    .frame(width: 30, height: 30)
                    .background(Color.red.gradient, in: .circle)
                    .foregroundStyle(.white)
            }
            .buttonStyle(.plain)
            .help(model.keybindings.help("Stop turn", .stopTurn))
            .transition(.opacity)
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
            .transition(.opacity)
        }
    }

    /// Diff lines picked in the Changes panel (#2643): what each refers to,
    /// whether it asks or requests a change, and the excerpt on hover.
    private var referenceChips: some View {
        ScrollView(.horizontal) {
            HStack(spacing: 6) {
                ForEach(model.codeReferences) { reference in
                    HStack(spacing: 5) {
                        Image(systemName: reference.intent == .ask ? "questionmark.bubble" : "pencil.line")
                            .foregroundStyle(.secondary)
                        Text(reference.label).font(.callout.monospaced()).lineLimit(1).truncationMode(.middle)
                        Button { model.codeReferences.removeAll { $0.id == reference.id } } label: {
                            Image(systemName: "xmark.circle.fill").foregroundStyle(.secondary)
                        }
                        .buttonStyle(.plain)
                        .help("Remove")
                        .accessibilityLabel("Remove reference")
                    }
                    .padding(.leading, 8)
                    .padding(.trailing, 6)
                    .padding(.vertical, 5)
                    .background(.fill.tertiary, in: .capsule)
                    .help("\(reference.intent == .ask ? "Question about" : "Change requested for") \(reference.label), \(reference.scope). Sent with your message, not on its own; never a pull request comment.\n\n" + reference.excerpt.prefix(12).joined(separator: "\n"))
                    .accessibilityElement(children: .combine)
                    .accessibilityLabel("\(reference.intent == .ask ? "Question about" : "Change requested for") \(reference.label)")
                }
            }
        }
        .scrollIndicators(.never)
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
                Text(model.canSteer ? "· sent when this turn ends, or now with ⌘⏎" : "· sent when this turn ends")
                    .foregroundStyle(.secondary)
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
                    if model.canSteer && prompt.images.isEmpty {
                        Button { Task { await model.sendNow(prompt) } } label: { Image(systemName: "arrow.up.circle") }
                            .buttonStyle(.plain)
                            .foregroundStyle(.secondary)
                            .help("Send now: the running turn reads it after its current step")
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
                .transition(.asymmetric(insertion: .offset(y: 12).combined(with: .opacity),
                                        removal: .scale(scale: 0.95).combined(with: .opacity)))
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
            ("New Thread", "square.and.pencil", { Task { await model.newSession() } }),
            ("New Thread in Folder…", "folder.badge.plus", { Task { await model.chooseChatProject() } }),
            ("Choose Default Project…", "folder", { model.chooseWorkspace() }),
            ("Ask for a Chart, Table or Tree…", "chart.bar.xaxis", { model.showsVisualizationGuide = true }),
        ] + (model.canSendDraftElsewhere
            ? [("Send Message to Another Thread…", "paperplane", { model.chooseChatForDraft() })] : [])
            + (model.canLaunchInBackground
                ? [("Start as New Thread in Background  ⌘⇧⏎", "paperplane.circle", { Task { await model.launchInBackground() } })] : [])
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

/// The composer's Model & Effort picker (#2595): Fast mode and the model on
/// one line, the effort slider under them, and for a model the user set up,
/// how it writes tool calls. The model's name opens the model list in the
/// panel's place; picking one comes back. ←→ move the slider, ⏎ or Esc close.
struct ModelEffortPanel: View {
    @ObservedObject var model: DesktopModel
    @State private var showsModels = false
    /// The stop under a dragged knob, named in the header while it moves.
    @State private var previewEffort: Int?

    private var efforts: [ModelChoice] { model.models.efforts }
    private var hasEffort: Bool { efforts.count > 1 }
    private var showsToolProtocol: Bool { !model.models.toolProtocols.isEmpty && model.selectedSession != nil }
    /// With nothing to set but the model, the list is the whole panel.
    private var listOnly: Bool { !hasEffort && model.models.fast == nil && !showsToolProtocol }

    var body: some View {
        Group {
            if showsModels || listOnly {
                modelsPage
                    .transition(.asymmetric(insertion: .opacity.combined(with: .offset(x: 12)), removal: .opacity))
            } else {
                settingsPage
                    .transition(.asymmetric(insertion: .opacity.combined(with: .offset(x: -12)), removal: .opacity))
            }
        }
        .animation(.snappy(duration: 0.22), value: showsModels)
        // Letters typed as the picker opened (its shortcut) filter the list.
        .onAppear { if !model.panelTypeahead.isEmpty { showsModels = true } }
    }

    private var settingsPage: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 8) {
                if let fast = model.models.fast {
                    FastModeButton(isOn: fast) { on in Task { await model.setFast(on) } }
                }
                Spacer(minLength: 8)
                modelButton
            }
            if hasEffort {
                EffortSlider(model: model, preview: $previewEffort)
            }
            if showsToolProtocol {
                ToolProtocolRow(model: model)
            }
        }
        .padding(8)
    }

    /// `GPT-6.1 Sol  Light ›`: the model, then its effort dimmer.
    private var modelButton: some View {
        let current = efforts.firstIndex { $0.id == (model.models.effort ?? "default") }
        let shown = previewEffort ?? current
        return Button { showsModels = true } label: {
            HStack(spacing: 5) {
                Text(ModelEffortPanel.label(for: model)).foregroundStyle(.primary)
                if hasEffort, let shown, efforts.indices.contains(shown) {
                    Text(efforts[shown].name).foregroundStyle(.tertiary)
                        .contentTransition(.numericText())
                        .animation(.snappy(duration: 0.15), value: shown)
                }
                Image(systemName: "chevron.right").font(.caption2.weight(.semibold)).foregroundStyle(.tertiary)
            }
            .font(.callout.weight(.medium))
            .lineLimit(1)
            .padding(.horizontal, 10)
            .frame(height: 28)
            .contentShape(.capsule)
        }
        .buttonStyle(.plain)
        .hoverHighlight(cornerRadius: 14, padding: 0)
        .help("Choose a model")
        .accessibilityLabel("Model \(ModelEffortPanel.label(for: model))")
        .accessibilityHint("Shows the models")
    }

    private var modelsPage: some View {
        VStack(alignment: .leading, spacing: 6) {
            if !listOnly {
                Button { showsModels = false } label: {
                    HStack(spacing: 4) {
                        Image(systemName: "chevron.left").font(.caption.weight(.semibold))
                        Text("Models")
                    }
                    .font(.callout.weight(.semibold))
                    .foregroundStyle(.secondary)
                    .padding(.horizontal, 8)
                    .frame(height: 24)
                    .contentShape(.rect)
                }
                .buttonStyle(.plain)
                .help("Back to effort (Esc)")
            }
            ModelListPanel(model: model, maxHeight: 260, closesOnPick: listOnly,
                           picked: { showsModels = false }) {
                if listOnly { model.composerPanel = nil } else { showsModels = false }
            }
        }
    }

    /// `provider/model` reads as just the model.
    static func label(for model: DesktopModel) -> String {
        let current = model.models.choices.first { $0.id == model.selectedModel }
        let name = current?.name ?? model.selectedModel ?? "Model"
        return name.split(separator: "/").last.map(String.init) ?? name
    }
}

/// Fast mode as a labelled toggle: offered only for a model on a ChatGPT
/// plan, and its speed costs plan usage, which the tooltip says.
struct FastModeButton: View {
    let isOn: Bool
    let set: (Bool) -> Void
    @State private var isHovered = false

    var body: some View {
        Button { set(!isOn) } label: {
            HStack(spacing: 5) {
                Image(systemName: isOn ? "bolt.fill" : "bolt")
                Text("Fast mode")
            }
            .font(.callout.weight(.medium))
            .foregroundStyle(isOn ? AnyShapeStyle(Color.brand) : AnyShapeStyle(.secondary))
            .padding(.horizontal, 10)
            .frame(height: 28)
            .background(isOn ? AnyShapeStyle(Color.brand.opacity(0.16))
                        : isHovered ? AnyShapeStyle(.fill.tertiary) : AnyShapeStyle(.clear), in: .capsule)
            .contentShape(.capsule)
            .animation(.snappy(duration: 0.18), value: isOn)
        }
        .buttonStyle(.plain)
        .onHover { isHovered = $0 }
        .help(ModelEffortSummary.fastExplanation)
        .accessibilityLabel("Fast mode")
        .accessibilityValue(isOn ? "On" : "Off")
    }
}

/// Reasoning effort as one track of evenly spaced stops, Off first. The knob
/// follows the pointer while dragged and springs to the nearest stop when
/// let go; a click on the track glides there. The engine hears only the
/// stop the knob settles on.
struct EffortSlider: View {
    @ObservedObject var model: DesktopModel
    /// The stop under the knob while it is dragged, for the header to name.
    @Binding var preview: Int?
    @FocusState private var focused: Bool
    /// The knob's position while a drag holds it, 0...1.
    @State private var dragFraction: Double?
    /// The stop picked, shown at once while the engine confirms it.
    @State private var pending: Int?
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private static let height: CGFloat = 28
    private static let inset: CGFloat = 2
    private static var knob: CGFloat { height - 2 * inset }

    private var levels: [ModelChoice] { model.models.efforts }
    private var current: Int {
        pending ?? levels.firstIndex { $0.id == (model.models.effort ?? "default") } ?? 0
    }
    private var shown: Int { dragFraction.map { EffortTrack.index(nearest: $0, count: levels.count) } ?? current }
    private var settle: Animation { reduceMotion ? .easeOut(duration: 0.12) : .spring(response: 0.32, dampingFraction: 0.78) }

    var body: some View {
        GeometryReader { proxy in
            let travel = max(proxy.size.width - Self.knob - 2 * Self.inset, 0)
            let fraction = dragFraction ?? EffortTrack.fraction(of: current, count: levels.count)
            let knobX = Self.inset + travel * fraction
            ZStack(alignment: .leading) {
                Capsule().fill(.fill.tertiary)
                Capsule().fill(Color.brandFill)
                    .frame(width: knobX + Self.knob + Self.inset)
                ForEach(levels.indices, id: \.self) { index in
                    Circle()
                        .fill(index <= shown ? AnyShapeStyle(Color.white.opacity(0.5)) : AnyShapeStyle(.tertiary))
                        .frame(width: 4, height: 4)
                        .offset(x: Self.inset + travel * EffortTrack.fraction(of: index, count: levels.count)
                                + Self.knob / 2 - 2)
                }
                Circle()
                    .fill(.white)
                    .shadow(color: .black.opacity(0.25), radius: 2, y: 1)
                    .frame(width: Self.knob, height: Self.knob)
                    .scaleEffect(dragFraction == nil ? 1 : 1.06)
                    .offset(x: knobX)
            }
            .contentShape(.capsule)
            .gesture(
                DragGesture(minimumDistance: 0)
                    .onChanged { drag in
                        let next = EffortTrack.fraction(atX: drag.location.x - Self.inset,
                                                        width: proxy.size.width - 2 * Self.inset, knob: Self.knob)
                        // The first touch glides there; then the knob tracks the pointer.
                        if dragFraction == nil {
                            withAnimation(settle) { dragFraction = next }
                        } else {
                            dragFraction = next
                        }
                    }
                    .onEnded { _ in
                        let index = shown
                        withAnimation(settle) {
                            dragFraction = nil
                            pending = index
                        }
                        commit(index)
                    }
            )
        }
        .frame(height: Self.height)
        .onChange(of: shown) { _, index in
            preview = dragFraction == nil ? nil : index
            if dragFraction != nil {
                NSHapticFeedbackManager.defaultPerformer.perform(.alignment, performanceTime: .now)
            }
        }
        .onChange(of: dragFraction == nil) { _, resting in if resting { preview = nil } }
        .onChange(of: model.models.effort) { _, _ in pending = nil }
        .focusable()
        .focusEffectDisabled()
        .focused($focused)
        .onAppear { focused = true }
        .onKeyPress(.leftArrow) { step(-1) }
        .onKeyPress(.rightArrow) { step(1) }
        .onKeyPress(.return) { model.composerPanel = nil; return .handled }
        .onKeyPress(.escape) { model.composerPanel = nil; return .handled }
        .accessibilityElement()
        .accessibilityLabel("Reasoning effort")
        .accessibilityValue(levels.indices.contains(current) ? levels[current].name : "")
        .accessibilityAdjustableAction { direction in
            _ = step(direction == .increment ? 1 : -1)
        }
        .help("Reasoning effort: \(levels.indices.contains(shown) ? levels[shown].name : "")")
    }

    private func step(_ delta: Int) -> KeyPress.Result {
        guard !levels.isEmpty else { return .ignored }
        let index = min(max(current + delta, 0), levels.count - 1)
        withAnimation(settle) { pending = index }
        commit(index)
        return .handled
    }

    private func commit(_ index: Int) {
        guard levels.indices.contains(index) else { return }
        let effort = levels[index].id
        guard effort != (model.models.effort ?? "default") else {
            pending = nil
            return
        }
        Task { await model.setEffort(effort) }
    }
}

/// How a model the user configured writes tool calls. A small local model
/// may only manage one of them; the choice is saved on its profile.
struct ToolProtocolRow: View {
    @ObservedObject var model: DesktopModel

    var body: some View {
        let choices = model.models.toolProtocols
        let current = model.models.toolProtocol ?? choices.first?.id ?? ""
        HStack {
            Label("Tool calls", systemImage: "wrench.and.screwdriver")
                .font(.callout)
                .foregroundStyle(.secondary)
            Spacer()
            Picker("Tool calls", selection: Binding(get: { current }, set: { id in
                Task { await model.setToolProtocol(id) }
            })) {
                ForEach(choices) { choice in
                    Text(choice.name).tag(choice.id)
                }
            }
            .pickerStyle(.menu)
            .labelsHidden()
            .fixedSize()
        }
        .padding(.horizontal, 6)
        .help(choices.first { $0.id == current }?.detail ?? "How the model writes tool calls")
    }
}

/// The model list with a filter, used by the composer panel and in Settings.
/// Esc clears the filter, then closes.
struct ModelListPanel: View {
    @ObservedObject var model: DesktopModel
    var maxHeight: CGFloat = 340
    /// Off in the Model & Effort picker, which stays open to show the
    /// picked model's effort and Fast choices.
    var closesOnPick = true
    /// After a pick that keeps the panel open.
    var picked: () -> Void = {}
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
        // As in the ⌘K switcher: the field is in the window only once the
        // panel's transition has inserted it, so focus is asked for again a
        // tick later, and letters typed meanwhile filter the list.
        .task {
            await Task.yield()
            focused = true
            try? await Task.sleep(for: .milliseconds(60))
            if focused { takeTypeahead() } else { focused = true }
        }
        .onChange(of: focused) { _, on in
            if on { takeTypeahead() }
        }
    }

    private func takeTypeahead() {
        let typed = model.takePanelTypeahead()
        if !typed.isEmpty { filter = typed + filter }
    }

    private func pick(_ choice: ModelChoice) {
        if closesOnPick { close() } else { picked() }
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
            .padding(.vertical, 8)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(isHighlighted ? AnyShapeStyle(Color.brand.opacity(0.2))
                        : isHovered ? AnyShapeStyle(.fill.tertiary) : AnyShapeStyle(.clear), in: .rect(cornerRadius: 14))
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .onHover { isHovered = $0 }
    }
}
