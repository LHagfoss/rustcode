import RustCodeRemote
import SwiftUI

struct SessionsView: View {
    @Environment(RemoteStore.self) private var store
    @State private var showsSettings = LaunchOptions.current.presentsSettings
    var presentsAttentionOnLaunch = false

    var body: some View {
        NavigationSplitView {
            SessionSidebar(showsSettings: $showsSettings)
        } detail: {
            if store.selectedSessionID != nil {
                TranscriptView(presentsAttentionOnAppear: presentsAttentionOnLaunch)
            } else {
                ContentUnavailableView(
                    "No Session Selected",
                    systemImage: "bubble.left.and.text.bubble.right",
                    description: Text("Choose a shared session to watch it and continue it from here.")
                )
            }
        }
        .sheet(isPresented: $showsSettings) {
            SettingsView()
        }
    }
}

private struct SessionSidebar: View {
    @Environment(RemoteStore.self) private var store
    @Binding var showsSettings: Bool
    @State private var query = ""

    private var sessions: [SessionSummary] {
        let query = query.trimmingCharacters(in: .whitespaces)
        guard !query.isEmpty else { return store.sessions }
        return store.sessions.filter { session in
            [session.title, session.workspace, session.model].contains { $0.localizedStandardContains(query) }
        }
    }

    private var selection: Binding<String?> {
        Binding(get: { store.selectedSessionID }, set: { store.select($0) })
    }

    var body: some View {
        List(selection: selection) {
            Section {
                ForEach(sessions) { session in
                    SessionRow(session: session, badge: store.badges.badge(for: session))
                        .tag(session.id)
                        .contextMenu {
                            Button("Copy Title", systemImage: "doc.on.doc") {
                                Clipboard.copy(session.title)
                            }
                            Button("Copy Workspace Path", systemImage: "folder") {
                                Clipboard.copy(session.workspace)
                            }
                        }
                }
            } footer: {
                if !sessions.isEmpty {
                    Text("Sessions appear here while they are shared with /\u{2060}remote and their terminal stays open.")
                }
            }
        }
        .overlay {
            if store.sessions.isEmpty {
                emptyState
            } else if sessions.isEmpty {
                ContentUnavailableView.search(text: query)
            }
        }
        .searchable(text: $query, prompt: "Search sessions")
        .navigationTitle("Sessions")
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button("Settings", systemImage: "gearshape") {
                    showsSettings = true
                }
            }
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
                "No Shared Sessions",
                systemImage: "terminal",
                description: Text("Connected to \(store.host?.name ?? "the host"). Sessions appear here once they are shared with /\u{2060}remote.")
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
                Text(session.title)
                    .font(.body.weight(.medium))
                    .lineLimit(2)
                Text("\(session.workspace) · \(session.model)")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            Spacer(minLength: 8)
            indicator
        }
        .padding(.vertical, 4)
        .accessibilityElement(children: .combine)
        .accessibilityLabel("\(session.title), \(statusDescription)")
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
