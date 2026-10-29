import Foundation

/// The chats this engine has open, apart from the one on screen, and the
/// chats being opened. `DesktopModel` keeps the async side (the
/// `session/load` request, catching up on queued updates, checking the
/// engine was not replaced); this keeps the bookkeeping those races guard.
///
/// The engine refuses a second `session/load` of an open session, so
/// switching back to a chat restores its snapshot here instead, kept current
/// by its updates. A chat being opened collects its replay off screen and
/// shows it at once: applied line by line, every replayed chunk redrew the
/// whole chat. Loads are kept per chat: picking another chat mid-load used to
/// drop the replay and remember the chat as empty (blank until a restart).
public struct OpenSessions: Equatable, Sendable {
    /// What an open chat looked like when the user left it.
    public struct Snapshot: Equatable, Sendable {
        public var transcript: Transcript
        public var models: ModelOptions
        public var selectedModel: String?

        public init(transcript: Transcript, models: ModelOptions, selectedModel: String?) {
            self.transcript = transcript
            self.models = models
            self.selectedModel = selectedModel
        }
    }

    /// Where a chat's `session/update` goes.
    public enum Route: Equatable, Sendable {
        /// Into the replay of a chat being opened; already applied.
        case replay
        /// To the chat on screen; the caller applies it.
        case shown
        /// To a chat open in the background; already applied, except a
        /// notice or a stopped turn, which the caller shows instead.
        case background
        /// To a chat neither shown, open nor loading.
        case ignored
    }

    private var open: [String: Snapshot] = [:]
    private var replays: [String: Transcript] = [:]
    /// Chats being opened.
    public private(set) var loading: Set<String> = []

    public init() {}

    /// Chats open in the background.
    public var openIds: Dictionary<String, Snapshot>.Keys { open.keys }

    public func isOpen(_ id: String) -> Bool { open[id] != nil }
    public func isLoading(_ id: String) -> Bool { loading.contains(id) }
    public func snapshot(_ id: String) -> Snapshot? { open[id] }
    /// The replay collected so far for a chat being opened.
    public func replay(_ id: String) -> Transcript? { replays[id] }

    /// A `session/load` of `id` starts: its replay collects from empty.
    public mutating func beginLoad(_ id: String) {
        loading.insert(id)
        replays[id] = Transcript()
    }

    /// The load answered: the chat is open with its replay and models. The
    /// load stays marked until `endLoad`.
    public mutating func finishLoad(_ id: String, models: ModelOptions) {
        open[id] = Snapshot(transcript: replays[id] ?? Transcript(), models: models, selectedModel: models.current)
    }

    /// The load is over, answered or failed: drop its replay and mark.
    public mutating func endLoad(_ id: String) {
        replays[id] = nil
        loading.remove(id)
    }

    /// Keep the chat the user leaves. A chat still loading has no transcript
    /// yet (its load stores it), so it is skipped.
    public mutating func stash(_ id: String, transcript: Transcript, models: ModelOptions, selectedModel: String?) {
        guard !loading.contains(id) else { return }
        open[id] = Snapshot(transcript: transcript, models: models, selectedModel: selectedModel)
    }

    /// The chat is closed, moved or trashed: the next pick loads it again.
    public mutating func forget(_ id: String) {
        open[id] = nil
    }

    /// The engine stopped: nothing is open or loading in the next one.
    public mutating func reset() {
        self = OpenSessions()
    }

    /// Changes an open background chat's transcript; false when not open.
    @discardableResult
    public mutating func updateTranscript(of id: String, _ body: (inout Transcript) -> Void) -> Bool {
        guard open[id] != nil else { return false }
        body(&open[id]!.transcript)
        return true
    }

    /// Where `update` for `sessionId` goes, applying it to a replay or a
    /// background chat. A report answer, a notice and a stopped turn never go
    /// into a replay, and a notice or stopped turn not into a background chat.
    public mutating func route(_ update: SessionUpdate, isReport: Bool = false, in sessionId: String,
                               selected: String?, at now: Date = Date()) -> Route {
        let alert: Bool
        switch update {
        case .notice, .stopped: alert = true
        default: alert = false
        }
        if replays[sessionId] != nil, !isReport, !alert {
            replays[sessionId]?.apply(update)
            return .replay
        }
        if sessionId == selected { return .shown }
        guard open[sessionId] != nil else { return .ignored }
        if !alert { open[sessionId]!.transcript.apply(update, at: now) }
        return .background
    }
}
