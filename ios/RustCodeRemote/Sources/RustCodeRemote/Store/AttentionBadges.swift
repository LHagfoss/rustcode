import Foundation

public enum AttentionBadge: Sendable, Equatable {
    case approval
    case question
    /// A turn finished while the session was not on screen.
    case completed
}

/// Foreground badges. Questions and approvals come from the host's session metadata;
/// "completed" is client state, remembered until the session is opened.
public struct AttentionBadges: Sendable, Equatable {
    public private(set) var unseenCompletions: Set<String> = []

    public init() {}

    /// The badge reducer: compares two consecutive session lists.
    public mutating func observe(previous: [SessionSummary], current: [SessionSummary], selected: String?) {
        let before = Dictionary(previous.map { ($0.id, $0) }, uniquingKeysWith: { $1 })
        for session in current where session.id != selected {
            if before[session.id]?.activity == .running, session.activity == .idle {
                unseenCompletions.insert(session.id)
            }
        }
        unseenCompletions.formIntersection(current.map(\.id))
    }

    public mutating func markSeen(_ sessionID: String) {
        unseenCompletions.remove(sessionID)
    }

    public mutating func removeAll() {
        unseenCompletions.removeAll()
    }

    /// The most urgent badge for a session: an approval blocks work, a question waits for
    /// input, a completion is only news.
    public func badge(for session: SessionSummary) -> AttentionBadge? {
        if session.attention.pendingApprovals > 0 { return .approval }
        if session.attention.pendingQuestions > 0 { return .question }
        if unseenCompletions.contains(session.id) { return .completed }
        return nil
    }

    public func count(in sessions: [SessionSummary]) -> Int {
        sessions.count { badge(for: $0) != nil }
    }
}
