import Foundation

/// Optional host-wide operations a gateway advertises with its session list.
public enum HostCapability {
    /// The host lists every saved session and can reopen one that nothing holds.
    public static let savedSessions = "saved_sessions"
}

/// Whether a saved session can be opened from the phone.
public enum SavedSessionState: Sendable, Equatable {
    /// Nothing holds it; the host can reopen it.
    case saved
    /// Already shared; it is in the live list.
    case shared
    /// A terminal, the app or a session owner on the host holds it without sharing it.
    case open
    /// A state from a newer host. Treated as not openable.
    case unknown
}

/// What has a saved session open on the host.
public enum SavedSessionHolder: Sendable, Equatable, Hashable {
    /// The RustCode app on the Mac.
    case app
    /// A RustCode terminal.
    case terminal
    /// A session owner the host started for a device.
    case remote
}

/// One session saved on the host, shared or not.
public struct SavedSession: Identifiable, Hashable, Sendable {
    public var id: String
    public var title: String
    /// Absent for a session that predates workspace recording.
    public var workspace: String?
    /// When its transcript was last written.
    public var updatedAt: Date?
    public var messageCount: Int
    public var state: SavedSessionState
    /// What has an `.open` session open, when the host says; `nil` from older hosts.
    public var openIn: SavedSessionHolder?

    public init(
        id: String,
        title: String,
        workspace: String? = nil,
        updatedAt: Date? = nil,
        messageCount: Int = 0,
        state: SavedSessionState = .saved,
        openIn: SavedSessionHolder? = nil
    ) {
        self.id = id
        self.title = title
        self.workspace = workspace
        self.updatedAt = updatedAt
        self.messageCount = messageCount
        self.state = state
        self.openIn = openIn
    }

    public var displayTitle: String {
        title.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? "Thread \(id.prefix(8))" : title
    }

    /// Where an `.open` session is open, ready to show under its title.
    public var openElsewhereDetail: String {
        switch openIn {
        case .app: "Open in the RustCode app on your Mac"
        case .terminal: "Open in a terminal on your Mac"
        case .remote, nil: "Open on your Mac"
        }
    }

    /// Only a session nothing holds, with a recorded workspace, can be reopened from here.
    public var canOpen: Bool { state == .saved && workspace?.isEmpty == false }
}

/// One page of saved sessions, newest first.
public struct SavedSessionPage: Sendable, Equatable {
    public var sessions: [SavedSession]
    /// Continues the listing; absent on the last page.
    public var nextCursor: String?

    public init(sessions: [SavedSession], nextCursor: String? = nil) {
        self.sessions = sessions
        self.nextCursor = nextCursor
    }
}
