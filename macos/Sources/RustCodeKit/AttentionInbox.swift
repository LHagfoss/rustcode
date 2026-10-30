import Foundation

/// One thing across all threads that wants the reader (#2636): an approval
/// or question waiting for an answer, a turn that stopped, or a reply not yet
/// read. Entries are derived from what the app has observed; the inbox never
/// answers or resolves anything itself. An approval or question leaves only
/// when it is answered (in its thread, where the existing cards stay
/// authoritative) or its turn ends.
public struct AttentionEntry: Identifiable, Equatable, Sendable {
    public enum Kind: Equatable, Sendable {
        /// A tool call waits for approval.
        case approval(title: String)
        /// The agent asked a question.
        case question(text: String)
        /// The turn stopped (provider error, connection, loop guard) and can
        /// be continued.
        case stopped(reason: String)
        /// The turn finished while another thread was on screen.
        case finished
    }

    public var id: String
    public var sessionId: String
    public var kind: Kind
    /// When the app first saw it.
    public var since: Date

    public init(id: String, sessionId: String, kind: Kind, since: Date) {
        self.id = id
        self.sessionId = sessionId
        self.kind = kind
        self.since = since
    }

    /// Waits for an answer: it cannot be dismissed, only answered.
    public var isUnresolved: Bool {
        switch kind {
        case .approval, .question: true
        case .stopped, .finished: false
        }
    }

    /// A notification the reader may clear without acting on it.
    public var isDismissible: Bool { !isUnresolved }

    /// What the entry is about, in a few words.
    public var reason: String {
        switch kind {
        case .approval(let title): "Approve: \(title)"
        case .question(let text): "Question: \(text)"
        case .stopped(let reason): reason
        case .finished: "Finished"
        }
    }
}

/// The inbox: entries in reading order, and the first-seen clock that gives
/// each its age.
public struct AttentionInbox: Equatable, Sendable {
    /// A pending approval or question, as the inbox needs it.
    public struct Request: Equatable, Sendable {
        public var id: String
        public var sessionId: String
        /// Set for a question; nil for a tool approval.
        public var question: String?
        public var title: String

        public init(id: String, sessionId: String, question: String?, title: String) {
            self.id = id
            self.sessionId = sessionId
            self.question = question
            self.title = title
        }
    }

    /// When each entry id was first observed. Kept across refreshes so an
    /// entry's age does not reset; ids no longer present are dropped.
    public private(set) var firstSeen: [String: Date] = [:]
    /// Dismissed notification ids, until the thing they describe goes away.
    public private(set) var dismissed: Set<String> = []

    public init() {}

    static func stoppedID(_ session: String, _ reason: String) -> String { "stopped:\(session):\(reason)" }
    static func finishedID(_ session: String) -> String { "finished:\(session)" }

    /// The current entries, unresolved first (oldest first: the longest
    /// wait leads), then notifications newest first. Threads not in `known`
    /// (deleted, archived, trashed) are left out.
    public mutating func entries(
        requests: [Request],
        stopped: [String: String],
        unseen: Set<String>,
        known: Set<String>,
        now: Date
    ) -> [AttentionEntry] {
        var current: [AttentionEntry] = []
        for request in requests where known.contains(request.sessionId) {
            let kind: AttentionEntry.Kind = request.question.map { .question(text: $0) } ?? .approval(title: request.title)
            current.append(AttentionEntry(id: "request:\(request.id)", sessionId: request.sessionId, kind: kind, since: now))
        }
        for (session, reason) in stopped where known.contains(session) {
            current.append(AttentionEntry(id: Self.stoppedID(session, reason), sessionId: session,
                                          kind: .stopped(reason: reason), since: now))
        }
        // A stopped thread also counts as unseen; one entry says it.
        for session in unseen where known.contains(session) && stopped[session] == nil {
            current.append(AttentionEntry(id: Self.finishedID(session), sessionId: session, kind: .finished, since: now))
        }
        let ids = Set(current.map(\.id))
        firstSeen = firstSeen.filter { ids.contains($0.key) }
        dismissed = dismissed.intersection(ids)
        for index in current.indices {
            let id = current[index].id
            current[index].since = firstSeen[id] ?? now
            firstSeen[id] = current[index].since
        }
        let unresolved = current.filter(\.isUnresolved).sorted { ($0.since, $0.id) < ($1.since, $1.id) }
        let notices = current.filter { !$0.isUnresolved && !dismissed.contains($0.id) }
            .sorted { ($0.since, $0.id) > ($1.since, $1.id) }
        return unresolved + notices
    }

    /// Clear a notification. An approval or question cannot be dismissed.
    public mutating func dismiss(_ entry: AttentionEntry) {
        guard entry.isDismissible else { return }
        dismissed.insert(entry.id)
    }
}
