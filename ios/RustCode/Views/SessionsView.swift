import RustCodeRemote
import SwiftUI

struct OpenSessionSidebarAction: Sendable {
    var perform: @MainActor @Sendable (Bool) -> Void = { _ in }
    @MainActor func callAsFunction(toggle: Bool = false) { perform(toggle) }
}

extension EnvironmentValues {
    @Entry var openSessionSidebar = OpenSessionSidebarAction()
    @Entry var isChatObscured = false
}

struct SessionsView: View {
    @Environment(RemoteStore.self) private var store
    @Environment(\.horizontalSizeClass) private var sizeClass
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var drafts = ChatDrafts()
    @State private var showsSettings = LaunchOptions.current.presentsSettings
    @State private var unpairsAfterSettings = false
    @State private var showsSidebar = false
    @State private var showsNewSession = false
    @GestureState(resetTransaction: Transaction(animation: UIAccessibility.isReduceMotionEnabled ? nil : .snappy(duration: 0.25)))
    private var drawerDrag = DrawerDrag()
    @State private var dismissedKeyboardForDrag = false
    @State private var columns: NavigationSplitViewVisibility = .all
    @AccessibilityFocusState private var sidebarFocused: Bool
    var presentsAttentionOnLaunch = false

    var body: some View {
        Group {
            if sizeClass == .compact {
                GeometryReader { geometry in
                    let width = min(max(0, geometry.size.width * 0.8), 360)
                    let offset = min(width, max(0, (showsSidebar ? width : 0) + drawerDrag.offset))
                    let progress = width > 0 ? offset / width : 0
                    let obscured = showsSidebar || offset > 0
                    ZStack(alignment: .leading) {
                        NavigationStack {
                            SessionSidebar(showsSettings: $showsSettings, didSelect: closeSidebar)
                                .toolbar {
                                    ToolbarItem(placement: .topBarLeading) {
                                        Button("Close sessions", systemImage: "sidebar.left") { closeSidebar() }
                                            .accessibilityFocused($sidebarFocused)
                                    }
                                }
                        }
                        .frame(width: width)
                        .background(ChatSurface.sidebar.ignoresSafeArea())
                        .offset(x: offset - width)
                        .opacity(obscured ? 1 : 0)
                        .allowsHitTesting(showsSidebar)
                        .accessibilityHidden(!showsSidebar)
                        .accessibilityIdentifier("sessionSidebar")
                        .accessibilityAction(.escape, closeSidebar)

                        NavigationStack {
                            chat
                                .background(ChatSurface.background.ignoresSafeArea())
                                .softNavigationChrome()
                                .disabled(obscured)
                                .environment(\.isChatObscured, obscured)
                        }
                        .accessibilityHidden(obscured)
                        .overlay {
                            if obscured {
                                Button { closeSidebar() } label: {
                                    Color.black.opacity(0.16 * progress).ignoresSafeArea()
                                }
                                .buttonStyle(.plain)
                                .accessibilityLabel("Close sessions")
                            }
                        }
                        .clipShape(ChatScreenShape(
                            radius: 32 * progress,
                            topInset: geometry.safeAreaInsets.top,
                            bottomInset: geometry.safeAreaInsets.bottom
                        ))
                        .shadow(color: .black.opacity(0.2 * progress), radius: 16, x: -4)
                        // Keep the chat at its full width and move its header, transcript,
                        // and composer together with the menu throughout the gesture.
                        .offset(x: offset)
                    }
                    .background(ChatSurface.sidebar.ignoresSafeArea())
                    .simultaneousGesture(drawerGesture(width: width, opening: !showsSidebar))
                    .animation(reduceMotion ? nil : .snappy(duration: 0.25), value: showsSidebar)
                    .onChange(of: showsSidebar) { _, open in sidebarFocused = open }

                }
            } else {
                NavigationSplitView(columnVisibility: $columns) {
                    SessionSidebar(showsSettings: $showsSettings)
                        .navigationSplitViewColumnWidth(min: 280, ideal: 320, max: 380)
                } detail: {
                    chat
                        .background(ChatSurface.background.ignoresSafeArea())
                        .softNavigationChrome()
                }
                .toolbar(removing: .sidebarToggle)
            }
        }
        .background(ChatSurface.background.ignoresSafeArea())
        .environment(drafts)
        .tint(.primary)
        .sensoryFeedback(.selection, trigger: showsSidebar)
        .sensoryFeedback(.selection, trigger: store.selectedSessionID)
        .environment(\.openSessionSidebar, OpenSessionSidebarAction { toggle in
            UIApplication.shared.sendAction(#selector(UIResponder.resignFirstResponder), to: nil, from: nil, for: nil)
            if sizeClass == .compact { showsSidebar = true } else { columns = toggle && columns == .all ? .detailOnly : .all }
        })
        .onChange(of: store.selectedSessionID) { _, selected in
            if selected != nil { showsSidebar = false }
        }
        .sheet(isPresented: $showsSettings, onDismiss: {
            guard unpairsAfterSettings else { return }
            unpairsAfterSettings = false
            Task { await store.unpair() }
        }) {
            SettingsView { unpairsAfterSettings = true }
        }
        .sheet(isPresented: $showsNewSession) { NewSessionView() }
    }

    @ViewBuilder
    private var chat: some View {
        if store.selectedSessionID != nil {
            TranscriptView(presentsAttentionOnAppear: presentsAttentionOnLaunch)
        } else {
            ContentUnavailableView {
                Label("RustCode", systemImage: "bubble.left.and.text.bubble.right")
            } description: {
                Text("Start a conversation on your Mac, or continue one from your sessions.")
            } actions: {
                Button("Start a new session", systemImage: "square.and.pencil") { showsNewSession = true }
                    .brandProminentButtonStyle()
                    .disabled(store.connection != .connected)
            }
            .navigationTitle("RustCode")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button("Show sessions", systemImage: "line.3.horizontal") { showsSidebar = true; columns = .all }
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Settings", systemImage: "gearshape") { showsSettings = true }
                }
            }
            .safeAreaInset(edge: .bottom, spacing: 0) {
                ConnectionStatusBar()
            }
        }
    }

    private struct DrawerDrag {
        var horizontal: Bool?
        var offset: CGFloat = 0
    }

    private func drawerGesture(width: CGFloat, opening: Bool) -> some Gesture {
        DragGesture(minimumDistance: 10)
            .updating($drawerDrag) { value, state, transaction in
                guard opening ? value.startLocation.x <= 24 : value.startLocation.x >= width else { return }
                if state.horizontal == nil {
                    state.horizontal = abs(value.translation.width) > abs(value.translation.height) * 1.3
                }
                guard state.horizontal == true else { return }
                transaction.animation = nil
                state.offset = opening ? max(0, value.translation.width) : min(0, value.translation.width)
            }
            .onChanged { value in
                guard opening ? value.startLocation.x <= 24 : value.startLocation.x >= width else { return }
                guard !dismissedKeyboardForDrag, drawerDrag.horizontal != false,
                      abs(value.translation.width) > abs(value.translation.height) * 1.3 else { return }
                dismissedKeyboardForDrag = true
                UIApplication.shared.sendAction(#selector(UIResponder.resignFirstResponder), to: nil, from: nil, for: nil)
            }
            .onEnded { value in
                defer { dismissedKeyboardForDrag = false }
                guard opening ? value.startLocation.x <= 24 : value.startLocation.x >= width else { return }
                guard abs(value.translation.width) > abs(value.translation.height) * 1.3 else { return }
                let predicted = (opening ? 0 : width) + value.predictedEndTranslation.width
                withAnimation(reduceMotion ? nil : .snappy(duration: 0.25)) {
                    showsSidebar = predicted > width * 0.5
                }
            }
    }

    private func closeSidebar() {
        UIApplication.shared.sendAction(#selector(UIResponder.resignFirstResponder), to: nil, from: nil, for: nil)
        showsSidebar = false
    }
}

/// Include the status-bar and home-indicator backgrounds in the rounded chat mask.
private struct ChatScreenShape: Shape {
    var radius: CGFloat
    var topInset: CGFloat
    var bottomInset: CGFloat
    var animatableData: CGFloat {
        get { radius }
        set { radius = newValue }
    }

    func path(in rect: CGRect) -> Path {
        UnevenRoundedRectangle(topLeadingRadius: radius, bottomLeadingRadius: radius)
            .path(in: CGRect(x: rect.minX, y: rect.minY - topInset,
                             width: rect.width, height: rect.height + topInset + bottomInset))
    }
}

private struct SessionSidebar: View {
    @Environment(RemoteStore.self) private var store
    @Binding var showsSettings: Bool
    var didSelect: () -> Void = {}
    @State private var query = ""
    @State private var showsNewSession = false
    @State private var copiedIDCount = 0

    private var sessions: [SessionSummary] {
        let query = query.trimmingCharacters(in: .whitespaces)
        guard !query.isEmpty else { return store.sessions }
        return store.sessions.filter { session in
            [session.displayTitle, session.workspace, session.model].contains { $0.localizedStandardContains(query) }
        }
    }

    /// Saved sessions on the host that are not live. The host searches them too; filtering
    /// here as well keeps the list consistent while its answer is on the way.
    private var recent: [SavedSession] {
        guard store.supportsSavedSessions else { return [] }
        let query = query.trimmingCharacters(in: .whitespaces)
        guard !query.isEmpty else { return store.recentSessions }
        return store.recentSessions.filter { session in
            [session.displayTitle, session.workspace ?? "", session.id].contains { $0.localizedStandardContains(query) }
        }
    }

    /// What the saved-session listing depends on; a change lists again.
    private struct SavedListing: Equatable {
        var query: String
        var available: Bool
    }

    var body: some View {
        VStack(spacing: 8) {
            HStack(spacing: 8) {
                Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
                TextField("Search sessions", text: $query)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .submitLabel(.search)
                    .accessibilityLabel("Search sessions")
                if !query.isEmpty {
                    Button { query = "" } label: {
                        Image(systemName: "xmark.circle.fill").foregroundStyle(.secondary)
                            .frame(width: 44, height: 44)
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Clear session search")
                }
            }
            .padding(.leading, 14)
            .padding(.trailing, query.isEmpty ? 14 : 0)
            .frame(minHeight: 44)
            .glassSurface(in: .capsule, interactive: true)
            .padding(.horizontal, 16)
            .padding(.top, 8)

            List {
                Section {
                ForEach(sessions) { session in
                    Button {
                        store.select(session.id)
                        didSelect()
                    } label: {
                        SessionRow(session: session, badge: store.badges.badge(for: session))
                            .foregroundStyle(.primary)
                            .padding(12)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .background(
                                session.id == store.selectedSessionID ? Color.primary.opacity(0.07) : Color.clear,
                                in: .rect(cornerRadius: 12)
                            )
                            .contentShape(.rect)
                    }
                    .buttonStyle(.plain)
                    .listRowBackground(Color.clear)
                    .listRowSeparator(.hidden)
                    .listRowInsets(EdgeInsets(top: 2, leading: 12, bottom: 2, trailing: 12))
                    .contextMenu {
                        Button("Retry Latest Prompt", systemImage: "arrow.clockwise") {
                            didSelect()
                            Task { await store.retryLatestPrompt(in: session.id) }
                        }
                        .disabled(store.connection != .connected)
                        Divider()
                        Button("Copy Title", systemImage: "doc.on.doc") {
                            Clipboard.copy(session.displayTitle)
                        }
                        Button("Copy Workspace Path", systemImage: "folder") {
                            Clipboard.copy(session.workspace)
                        }
                        copySessionIDButton(session.id)
                        if session.id == store.selectedSessionID {
                            Button("Detach", systemImage: "eye.slash") {
                                store.detach()
                            }
                        }
                    }
                    .accessibilityAction(named: "Copy Session ID") { copySessionID(session.id) }
                    .swipeActions(edge: .leading) {
                        // Detaching only stops following the session from this device.
                        if session.id == store.selectedSessionID {
                            Button("Detach", systemImage: "eye.slash") {
                                store.detach()
                            }
                            .tint(.gray)
                        }
                    }
                }
                } header: {
                    if !recent.isEmpty, !sessions.isEmpty {
                        sectionHeader("Open")
                    }
                }

                if !recent.isEmpty {
                    Section {
                        ForEach(recent) { session in
                            recentRow(session)
                        }
                        if store.isLoadingSavedSessions {
                            ProgressView()
                                .frame(maxWidth: .infinity)
                                .listRowBackground(Color.clear)
                                .listRowSeparator(.hidden)
                        }
                    } header: {
                        sectionHeader("Recent")
                    }
                }
            }
            .listStyle(.plain)
            .scrollContentBackground(.hidden)
            .scrollDismissesKeyboard(.interactively)
            .refreshable {
                await store.refreshSessions()
                await store.loadSavedSessions(query: query)
            }
            .task(id: SavedListing(
                query: query.trimmingCharacters(in: .whitespaces),
                available: store.connection == .connected && store.supportsSavedSessions
            )) {
                guard store.connection == .connected, store.supportsSavedSessions else { return }
                // Typing refines the search; wait for a pause before asking the host.
                if !query.trimmingCharacters(in: .whitespaces).isEmpty {
                    guard (try? await Task.sleep(for: .milliseconds(300))) != nil else { return }
                }
                await store.loadSavedSessions(query: query)
            }
            .overlay {
                // Not hit-testable, so the empty list underneath can still be pulled to refresh.
                if store.sessions.isEmpty, recent.isEmpty, query.isEmpty {
                    emptyState.allowsHitTesting(false)
                } else if sessions.isEmpty, recent.isEmpty, !store.isLoadingSavedSessions {
                    ContentUnavailableView.search(text: query)
                }
            }

            Button {
                showsNewSession = true
            } label: {
                Label("New", systemImage: "square.and.pencil")
                    .font(.headline)
                    .frame(maxWidth: .infinity)
            }
            .brandProminentButtonStyle()
            .controlSize(.large)
            .disabled(store.connection != .connected)
            .accessibilityIdentifier("newSession")
            .padding(.horizontal, 16)
            .padding(.top, 8)
            .padding(.bottom, 8)
        }
        .copyConfirmation("Session ID Copied", trigger: copiedIDCount)
        .background(ChatSurface.sidebar.ignoresSafeArea())
        .navigationTitle("Sessions")
        .navigationBarTitleDisplayMode(.inline)
        .softNavigationChrome()
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button("Settings", systemImage: "gearshape") {
                    showsSettings = true
                }
            }
        }
        .sheet(isPresented: $showsNewSession) {
            NewSessionView()
        }
        .safeAreaInset(edge: .bottom, spacing: 0) {
            ConnectionStatusBar()
        }
    }

    /// The host's session identifier, never the title or workspace shown in the row.
    private func copySessionIDButton(_ id: String) -> some View {
        Button("Copy Session ID", systemImage: "number") { copySessionID(id) }
    }

    private func copySessionID(_ id: String) {
        Clipboard.copy(id)
        copiedIDCount += 1
    }

    private func sectionHeader(_ title: String) -> some View {
        Text(title)
            .font(.footnote.weight(.semibold))
            .foregroundStyle(.secondary)
            .textCase(nil)
            .padding(.horizontal, 12)
            .listRowInsets(EdgeInsets(top: 12, leading: 12, bottom: 4, trailing: 12))
            .accessibilityAddTraits(.isHeader)
    }

    private func recentRow(_ session: SavedSession) -> some View {
        Button {
            Task {
                if await store.openSavedSession(session.id) { didSelect() }
            }
        } label: {
            SavedSessionRow(session: session, isOpening: store.reopeningSessionID == session.id)
                .foregroundStyle(.primary)
                .padding(12)
                .frame(maxWidth: .infinity, alignment: .leading)
                .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .disabled(!session.canOpen || store.reopeningSessionID != nil || store.connection != .connected)
        .listRowBackground(Color.clear)
        .listRowSeparator(.hidden)
        .listRowInsets(EdgeInsets(top: 2, leading: 12, bottom: 2, trailing: 12))
        .contextMenu {
            Button("Copy Title", systemImage: "doc.on.doc") {
                Clipboard.copy(session.displayTitle)
            }
            if let workspace = session.workspace {
                Button("Copy Workspace Path", systemImage: "folder") {
                    Clipboard.copy(workspace)
                }
            }
            copySessionIDButton(session.id)
        }
        .accessibilityAction(named: "Copy Session ID") { copySessionID(session.id) }
        .onAppear {
            // Continue the listing as its end scrolls into view.
            if session.id == store.recentSessions.last?.id, store.hasMoreSavedSessions {
                Task { await store.loadMoreSavedSessions() }
            }
        }
    }

    @ViewBuilder
    private var emptyState: some View {
        switch store.connection {
        case .connected:
            ContentUnavailableView(
                "No Sessions Yet",
                systemImage: "terminal",
                description: Text("Tap New to choose a folder on \(store.host?.name ?? "your Mac"), or open RustCode in a terminal on it: every open session appears here.")
            )
        case .connecting:
            ProgressView("Connecting…")
        default:
            ContentUnavailableView(
                "Not Connected",
                systemImage: "wifi.slash",
                description: Text("Sessions appear once the host is reachable.")
            )
        }
    }
}

private struct SessionRow: View {
    let session: SessionSummary
    let badge: AttentionBadge?

    var body: some View {
        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 4) {
                Text(session.displayTitle)
                    .font(.body.weight(.medium))
                    .lineLimit(2)
                Text("\(session.workspace) · \(session.model)")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
                if session.turnCount != nil || session.lastActiveAt != nil {
                    HStack(spacing: 8) {
                        if let turns = session.turnCount {
                            Text("\(turns) \(turns == 1 ? "turn" : "turns")")
                        }
                        Spacer(minLength: 8)
                        if let lastActiveAt = session.lastActiveAt {
                            // Re-rendered each minute so "Now" and today's times stay current.
                            TimelineView(.everyMinute) { context in
                                Text(LastActivePresentation.label(for: lastActiveAt, now: context.date))
                                    .monospacedDigit()
                            }
                        }
                    }
                    .font(.caption)
                    .foregroundStyle(.secondary)
                }
            }
            Spacer(minLength: 8)
            indicator
        }
        .padding(.vertical, 4)
        .accessibilityElement(children: .combine)
        .accessibilityLabel("\(session.displayTitle), \(statusDescription)\(session.turnCount.map { ", \($0) turns" } ?? "")\(session.lastActiveAt.map { ", last active \(LastActivePresentation.label(for: $0))" } ?? "")")
    }

    @ViewBuilder
    private var indicator: some View {
        switch badge {
        case .approval:
            Image(systemName: "hand.raised.fill").foregroundStyle(.attention)
        case .question:
            Image(systemName: "questionmark.bubble.fill").foregroundStyle(.attention)
        case .completed:
            Circle().fill(.tint).frame(width: 10, height: 10)
        case nil:
            if session.health == .lost {
                Image(systemName: "bolt.horizontal.circle").foregroundStyle(.secondary)
            } else if session.activity == .running {
                ProgressView().controlSize(.small)
            }
        }
    }

    private var statusDescription: String {
        switch badge {
        case .approval: "approval needed"
        case .question: "question waiting"
        case .completed: "finished"
        case nil:
            switch session.activity {
            case .running: "running"
            case .waitingForUser: "waiting for you"
            case .idle: "idle"
            }
        }
    }
}

/// A session saved on the host but not live: tapping it reopens it on the host.
private struct SavedSessionRow: View {
    let session: SavedSession
    let isOpening: Bool

    var body: some View {
        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 4) {
                Text(session.displayTitle)
                    .font(.body.weight(.medium))
                    .lineLimit(2)
                Text(detail)
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                    // A workspace path keeps both ends; where it is open is a sentence that wraps.
                    .lineLimit(session.state == .open ? 2 : 1)
                    .truncationMode(session.state == .open ? .tail : .middle)
                HStack(spacing: 8) {
                    Text("\(session.messageCount) \(session.messageCount == 1 ? "message" : "messages")")
                    Spacer(minLength: 8)
                    if let updatedAt = session.updatedAt {
                        TimelineView(.everyMinute) { context in
                            Text(LastActivePresentation.label(for: updatedAt, now: context.date))
                                .monospacedDigit()
                        }
                    }
                }
                .font(.caption)
                .foregroundStyle(.secondary)
            }
            Spacer(minLength: 8)
            if isOpening {
                ProgressView().controlSize(.small)
            } else if session.state == .open {
                Image(systemName: session.openIn == .terminal ? "terminal" : "desktopcomputer")
                    .foregroundStyle(.secondary)
            }
        }
        .padding(.vertical, 4)
        .opacity(session.canOpen || isOpening ? 1 : 0.6)
        .accessibilityElement(children: .combine)
        .accessibilityLabel("\(session.displayTitle), \(status)\(session.updatedAt.map { ", last active \(LastActivePresentation.label(for: $0))" } ?? "")")
        .accessibilityHint(session.canOpen ? "Reopens this session on your Mac." : "")
    }

    private var detail: String {
        switch session.state {
        case .open: session.openElsewhereDetail
        case .saved, .shared, .unknown:
            if let workspace = session.workspace, !workspace.isEmpty {
                workspace
            } else {
                "No folder recorded · resume it in a terminal"
            }
        }
    }

    private var status: String {
        if isOpening { return "opening" }
        switch session.state {
        case .open: return session.openElsewhereDetail.prefix(1).lowercased() + session.openElsewhereDetail.dropFirst()
        case .saved where session.canOpen: return "saved"
        default: return "cannot be opened from here"
        }
    }
}

/// Shown under the session list whenever the connection is anything but healthy, and as a
/// quiet reminder in demo mode.
struct ConnectionStatusBar: View {
    @Environment(RemoteStore.self) private var store

    var body: some View {
        if let status {
            HStack(spacing: 10) {
                if store.connection == .connecting {
                    ProgressView().controlSize(.small)
                } else {
                    Image(systemName: status.symbol)
                        .foregroundStyle(.secondary)
                        .accessibilityHidden(true)
                }
                Text(status.text)
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                    .lineLimit(3)
                Spacer(minLength: 8)
                if store.canRetry {
                    Button("Retry") { store.retryNow() }
                        .font(.footnote.weight(.semibold))
                        .secondaryButtonStyle()
                        .buttonBorderShape(.capsule)
                        .controlSize(.small)
                        .accessibilityLabel("Retry connection now")
                }
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 10)
            .frame(maxWidth: .infinity)
            .glassSurface(in: .rect(cornerRadius: 22))
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
            .accessibilityElement(children: .contain)
        }
    }

    private var status: (symbol: String, text: String)? {
        switch store.connection {
        case .connected:
            store.isDemo ? ("play.circle", "Demo mode. This is a simulated host.") : nil
        case .connecting:
            ("arrow.triangle.2.circlepath", "Connecting to \(store.host?.name ?? "host")…")
        case .waitingToRetry(let attempt, let delay, _):
            ("wifi.exclamationmark", "Connection lost. Retrying in \(delay.formatted(.units(allowed: [.seconds], width: .wide))) (attempt \(attempt)).")
        case .offline(let reason):
            ("wifi.slash", reason)
        case .suspended:
            ("pause.circle", "Paused while in the background.")
        case .unpaired, .unauthorized:
            nil
        }
    }
}

extension ConnectionState {
    var summary: String {
        switch self {
        case .unpaired: "Not paired"
        case .connecting: "Connecting"
        case .connected: "Connected"
        case .waitingToRetry: "Reconnecting"
        case .offline: "Offline"
        case .suspended: "Paused"
        case .unauthorized: "Not authorized"
        }
    }
}
