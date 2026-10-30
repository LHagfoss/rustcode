import Foundation

/// A tool call as the transcript shows it: one card that later updates fill in.
public struct ToolItem: Equatable, Sendable {
    public var callId: String
    public var title: String
    public var kind: String?
    public var status: ToolStatus?
    public var input: String?
    public var output: String?
    /// One line saying what the call acted on, taken from its arguments.
    public var inputDetail: String?
    /// What the model said the call does (its `summary` argument).
    public var summary: String?
    /// What the call changed, one entry per edited file.
    public var changes: [FileChange] = []
    /// The call never reported back: the turn ended and closed it. Its
    /// status is how the card shows it, not an outcome (#2649).
    public var settledByTurnEnd = false
    /// A finished command's exit code, when the engine sent it.
    public var exitCode: Int?

    public var displayTitle: String { ToolPresentation.title(title) }
    public var detail: String? { inputDetail ?? ToolPresentation.detail(input: nil, output: output) }
    /// Still running yet with nothing to show: no detail, input, output or
    /// changes. Such a card is only a spinner, so the chat leaves it out.
    public var isBlank: Bool {
        !(status?.isTerminal ?? true) && inputDetail == nil && input == nil && output == nil && changes.isEmpty
    }
}

public struct TranscriptItem: Identifiable, Equatable, Sendable {
    public enum Content: Equatable, Sendable {
        case user(String)
        case assistant(String)
        case thinking(String)
        case tool(ToolItem)
        /// A host slash command the user ran and the engine's answer to it.
        case command(input: String, output: String)
        /// The history was compacted here. What came before stays above it
        /// in the chat; the model now works from the card's summary.
        case compaction(CompactionCard)
    }

    public let id: Int
    public var content: Content
    /// When a live thought began, and how long it ran once the next item
    /// arrived or the turn ended. Replayed history has neither.
    public var startedAt: Date?
    public var duration: TimeInterval?
    /// A thought that is the provider's reasoning summary (OpenAI shows only
    /// that of its hidden reasoning), not the model's own reasoning text.
    public var isReasoningSummary = false
    /// On a turn's last reply: how long the turn took and when it ended.
    public var turn: TurnTiming?
    /// When a user's message was sent, or a reply finished being written.
    /// Nil while a reply streams, and for history the engine sent no time for.
    public var at: Date?

    /// Everything a find in the chat can match.
    public var searchableText: String {
        switch content {
        case .user(let text), .assistant(let text), .thinking(let text): text
        case .tool(let tool): [tool.title, tool.input, tool.output].compactMap { $0 }.joined(separator: "\n")
        case .command(let input, let output): input + "\n" + output
        case .compaction(let card): [card.headline, card.summary].compactMap { $0 }.joined(separator: "\n")
        }
    }

    /// The header of a thought. A provider's summary says so, live or
    /// replayed, rather than claiming to be the model's thinking; reasoning
    /// text says how long it took when that is known.
    public static func thoughtTitle(isLive: Bool, isSummary: Bool, duration: TimeInterval?) -> String {
        if isLive { return "Thinking…" }
        if isSummary { return "Reasoning summary" }
        if let duration { return thoughtLabel(duration: duration) }
        return "Thinking"
    }

    /// "Thought for 12s", the header of a finished thought.
    public static func thoughtLabel(duration: TimeInterval) -> String {
        let seconds = max(Int(duration.rounded()), 1)
        if seconds < 60 { return "Thought for \(seconds)s" }
        let minutes = seconds / 60
        return seconds % 60 == 0 ? "Thought for \(minutes)m" : "Thought for \(minutes)m \(seconds % 60)s"
    }
}

/// The rendered conversation of one session. Streamed chunks join the item
/// they continue, and tool updates change the card of their call in place.
public struct Transcript: Equatable, Sendable {
    public private(set) var items: [TranscriptItem] = []
    /// The session's latest context and token usage; it travels with the
    /// transcript so each chat keeps its own while another is shown.
    public private(set) var usage: SessionUsage?
    private var nextId = 0
    /// Which conversation this is. Item ids count from 0 in every chat, so a
    /// view showing another chat must key on this too: keyed on ids alone,
    /// the chat kept the last chat's row heights and drew nothing until scrolled.
    public private(set) var identity = UUID()
    /// When the running turn began, for its live clock; nil between turns.
    public private(set) var turnStartedAt: Date?
    /// The first item the running turn may have added.
    private var turnFirstId: Int?
    /// A reply that arrived live and has no finish time yet. Replayed
    /// replies never get one made up.
    private var liveReplyId: Int?

    public init() {}

    public var isEmpty: Bool { items.isEmpty }

    /// Ids of the items containing `query`, ignoring case and diacritics, in order.
    public func matches(_ query: String) -> [Int] {
        ChatFind.matches(in: items, query: query)
    }

    public mutating func removeAll() {
        items.removeAll()
        usage = nil
        identity = UUID()
        turnStartedAt = nil
        turnFirstId = nil
        liveReplyId = nil
    }

    /// A turn starts at `now`. A message handed to a running turn does not
    /// restart its clock.
    public mutating func beginTurn(at now: Date) {
        guard turnStartedAt == nil else { return }
        turnStartedAt = now
        turnFirstId = nextId
    }

    /// A prompt the user sent at `at`. The engine echoes user messages only
    /// when it replays a session, so the client adds its own.
    public mutating func appendUser(_ text: String, at: Date? = nil) {
        append(.user(text))
        items[items.count - 1].at = at
    }

    /// Take back the latest message the user sent as `text`, as a steer the
    /// turn ended before reading, which is sent again as its own turn.
    public mutating func removeLastUser(_ text: String) {
        guard let index = items.lastIndex(where: { if case .user(text) = $0.content { true } else { false } }) else { return }
        items.remove(at: index)
    }

    /// Which of the user's messages item `id` is, counting from 0, as the
    /// engine counts them for `_rustcode/rewind`.
    public func userIndex(of id: Int) -> Int? {
        guard let position = items.firstIndex(where: { $0.id == id }),
              case .user = items[position].content else { return nil }
        return items[..<position].filter { if case .user = $0.content { true } else { false } }.count
    }

    /// How many of the user's messages follow item `id`.
    public func userMessages(after id: Int) -> Int {
        guard let position = items.firstIndex(where: { $0.id == id }) else { return 0 }
        return items[(position + 1)...].filter { if case .user = $0.content { true } else { false } }.count
    }

    /// Drop item `id` and everything after it, as a retry rewinds the chat.
    public mutating func removeFrom(_ id: Int) {
        guard let position = items.firstIndex(where: { $0.id == id }) else { return }
        items.removeSubrange(position...)
    }

    /// A host command the user sent; the engine's reply fills it in.
    public mutating func appendCommand(_ input: String) {
        append(.command(input: input, output: ""))
    }

    /// `now` is when a live update arrived, so a thought can say how long it
    /// took; a replayed session passes nil.
    public mutating func apply(_ update: SessionUpdate, at now: Date? = nil) {
        switch update {
        case .userText(let text, let at):
            append(.user(text), at: now)
            items[items.count - 1].at = at ?? now
        case .agentText(let text, let turn, let at):
            if case .command(let input, let output) = items.last?.content {
                items[items.count - 1].content = .command(input: input, output: output + text)
            } else if case .assistant(let previous) = items.last?.content {
                items[items.count - 1].content = .assistant(previous + text)
            } else {
                append(.assistant(text), at: now)
            }
            if case .assistant = items.last?.content {
                if let turn { stamp(turn, on: items.count - 1) }
                if let at { items[items.count - 1].at = at }
            }
        case .thoughtText(let text, let summary, let durationMs):
            if case .thinking(let previous) = items.last?.content, items[items.count - 1].isReasoningSummary == summary {
                items[items.count - 1].content = .thinking(previous + text)
            } else {
                // Whitespace alone starts no thought: its row would only say "Thinking".
                guard !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
                append(.thinking(text), at: now)
                items[items.count - 1].isReasoningSummary = summary
            }
            if let durationMs { items[items.count - 1].duration = Double(durationMs) / 1000 }
        case .toolCall(let id, let title, let kind, let status, let input, let engineDetail, let engineSummary):
            let rendered = input.map { $0.prettyPrinted() }
            let detail = ToolPresentation.detail(input: input, output: nil) ?? engineDetail
            let summary = engineSummary.flatMap(ToolPresentation.summary) ?? ToolPresentation.summary(input: input)
            if let index = toolIndex(id), case .tool(var tool) = items[index].content {
                tool.title = title
                tool.kind = kind ?? tool.kind
                tool.status = status ?? tool.status
                tool.input = rendered ?? tool.input
                tool.inputDetail = detail ?? tool.inputDetail
                tool.summary = summary ?? tool.summary
                items[index].content = .tool(tool)
            } else {
                append(.tool(ToolItem(callId: id, title: title, kind: kind, status: status, input: rendered,
                                      inputDetail: detail, summary: summary)), at: now)
            }
        case .toolCallUpdate(let id, let title, let status, let output, let diffs, let outputDelta, let exitCode):
            let changes = FileChange.group(diffs)
            if let index = toolIndex(id), case .tool(var tool) = items[index].content {
                tool.title = title ?? tool.title
                tool.status = status ?? tool.status
                if let outputDelta { tool.output = Self.liveTail((tool.output ?? "") + outputDelta) }
                tool.output = output ?? tool.output
                if !changes.isEmpty { tool.changes = changes }
                tool.exitCode = exitCode ?? tool.exitCode
                items[index].content = .tool(tool)
            } else {
                append(.tool(ToolItem(callId: id, title: title ?? "Tool", status: status, output: output, changes: changes)), at: now)
            }
        case .contextReport(let text, _):
            apply(.agentText(text), at: now)
        case .compaction(let card):
            // A `/compact` answers with the card: it takes the place of the
            // command's empty answer rather than following it.
            if case .command(_, "") = items.last?.content {
                items.removeLast()
            }
            append(.compaction(card), at: now)
        case .usage(let usage):
            self.usage = usage
        case .notice, .stopped, .title, .configOptions, .availableCommands, .unknown:
            break
        }
    }

    /// Live output a running call shows, at most its last 64 KB; the finished
    /// result replaces it.
    static func liveTail(_ text: String) -> String {
        let limit = 64 * 1024
        guard text.utf8.count > limit else { return text }
        return String(text.utf8.suffix(limit)) ?? String(text.suffix(limit / 4))
    }

    private func toolIndex(_ callId: String) -> Int? {
        items.lastIndex { item in
            if case .tool(let tool) = item.content { return tool.callId == callId }
            return false
        }
    }

    /// The turn ended: a thought still open at the end took until `now`.
    public mutating func finishThinking(at now: Date) {
        guard let last = items.indices.last, case .thinking = items[last].content,
              let started = items[last].startedAt, items[last].duration == nil else { return }
        items[last].duration = max(now.timeIntervalSince(started), 0)
    }

    /// The turn ended: its open thought stops its clock, and no card it ran
    /// may keep spinning, even when its finishing update never arrived. A card
    /// with output completed; an empty one failed with `failure` when the turn
    /// was cancelled or failed, or completed when the turn did.
    ///
    /// A turn that completed, or that the user stopped (`cancelled`), puts
    /// its timing on its last reply; one that failed never finished, so it
    /// shows none.
    public mutating func finishTurn(at now: Date, failure: String? = nil, cancelled: Bool = false) {
        finishThinking(at: now)
        finishReply(at: now)
        // A turn stopped before any reply gets an empty one, which shows
        // that it was stopped and after how long, as its replay does.
        if cancelled, let first = turnFirstId, turnStartedAt != nil,
           !items.contains(where: { item in
               if case .assistant = item.content { item.id >= first } else { false }
           }) {
            append(.assistant(""))
        }
        if let started = turnStartedAt, let first = turnFirstId, failure == nil || cancelled,
           let last = items.lastIndex(where: { item in
               if case .assistant = item.content { item.id >= first } else { false }
           }) {
            stamp(TurnTiming(finished: now, elapsed: max(now.timeIntervalSince(started), 0),
                             outcome: cancelled ? .cancelled : .completed), on: last)
        }
        turnStartedAt = nil
        turnFirstId = nil
        for index in items.indices {
            guard case .tool(var tool) = items[index].content, tool.status == .pending || tool.status == .inProgress else { continue }
            let hasOutput = !(tool.output ?? "").isEmpty || !tool.changes.isEmpty
            if let failure, !hasOutput {
                tool.status = .failed
                tool.output = failure
            } else {
                tool.status = .completed
            }
            tool.settledByTurnEnd = true
            items[index].content = .tool(tool)
        }
    }

    /// Only a turn's last reply shows its timing: an earlier reply of the
    /// same replayed turn gives it up.
    private mutating func stamp(_ turn: TurnTiming, on index: Int) {
        if let id = turn.id {
            for earlier in items.indices where earlier < index && items[earlier].turn?.id == id {
                items[earlier].turn = nil
            }
        }
        items[index].turn = turn
    }

    /// The live reply still being written was done by `now`, when whatever
    /// follows it arrived.
    private mutating func finishReply(at now: Date) {
        guard let id = liveReplyId else { return }
        liveReplyId = nil
        guard let last = items.indices.last, items[last].id == id, items[last].at == nil else { return }
        items[last].at = now
    }

    private mutating func append(_ content: TranscriptItem.Content, at now: Date? = nil) {
        // Whatever follows a thought or a reply ends it.
        if let now {
            finishThinking(at: now)
            finishReply(at: now)
        }
        var item = TranscriptItem(id: nextId, content: content)
        if case .thinking = content { item.startedAt = now }
        if case .assistant = content, now != nil { liveReplyId = nextId }
        items.append(item)
        nextId += 1
    }
}
