import Foundation

public enum TurnOutcome: Sendable, Equatable {
    case completed
    case cancelled
    case failed
}

/// One applied change to a shared session, already decoded into domain terms.
public enum SessionEvent: Sendable, Equatable {
    case itemAppended(TranscriptItem)
    /// Replaces the item with the same ID (tool status, subagent progress).
    case itemUpdated(TranscriptItem)
    /// Appends text to an assistant item, creating it if this is its first delta.
    case assistantDelta(itemID: String, text: String)
    case assistantCompleted(itemID: String)
    case turnStarted(turnID: String)
    case turnFinished(turnID: String, outcome: TurnOutcome)
    case questionChanged(PendingQuestion?)
    case approvalChanged(PendingApproval?)
}

public struct SessionUpdate: Sendable, Equatable {
    public var session: SessionRef
    /// Monotonic per registration. Exactly one greater than the previous applied update.
    public var sequence: UInt64
    public var event: SessionEvent

    public init(session: SessionRef, sequence: UInt64, event: SessionEvent) {
        self.session = session
        self.sequence = sequence
        self.event = event
    }
}

/// The authoritative state of one attached session: a bounded transcript tail, the complete
/// current attention state and the sequence watermark that state represents.
public struct SessionSnapshot: Sendable, Equatable {
    public var session: SessionRef
    public var items: [TranscriptItem]
    public var activeTurnID: String?
    public var pendingQuestion: PendingQuestion?
    public var pendingApproval: PendingApproval?
    public var sequence: UInt64
    /// Cursor for the page before `items`; nil when the snapshot already starts at the beginning.
    public var historyCursor: HistoryCursor?

    public init(
        session: SessionRef,
        items: [TranscriptItem] = [],
        activeTurnID: String? = nil,
        pendingQuestion: PendingQuestion? = nil,
        pendingApproval: PendingApproval? = nil,
        sequence: UInt64 = 0,
        historyCursor: HistoryCursor? = nil
    ) {
        self.session = session
        self.items = items
        self.activeTurnID = activeTurnID
        self.pendingQuestion = pendingQuestion
        self.pendingApproval = pendingApproval
        self.sequence = sequence
        self.historyCursor = historyCursor
    }

    public var isRunning: Bool { activeTurnID != nil }

    public enum ApplyOutcome: Sendable, Equatable {
        case applied
        /// Already represented in the snapshot; ignored.
        case duplicate
        /// Updates were lost. The snapshot is unchanged and must be replaced by a fresh one.
        case gap(expected: UInt64, received: UInt64)
        /// Belongs to another session or registration; ignored.
        case foreign
    }

    /// The transcript reducer. Applies an update only when it is the next one in sequence,
    /// so a delta that the snapshot already contains is never appended twice.
    @discardableResult
    public mutating func apply(_ update: SessionUpdate) -> ApplyOutcome {
        guard update.session == session else { return .foreign }
        guard update.sequence > sequence else { return .duplicate }
        guard update.sequence == sequence + 1 else {
            return .gap(expected: sequence + 1, received: update.sequence)
        }
        sequence = update.sequence

        switch update.event {
        case .itemAppended(let item):
            if let index = items.lastIndex(where: { $0.id == item.id }) {
                items[index] = item
            } else {
                items.append(item)
            }
        case .itemUpdated(let item):
            if let index = items.lastIndex(where: { $0.id == item.id }) {
                items[index] = item
            } else {
                items.append(item)
            }
        case .assistantDelta(let itemID, let text):
            if let index = items.lastIndex(where: { $0.id == itemID }),
               case .assistant(var assistant) = items[index].content {
                assistant.text += text
                assistant.isStreaming = true
                items[index].content = .assistant(assistant)
            } else {
                items.append(TranscriptItem(id: itemID, .assistant(AssistantText(text: text, isStreaming: true))))
            }
        case .assistantCompleted(let itemID):
            finishStreaming { $0.id == itemID }
        case .turnStarted(let turnID):
            activeTurnID = turnID
        case .turnFinished(let turnID, _):
            if activeTurnID == turnID { activeTurnID = nil }
            finishStreaming { _ in true }
        case .questionChanged(let question):
            pendingQuestion = question
        case .approvalChanged(let approval):
            pendingApproval = approval
        }
        return .applied
    }

    /// Inserts an older page in front of the transcript, skipping items already present.
    public mutating func prepend(_ page: HistoryPage) {
        let known = Set(items.map(\.id))
        items.insert(contentsOf: page.items.filter { !known.contains($0.id) }, at: 0)
        historyCursor = page.nextCursor
    }

    private mutating func finishStreaming(where matches: (TranscriptItem) -> Bool) {
        for index in items.indices where matches(items[index]) {
            if case .assistant(var assistant) = items[index].content, assistant.isStreaming {
                assistant.isStreaming = false
                items[index].content = .assistant(assistant)
            }
        }
    }
}
