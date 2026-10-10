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
    @Environment(\.colorScheme) private var colorScheme
    @Environment(\.horizontalSizeClass) private var sizeClass
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var drafts = ChatDrafts()
    @State private var showsSettings = LaunchOptions.current.presentsSettings
    @State private var unpairsAfterSettings = false
    @State private var showsSidebar = false
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
                    let width = min(geometry.size.width * 0.84, 360)
                    let offset = min(width, max(0, (showsSidebar ? width : 0) + drawerDrag.offset))
                    let progress = offset / width
                    let obscured = showsSidebar || offset > 0
                    ZStack(alignment: .leading) {
                        Color(uiColor: .systemBackground).ignoresSafeArea()
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
                        .offset(x: -32 * (1 - progress))
                        .opacity(progress)
                        .allowsHitTesting(showsSidebar)
                        .accessibilityHidden(!showsSidebar)
                        .accessibilityIdentifier("sessionSidebar")
                        .accessibilityAction(.escape, closeSidebar)

                        NavigationStack {
                            chat
                                .background(colorScheme == .dark ? Color(white: 0.07) : Color(uiColor: .systemBackground))
                                .disabled(showsSidebar)
                                .accessibilityHidden(obscured)
                                .environment(\.isChatObscured, obscured)
                        }
                        .background(Color(uiColor: .secondarySystemBackground))
                        .overlay {
                            if obscured {
                                Button { closeSidebar() } label: {
                                    Color.black.opacity(0.12 * progress).ignoresSafeArea()
                                }
                                .buttonStyle(.plain)
                                .accessibilityLabel("Close sessions")
                            }
                        }
                        .clipShape(.rect(cornerRadius: 28 * progress))
                        .overlay {
                            RoundedRectangle(cornerRadius: 28 * progress)
                                .stroke(.white.opacity(0.08 * progress), lineWidth: 0.5)
                                .allowsHitTesting(false)
                        }
                        .shadow(color: .black.opacity(0.3 * progress), radius: 16, x: -4)
                        .offset(x: offset)
                        .accessibilityHidden(obscured)
                    }
                    .clipped()
                    .simultaneousGesture(drawerGesture(width: width, opening: !showsSidebar))
                    .animation(reduceMotion ? nil : .snappy(duration: 0.25), value: showsSidebar)
                    .onChange(of: showsSidebar) { _, open in sidebarFocused = open }

                }
            } else {
                NavigationSplitView(columnVisibility: $columns) {
                    SessionSidebar(showsSettings: $showsSettings)
                        .navigationSplitViewColumnWidth(min: 280, ideal: 320, max: 380)
                } detail: { chat }
                .toolbar(removing: .sidebarToggle)
            }
        }
        .environment(drafts)
        .environment(\.openSessionSidebar, OpenSessionSidebarAction { toggle in
            UIApplication.shared.sendAction(#selector(UIResponder.resignFirstResponder), to: nil, from: nil, for: nil)
            if sizeClass == .compact { showsSidebar = true } else { columns = toggle && columns == .all ? .detailOnly : .all }
        })
        .onAppear { if store.selectedSessionID == nil { showsSidebar = true } }
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
    }

    @ViewBuilder
    private var chat: some View {
        if store.selectedSessionID != nil {
            TranscriptView(presentsAttentionOnAppear: presentsAttentionOnLaunch)
        } else {
            ContentUnavailableView {
                Label("Choose a Session", systemImage: "bubble.left.and.text.bubble.right")
            } description: {
                Text("Open your sessions or start a new conversation on your Mac.")
            } actions: {
                Button("Show sessions", systemImage: "sidebar.left") { showsSidebar = true; columns = .all }
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
                guard !opening || value.startLocation.x <= 24 else { return }
                if state.horizontal == nil {
                    state.horizontal = abs(value.translation.width) > abs(value.translation.height) * 1.3
                }
                guard state.horizontal == true else { return }
                transaction.animation = nil
                state.offset = opening ? max(0, value.translation.width) : min(0, value.translation.width)
            }
            .onChanged { value in
                guard !opening || value.startLocation.x <= 24 else { return }
                guard !dismissedKeyboardForDrag, drawerDrag.horizontal != false,
                      abs(value.translation.width) > abs(value.translation.height) * 1.3 else { return }
                dismissedKeyboardForDrag = true
                UIApplication.shared.sendAction(#selector(UIResponder.resignFirstResponder), to: nil, from: nil, for: nil)
            }
            .onEnded { value in
                defer { dismissedKeyboardForDrag = false }
                guard !opening || value.startLocation.x <= 24 else { return }
                guard abs(value.translation.width) > abs(value.translation.height) * 1.3 else { return }
                let predicted = (opening ? 0 : width) + value.predictedEndTranslation.width
                withAnimation(reduceMotion ? nil : .snappy(duration: 0.25)) {
                    showsSidebar = predicted > width * 0.5
                }
            }
    }

    private func closeSidebar() { showsSidebar = false }
}

private struct SessionSidebar: View {
    @Environment(RemoteStore.self) private var store
    @Binding var showsSettings: Bool
    var didSelect: () -> Void = {}
    @State private var query = ""
    @State private var showsNewSession = false

    private var sessions: [SessionSummary] {
        let query = query.trimmingCharacters(in: .whitespaces)
        guard !query.isEmpty else { return store.sessions }
        return store.sessions.filter { session in
            [session.displayTitle, session.workspace, session.model].contains { $0.localizedStandardContains(query) }
        }
    }


    var body: some View {
        List {
            Section {
                ForEach(sessions) { session in
                    Button {
                        store.select(session.id)
                        didSelect()
                    } label: {
                        SessionRow(session: session, badge: store.badges.badge(for: session))
                            .foregroundStyle(.primary)
                    }
                    .buttonStyle(.plain)
                    .listRowBackground(session.id == store.selectedSessionID ? Color.accentColor.opacity(0.12) : Color.clear)
                        .contextMenu {
                            Button("Copy Title", systemImage: "doc.on.doc") {
                                Clipboard.copy(session.displayTitle)
                            }
                            Button("Copy Workspace Path", systemImage: "folder") {
                                Clipboard.copy(session.workspace)
                            }
                            if session.id == store.selectedSessionID {
                                Button("Detach", systemImage: "eye.slash") {
                                    store.detach()
                                }
                            }
                        }
                        .swipeActions(edge: .trailing) {
                            // Detaching only stops following the session from this device.
                            if session.id == store.selectedSessionID {
                                Button("Detach", systemImage: "eye.slash") {
                                    store.detach()
                                }
                                .tint(.gray)
                            }
                        }
                }
            } footer: {
                if !sessions.isEmpty {
                    Text("Start a session here, or share one from your Mac with /\u{2060}remote.")
                }
            }
        }
        .refreshable {
            await store.refreshSessions()
        }
        .overlay {
            // Not hit-testable, so the empty list underneath can still be pulled to refresh.
            if store.sessions.isEmpty {
                emptyState.allowsHitTesting(false)
            } else if sessions.isEmpty {
                ContentUnavailableView.search(text: query)
            }
        }
        .searchable(text: $query, prompt: "Search sessions")
        .navigationTitle("Sessions")
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button("New Session", systemImage: "square.and.pencil") {
                    showsNewSession = true
                }
                .disabled(store.connection != .connected)
            }
            ToolbarItem(placement: .topBarTrailing) {
                Button("Settings", systemImage: "gearshape") {
                    showsSettings = true
                }
            }
        }
        .sheet(isPresented: $showsNewSession) {
            NewSessionView()
        }
        .floatingBottomBar {
            ConnectionStatusBar()
        }
    }

    @ViewBuilder
    private var emptyState: some View {
        switch store.connection {
        case .connected:
            ContentUnavailableView(
                "No Sessions Yet",
                systemImage: "terminal",
                description: Text("Tap New Session to choose a folder on \(store.host?.name ?? "your Mac"), or share an existing session with /\u{2060}remote.")
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
                if let turns = session.turnCount {
                    Text("\(turns) \(turns == 1 ? "turn" : "turns")")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
            Spacer(minLength: 8)
            indicator
        }
        .padding(.vertical, 4)
        .accessibilityElement(children: .combine)
        .accessibilityLabel("\(session.displayTitle), \(statusDescription)\(session.turnCount.map { ", \($0) turns" } ?? "")")
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
