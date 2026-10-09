import Foundation

/// A live shared session is identified by its ID and the epoch of its current registration.
/// A new registration of the same session is a different identity.
public struct SessionRef: Hashable, Sendable {
    public var sessionID: String
    public var registrationEpoch: UInt64

    public init(sessionID: String, registrationEpoch: UInt64) {
        self.sessionID = sessionID
        self.registrationEpoch = registrationEpoch
    }
}

public enum SessionActivity: Sendable, Equatable {
    case idle
    case running
    case waitingForUser
}

public enum SessionHealth: Sendable, Equatable {
    case healthy
    case degraded
    case lost
}

/// Attention the host reports for a session.
public struct SessionAttention: Hashable, Sendable {
    public var pendingQuestions: Int
    public var pendingApprovals: Int

    public init(pendingQuestions: Int = 0, pendingApprovals: Int = 0) {
        self.pendingQuestions = pendingQuestions
        self.pendingApprovals = pendingApprovals
    }

    public var isEmpty: Bool { pendingQuestions == 0 && pendingApprovals == 0 }
}

public struct SessionSummary: Identifiable, Hashable, Sendable {
    public var ref: SessionRef
    public var title: String
    public var workspace: String
    public var model: String
    public var activity: SessionActivity
    public var attention: SessionAttention
    public var health: SessionHealth

    public var id: String { ref.sessionID }

    public init(
        ref: SessionRef,
        title: String,
        workspace: String,
        model: String,
        activity: SessionActivity = .idle,
        attention: SessionAttention = SessionAttention(),
        health: SessionHealth = .healthy
    ) {
        self.ref = ref
        self.title = title
        self.workspace = workspace
        self.model = model
        self.activity = activity
        self.attention = attention
        self.health = health
    }
}

// MARK: - Transcript

public struct TranscriptItem: Identifiable, Hashable, Sendable {
    public var id: String
    public var content: Content

    public enum Content: Hashable, Sendable {
        case user(String)
        case assistant(AssistantText)
        case tool(ToolCall)
        case subagent(SubagentSummary)
        case notice(String)
    }

    public init(id: String, _ content: Content) {
        self.id = id
        self.content = content
    }
}

public struct AssistantText: Hashable, Sendable {
    public var text: String
    public var isStreaming: Bool

    public init(text: String, isStreaming: Bool = false) {
        self.text = text
        self.isStreaming = isStreaming
    }
}

public struct ToolCall: Hashable, Sendable {
    public enum Status: Sendable, Equatable {
        case running
        case succeeded
        case failed
    }

    public var name: String
    public var summary: String
    public var input: String?
    public var output: String?
    public var status: Status
    /// The host bounded the output; the rest has to be fetched explicitly.
    public var isOutputTruncated: Bool

    public init(
        name: String,
        summary: String,
        input: String? = nil,
        output: String? = nil,
        status: Status,
        isOutputTruncated: Bool = false
    ) {
        self.name = name
        self.summary = summary
        self.input = input
        self.output = output
        self.status = status
        self.isOutputTruncated = isOutputTruncated
    }
}

public struct SubagentSummary: Hashable, Sendable {
    public enum Status: Sendable, Equatable {
        case queued
        case running
        case interrupted
        case completed
        case failed
        case cancelled
    }

    public var name: String
    public var task: String
    public var status: Status
    public var detail: String?

    public init(name: String, task: String, status: Status, detail: String? = nil) {
        self.name = name
        self.task = task
        self.status = status
        self.detail = detail
    }
}

// MARK: - Attention

public struct PendingQuestion: Identifiable, Hashable, Sendable {
    public struct Option: Identifiable, Hashable, Sendable {
        public var id: String
        public var label: String
        public var detail: String?

        public init(id: String, label: String, detail: String? = nil) {
            self.id = id
            self.label = label
            self.detail = detail
        }
    }

    public var id: String
    /// Short topic label, shown above the prompt. May be empty.
    public var header: String
    public var prompt: String
    public var options: [Option]
    public var allowsMultipleSelection: Bool
    public var allowsFreeText: Bool
    /// 1-based position within a chain of questions asked together.
    public var position: Int
    public var chainLength: Int

    public init(
        id: String,
        header: String = "",
        prompt: String,
        options: [Option],
        allowsMultipleSelection: Bool = false,
        allowsFreeText: Bool = false,
        position: Int = 1,
        chainLength: Int = 1
    ) {
        self.id = id
        self.header = header
        self.prompt = prompt
        self.options = options
        self.allowsMultipleSelection = allowsMultipleSelection
        self.allowsFreeText = allowsFreeText
        self.position = position
        self.chainLength = chainLength
    }
}

public struct QuestionAnswer: Hashable, Sendable {
    public var selectedOptionIDs: [String]
    public var text: String?

    public init(selectedOptionIDs: [String] = [], text: String? = nil) {
        self.selectedOptionIDs = selectedOptionIDs
        self.text = text
    }

    public var isEmpty: Bool {
        selectedOptionIDs.isEmpty && (text ?? "").trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }
}

/// One controller approval batch. It is approved or denied as a whole, by its exact batch ID.
public struct PendingApproval: Identifiable, Hashable, Sendable {
    public struct Item: Identifiable, Hashable, Sendable {
        public var id: String
        public var toolName: String
        public var title: String
        /// The host's note on why this needs approval. May be empty.
        public var risk: String
        public var detail: String
        /// False while `detail` is only a bounded preview. A preview is not the reviewed action.
        public var isDetailComplete: Bool
        /// Names the full detail for `getContent`. Nil when the rest cannot be fetched, in
        /// which case a truncated item can be denied but never approved from here.
        public var detailContentID: String?
        public var detailTotalBytes: UInt64?

        public init(
            id: String,
            toolName: String,
            title: String,
            risk: String = "",
            detail: String,
            isDetailComplete: Bool = true,
            detailContentID: String? = nil,
            detailTotalBytes: UInt64? = nil
        ) {
            self.id = id
            self.toolName = toolName
            self.title = title
            self.risk = risk
            self.detail = detail
            self.isDetailComplete = isDetailComplete
            self.detailContentID = detailContentID
            self.detailTotalBytes = detailTotalBytes
        }
    }

    public var batchID: String
    public var items: [Item]

    public var id: String { batchID }

    public init(batchID: String, items: [Item]) {
        self.batchID = batchID
        self.items = items
    }

    /// Approving is only allowed once every item's full detail has been fetched.
    public var canBeResolved: Bool { items.allSatisfy(\.isDetailComplete) }
}

public enum ApprovalDecision: Sendable, Equatable {
    case approve
    case deny
}

// MARK: - Turn activity

/// A command the session left running in the background.
public struct BackgroundTask: Identifiable, Hashable, Sendable {
    public var id: String
    public var command: String
    public var elapsed: Duration

    public init(id: String, command: String, elapsed: Duration) {
        self.id = id
        self.command = command
        self.elapsed = elapsed
    }
}

/// A prompt the session holds until it can act on it.
public struct PendingPrompt: Hashable, Sendable {
    public enum Kind: Sendable, Equatable {
        case steer
        case queue
    }

    public var kind: Kind
    public var text: String

    public init(kind: Kind, text: String) {
        self.kind = kind
        self.text = text
    }
}

/// Entries the host left out of a snapshot's lists to keep the frame bounded.
public struct OmittedCounts: Hashable, Sendable {
    public var tools: Int
    public var subagents: Int
    public var pendingPrompts: Int
    public var backgroundTasks: Int

    public init(tools: Int = 0, subagents: Int = 0, pendingPrompts: Int = 0, backgroundTasks: Int = 0) {
        self.tools = tools
        self.subagents = subagents
        self.pendingPrompts = pendingPrompts
        self.backgroundTasks = backgroundTasks
    }

    public var total: Int { tools + subagents + pendingPrompts + backgroundTasks }
}

/// One piece of content that was truncated in another frame.
public struct ContentChunk: Sendable, Equatable {
    public var contentID: String
    public var offset: UInt64
    public var text: String
    public var totalBytes: UInt64
    /// Nil at the end of the content.
    public var nextOffset: UInt64?

    public init(contentID: String, offset: UInt64, text: String, totalBytes: UInt64, nextOffset: UInt64?) {
        self.contentID = contentID
        self.offset = offset
        self.text = text
        self.totalBytes = totalBytes
        self.nextOffset = nextOffset
    }
}

// MARK: - History

/// Opaque to the client: produced and interpreted by the host.
public struct HistoryCursor: Hashable, Sendable {
    public var rawValue: String

    public init(_ rawValue: String) {
        self.rawValue = rawValue
    }
}

public struct HistoryPage: Sendable, Equatable {
    /// Oldest first.
    public var items: [TranscriptItem]
    /// Nil when there is nothing older.
    public var nextCursor: HistoryCursor?

    public init(items: [TranscriptItem], nextCursor: HistoryCursor?) {
        self.items = items
        self.nextCursor = nextCursor
    }
}
