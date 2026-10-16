/// A discrete live event, separate from snapshots and text deltas. The revision lets
/// repeated events of the same kind each produce feedback once.
public struct ChatFeedback: Sendable, Equatable {
    public enum Kind: Sendable, Equatable {
        case started, completed, cancelled, failed, attention
    }

    public var session: SessionRef
    public var revision: UInt64
    public var kind: Kind

    public init(session: SessionRef, revision: UInt64, kind: Kind) {
        self.session = session
        self.revision = revision
        self.kind = kind
    }

    public static func kind(for update: SessionUpdate, previous: SessionSnapshot) -> Kind? {
        guard update.session == previous.session, !update.isSnapshot,
              previous.sequence < UInt64.max, update.sequence == previous.sequence + 1 else { return nil }
        // Prefer the last meaningful event in a batch. Never buzz for streamed text,
        // history loading, duplicate updates or reconnect snapshots.
        for event in update.changes.reversed() {
            switch event {
            case .turnStarted(let id) where id != previous.activeTurnID:
                return .started
            case .turnFinished(let id, let outcome) where previous.activeTurnID != nil
                && (id == nil || id == previous.activeTurnID):
                switch outcome {
                case .completed: return .completed
                case .cancelled: return .cancelled
                case .failed: return .failed
                case .unknown: return nil
                }
            case .questionChanged(let question) where question != nil && question?.id != previous.pendingQuestion?.id:
                return .attention
            case .approvalChanged(let approval) where approval != nil && approval?.batchID != previous.pendingApproval?.batchID:
                return .attention
            default:
                continue
            }
        }
        return nil
    }
}
