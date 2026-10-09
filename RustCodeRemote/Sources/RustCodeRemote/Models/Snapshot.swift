import Foundation

public enum TurnOutcome: Sendable, Equatable {
    case completed
    case cancelled
    case failed
}

/// One change to a shared session, already decoded into domain terms.
public enum SessionEvent: Sendable, Equatable {
    case itemAppended(TranscriptItem)
    /// Replaces the item with the same ID (tool status, subagent progress).
    case itemUpdated(TranscriptItem)
    /// Appends text to an assistant item, creating it if this is its first delta.
    case assistantDelta(itemID: String, text: String)
    case assistantCompleted(itemID: String)
    /// Text of the running response, with no item named: it continues the assistant item
    /// at the end of the transcript, or starts one.
    case responseDelta(String)
    /// The result of a tool whose row is already in the transcript.
    case toolFinished(itemID: String, succeeded: Bool, isPending: Bool, output: String, isOutputTruncated: Bool)
    /// A sub-agent of the session changed; the item's ID identifies it.
    case subagentUpdated(TranscriptItem)
    case turnStarted(turnID: String)
    /// A nil `turnID` ends whichever turn is running.
    case turnFinished(turnID: String?, outcome: TurnOutcome)
    case questionChanged(PendingQuestion?)
    case approvalChanged(PendingApproval?)
    /// The named question left the pending state, whoever answered it.
    case questionResolved(questionID: String)
    case approvalResolved(batchID: String)
    /// Replaces all state for the session.
    case replaced(SessionSnapshot)
}

public struct SessionUpdate: Sendable, Equatable {
    public var session: SessionRef
    /// Monotonic per registration. Exactly one greater than the previous applied update.
    public var sequence: UInt64
    /// What the update changes, applied together. Empty for an event this version of the
    /// app does not understand: it still occupies its place in the sequence.
    public var changes: [SessionEvent]

    public init(session: SessionRef, sequence: UInt64, changes: [SessionEvent]) {
        self.session = session
        self.sequence = sequence
        self.changes = changes
    }

    public init(session: SessionRef, sequence: UInt64, event: SessionEvent) {
        self.init(session: session, sequence: sequence, changes: [event])
    }
}

/// The authoritative state of one attached session: a bounded transcript tail, the complete
/// current attention state and the sequence watermark that state represents.
public struct SessionSnapshot: Sendable, Equatable {
    public var session: SessionRef
    public var items: [TranscriptItem]
    public var activeTurnID: String?
    /// Whether the running turn accepts steering. Only a snapshot says so.
    public var canSteer: Bool
    public var pendingQuestion: PendingQuestion?
    public var pendingApproval: PendingApproval?
    /// Sub-agents of the session, as `.subagent` items.
    public var subagents: [TranscriptItem]
    public var backgroundTasks: [BackgroundTask]
    public var pendingPrompts: [PendingPrompt]
    public var omitted: OmittedCounts
    public var sequence: UInt64
    /// Cursor for the page before `items`; nil when the snapshot already starts at the beginning.
    public var historyCursor: HistoryCursor?

    public init(
        session: SessionRef,
        items: [TranscriptItem] = [],
        activeTurnID: String? = nil,
        canSteer: Bool = true,
        pendingQuestion: PendingQuestion? = nil,
        pendingApproval: PendingApproval? = nil,
        subagents: [TranscriptItem] = [],
        backgroundTasks: [BackgroundTask] = [],
        pendingPrompts: [PendingPrompt] = [],
        omitted: OmittedCounts = OmittedCounts(),
        sequence: UInt64 = 0,
        historyCursor: HistoryCursor? = nil
    ) {
        self.session = session
        self.items = items
        self.activeTurnID = activeTurnID
        self.canSteer = canSteer
        self.pendingQuestion = pendingQuestion
        self.pendingApproval = pendingApproval
        self.subagents = subagents
        self.backgroundTasks = backgroundTasks
        self.pendingPrompts = pendingPrompts
        self.omitted = omitted
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
        for change in update.changes {
            apply(change, sequence: update.sequence)
        }
        sequence = update.sequence
        return .applied
    }

    private mutating func apply(_ event: SessionEvent, sequence: UInt64) {
        switch event {
        case .itemAppended(let item), .itemUpdated(let item):
            upsert(item)
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
        case .responseDelta(let text):
            if let last = items.indices.last, case .assistant(var assistant) = items[last].content,
               assistant.isStreaming {
                assistant.text += text
                items[last].content = .assistant(assistant)
            } else {
                items.append(TranscriptItem(
                    id: "response:\(sequence)",
                    .assistant(AssistantText(text: text, isStreaming: true))
                ))
            }
        case .toolFinished(let itemID, let succeeded, let isPending, let output, let isOutputTruncated):
            let index = items.lastIndex { $0.id == itemID }
            var tool = ToolCall(name: "tool", summary: "", status: .running)
            if let index, case .tool(let existing) = items[index].content {
                tool = existing
            }
            // A tool that handed off to a background task has no result yet.
            tool.status = isPending ? .running : succeeded ? .succeeded : .failed
            tool.output = output.isEmpty ? nil : output
            tool.isOutputTruncated = isOutputTruncated
            upsert(TranscriptItem(id: itemID, .tool(tool)))
        case .subagentUpdated(let item):
            if let index = subagents.firstIndex(where: { $0.id == item.id }) {
                subagents[index] = item
            } else {
                subagents.append(item)
            }
        case .turnStarted(let turnID):
            activeTurnID = turnID
        case .turnFinished(let turnID, _):
            if turnID == nil || activeTurnID == turnID { activeTurnID = nil }
            finishStreaming { _ in true }
        case .questionChanged(let question):
            pendingQuestion = question
        case .approvalChanged(let approval):
            pendingApproval = approval
        case .questionResolved(let questionID):
            if pendingQuestion?.id == questionID { pendingQuestion = nil }
        case .approvalResolved(let batchID):
            if pendingApproval?.batchID == batchID { pendingApproval = nil }
        case .replaced(let snapshot):
            let session = session
            self = snapshot
            self.session = session
        }
    }

    /// Inserts an older page in front of the transcript, skipping items already present.
    public mutating func prepend(_ page: HistoryPage) {
        let known = Set(items.map(\.id))
        items.insert(contentsOf: page.items.filter { !known.contains($0.id) }, at: 0)
        historyCursor = page.nextCursor
    }

    /// Replaces the item with the same ID or appends a new one. A new item ends the
    /// response text before it: the turn has moved on to something else.
    private mutating func upsert(_ item: TranscriptItem) {
        if let index = items.lastIndex(where: { $0.id == item.id }) {
            items[index] = item
        } else {
            finishStreaming { _ in true }
            items.append(item)
        }
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
