import AppKit
import RustCodeKit
import SwiftUI

struct TranscriptRow: View, Equatable {
    let item: TranscriptItem
    var isCurrentMatch = false
    /// This is the reply the agent is still writing.
    var isStreaming = false
    /// The folded run of tool calls this row starts, if any.
    var fold: ToolRunFold? = nil
    /// The finished turn whose work this row starts, if any.
    var turnFold: TurnFold? = nil
    /// Drawn inside an opened turn, under its row.
    var isNested = false
    /// A long message the reader opened, drawn whole (#2303).
    var isExpanded = false
    /// The agents this row's spawn and the ones right after it started.
    var agents: [AgentSpawn]? = nil
    /// On a finished turn's last reply: what the turn changed and checked.
    var completion: TurnCompletion? = nil

    var body: some View {
        Group {
            if let turnFold {
                VStack(alignment: .leading, spacing: 12) {
                    TurnSummary(fold: turnFold)
                    if turnFold.isOpen { content.pipelineStep(node) }
                }
            } else if isNested {
                content.pipelineStep(node)
            } else {
                content
            }
        }
            .padding(isCurrentMatch ? 6 : 0)
            .background {
                if isCurrentMatch {
                    RoundedRectangle(cornerRadius: 16).fill(Color.brand.opacity(0.1))
                    RoundedRectangle(cornerRadius: 16).strokeBorder(Color.brand.opacity(0.6), lineWidth: 1.5)
                }
            }
            .padding(isCurrentMatch ? -6 : 0)
            .animation(.snappy(duration: 0.2), value: isCurrentMatch)
    }

    /// This row's mark on an opened turn's rail.
    private var node: PipelineNode {
        switch item.content {
        case .assistant: return .said
        case .thinking: return .thought
        case .tool(let tool):
            return tool.status == .failed || (fold?.failed ?? 0) > 0 ? .failed : .call
        default: return .call
        }
    }

    /// Inside an opened turn a thought is one quiet line, not a box.
    private var isStep: Bool { isNested || turnFold?.isOpen == true }

    @ViewBuilder
    private var content: some View {
        switch item.content {
        case .user(let text) where ToolAttachment.isAttachment(text):
            // The screenshots a tool took, shown with the work, not as a message.
            HStack(spacing: 6) {
                ForEach(Array(ImageMarkers.split(text).images.enumerated()), id: \.offset) { _, url in
                    InlineImage(url: url, maxSize: 160)
                }
            }
        case .user(let text):
            // Room between the question and the work that answers it.
            UserMessage(id: item.id, text: text, isExpanded: isExpanded || isCurrentMatch, sentAt: item.at)
                .padding(.bottom, 10)
        case .assistant(let text):
            VStack(alignment: .leading, spacing: 10) {
                PacedReply(text: text, isStreaming: isStreaming) { shown, isWriting in
                    AssistantMessage(id: item.id, text: shown, isStreaming: isWriting, turn: item.turn, writtenAt: item.at,
                                     isExpanded: isExpanded || isCurrentMatch, isStep: isStep)
                }
                if let completion, !isStreaming { TurnCompletionCard(completion: completion) }
            }
        case .thinking(let text) where isStep:
            ThoughtLine(text: text)
        case .thinking(let text):
            ThinkingBlock(text: text, isLive: isStreaming, duration: item.duration, isSummary: item.isReasoningSummary)
        case .tool(let tool):
            if let agents {
                AgentSpawnGroup(spawns: agents)
            } else if let fold {
                VStack(alignment: .leading, spacing: 12) {
                    ToolRunSummary(fold: fold)
                    if fold.isOpen, !tool.isBlank { ToolRow(tool: tool) }
                }
            } else if !tool.isBlank {
                // Calls in a row sit closer than the chat's other rows, as one list.
                ToolRow(tool: tool).padding(.vertical, -6)
            }
        case .command(let input, let output): CommandCard(input: input, output: output)
        case .compaction(let card): CompactionCardView(card: card)
        }
    }
}

/// Where the history was compacted: the numbers, and the summary the model
/// works from now behind a disclosure. The messages above stay in the chat;
/// only the model's context was folded.
private struct CompactionCardView: View {
    let card: CompactionCard
    @State private var showsSummary = false

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                Image(systemName: "arrow.down.right.and.arrow.up.left").foregroundStyle(Color.brand)
                Text(card.headline).font(.callout.weight(.semibold))
                if let detail = card.detail {
                    Text(detail).font(.caption).foregroundStyle(.secondary)
                }
                Spacer(minLength: 8)
                if card.summary != nil {
                    Button(showsSummary ? "Hide summary" : "Show summary") {
                        withAnimation(.snappy(duration: 0.2)) { showsSummary.toggle() }
                    }
                    .buttonStyle(.borderless)
                    .font(.caption)
                }
            }
            Text("The messages above are folded into this summary for the model.")
                .font(.caption)
                .foregroundStyle(.secondary)
            if showsSummary, let summary = card.summary {
                Divider().opacity(0.5)
                MarkdownText(text: summary)
            }
        }
        .card()
        .contextMenu {
            if let summary = card.summary {
                Button("Copy Summary", systemImage: "doc.on.doc") { Clipboard.copy(summary) }
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel(card.headline)
    }
}

/// `/command args` with the command in the brand colour, as the iOS app shows it.
func slashCommandText(_ text: String) -> Text {
    let parts = text.split(separator: " ", maxSplits: 1, omittingEmptySubsequences: false)
    let command = Text(String(parts[0])).foregroundStyle(Color.brand).fontWeight(.semibold)
    guard parts.count > 1 else { return command }
    return command + Text(" " + parts[1])
}

/// A host command and its answer: the engine replies `**Title**` then the output,
/// shown like the iOS app's command result.
private struct CommandCard: View {
    let input: String
    let output: String
    @Environment(\.messageActions) private var messageActions

    private var title: String? {
        guard output.hasPrefix("**"), let end = output.dropFirst(2).range(of: "**") else { return nil }
        return String(output[output.index(output.startIndex, offsetBy: 2)..<end.lowerBound])
    }

    private var content: String {
        guard let title else { return output }
        return String(output.dropFirst(title.count + 4)).trimmingCharacters(in: .whitespacesAndNewlines)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 8) {
                Image(systemName: "terminal").foregroundStyle(Color.brand)
                slashCommandText(input).font(.callout).textSelection(.enabled)
                Spacer(minLength: 8)
                if output.isEmpty { ProgressView().controlSize(.small) }
            }
            if let title {
                Text(title).font(.headline)
            }
            if !content.isEmpty {
                MarkdownText(text: content)
            }
        }
        .card()
        .contextMenu {
            Button("Copy Output", systemImage: "doc.on.doc") { Clipboard.copy(content) }
            Button("Copy Command", systemImage: "terminal") { Clipboard.copy(input) }
            if let actions = messageActions {
                Divider()
                Button("Run Again", systemImage: "arrow.clockwise") { actions.retry(input) }
            }
        }
    }
}

/// The engine's closing summary, as the iOS app shows it.
private struct SummaryCard: View {
    let summary: AssistantSummary

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Label("Summary", systemImage: "text.quote")
                .font(.caption.weight(.semibold))
                .foregroundStyle(.secondary)
            Text(summary.summary)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
            if let next = summary.nextAction {
                Divider().opacity(0.5)
                VStack(alignment: .leading, spacing: 3) {
                    Text("Next action").font(.caption.weight(.medium)).foregroundStyle(.secondary)
                    Text(next).font(.callout).textSelection(.enabled)
                }
            }
        }
        .card()
    }
}

private struct UserMessage: View {
    let id: Int
    let text: String
    var isExpanded = false
    var sentAt: Date?
    @Environment(\.messageActions) private var messageActions

    var body: some View {
        let message = ImageMarkers.split(text)
        VStack(alignment: .trailing, spacing: 6) {
            if !message.images.isEmpty {
                HStack(spacing: 6) {
                    ForEach(Array(message.images.enumerated()), id: \.offset) { _, url in
                        InlineImage(url: url, maxSize: 220)
                    }
                }
                .contextMenu {
                    if let actions = messageActions {
                        Button("Edit and Resend", systemImage: "pencil") { actions.edit(text) }
                        Button("Retry", systemImage: "arrow.clockwise") { actions.retryMessage(id) }
                    }
                }
                .frame(maxWidth: .infinity, alignment: .trailing)
                .padding(.leading, 80)
            }
            if !message.text.isEmpty || message.images.isEmpty {
                UserBubble(id: id, text: message.text, original: text, isExpanded: isExpanded, sentAt: sentAt)
            }
        }
    }
}

private struct UserBubble: View {
    let id: Int
    let text: String
    /// The message as sent, image markers included, for edit and resend.
    let original: String
    var isExpanded = false
    var sentAt: Date?
    @Environment(\.messageActions) private var messageActions
    @Environment(\.findQuery) private var findQuery

    var body: some View {
        // When it was sent and Copy sit below the bubble, clear of its text,
        // and always show, so nothing appears or moves under the pointer
        // (#2444, #2461).
        VStack(alignment: .trailing, spacing: MessageFooter.gap) {
            FoldingMessage(id: id, text: text, folds: true, isExpanded: isExpanded,
                           chrome: BubbleChrome(), chromeHeight: BubbleChrome.height) {
                Group {
                    if text.hasPrefix("/"), findQuery.isEmpty {
                        slashCommandText(text)
                    } else {
                        let shown = FindHighlight.apply(ChatLinks.linked(text), query: findQuery)
                        Text(shown).linkCursor(shown)
                    }
                }
                .textSelection(.enabled)
                .lineSpacing(2)
            }
            MessageFooter(edge: .trailing, label: sentAt.map { TurnTiming.time($0) }, date: sentAt,
                          help: "Copy message") { Clipboard.copy(text) }
        }
            .messageMenu { [text, original, id, messageActions] in
                var items = [MessageMenuItem(title: "Copy", systemImage: "doc.on.doc") { Clipboard.copy(text) }]
                if let actions = messageActions {
                    items.append(MessageMenuItem(title: "Edit and Resend", systemImage: "pencil", action: { actions.edit(original) },
                                                 startsGroup: true))
                    items.append(MessageMenuItem(title: "Retry", systemImage: "arrow.clockwise") { actions.retryMessage(id) })
                }
                return items
            }
            .frame(maxWidth: .infinity, alignment: .trailing)
            .padding(.leading, 80)
    }
}

/// Under a message: Copy at its outer edge, then when it was sent or written
/// in quiet type, the full date on hover.
private struct MessageFooter: View {
    /// The side the message hugs: Copy sits on that edge.
    let edge: HorizontalEdge
    var label: String?
    var date: Date?
    let help: String
    let action: () -> Void
    /// The room between a message and its footer.
    static let gap: CGFloat = 4

    var body: some View {
        HStack(spacing: 2) {
            if edge == .leading { copy }
            if let label {
                Text(label)
                    .font(.caption)
                    .monospacedDigit()
                    .foregroundStyle(.tertiary)
                    .lineLimit(1)
                    .padding(.horizontal, 4)
                    .help(date?.formatted(date: .complete, time: .standard) ?? "")
            }
            if edge == .trailing { copy }
        }
    }

    private var copy: some View {
        CopyButton(help: help, height: 22, iconOnly: true, action: action)
    }
}

/// A user message's bubble around its text.
private struct BubbleChrome: ViewModifier {
    /// How much taller the bubble is than its text.
    static let height: Double = 18

    func body(content: Content) -> some View {
        content
            .padding(.horizontal, 14)
            .padding(.vertical, Self.height / 2)
            .background(.fill.tertiary, in: .rect(cornerRadius: 18))
    }
}

/// A reply being written, played out at an even pace (`StreamPacer`): the
/// stream comes in bursts, and the text is written steadily a moment behind
/// it, then the rest plays out quickly once the stream ends. The reply counts
/// as still being written until it has all been shown. A reply that was
/// already there when it came into view shows whole. With Reduce Motion, or
/// the fade turned off in Settings, text shows as it arrives.
private struct PacedReply<Content: View>: View {
    let text: String
    let isStreaming: Bool
    @ViewBuilder let content: (_ shown: String, _ isWriting: Bool) -> Content
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @AppStorage(StreamingProse.revealKey) private var reveals = true
    @State private var pacer: StreamPacer?
    /// Characters shown, while the pacer is behind the stream.
    @State private var shown: Int?

    private var paces: Bool { reveals && !reduceMotion }

    var body: some View {
        let behind = shown.map { $0 < text.count } ?? false
        content(behind ? String(text.pacedPrefix(shown ?? 0)) : text, isStreaming || behind)
            .onChange(of: text.count) { _, count in pacer?.received(count, at: Date()) }
            .task(id: isStreaming) { await play() }
    }

    /// Writes the reply out a frame at a time until it is all shown and the
    /// stream is over.
    private func play() async {
        guard paces else { return pacer = nil }
        if pacer == nil {
            guard isStreaming else { return }
            pacer = StreamPacer(showing: text.count)
        }
        let clock = ContinuousClock()
        var last = clock.now
        while !Task.isCancelled {
            try? await Task.sleep(for: .milliseconds(16))
            // Read after the sleep: text that arrived meanwhile is in it.
            guard var next = pacer else { return }
            let now = clock.now
            let count = next.advance(by: (now - last) / .seconds(1), streaming: isStreaming)
            last = now
            pacer = next
            if shown != count { shown = count }
            if !isStreaming, next.isCaughtUp {
                pacer = nil
                shown = nil
                return
            }
        }
    }
}

/// Agent text, with `<think>` sections the engine leaves inline folded away.
private struct AssistantMessage: View {
    let id: Int
    let text: String
    var isStreaming = false
    /// On a turn's last reply: how long the turn took and when it ended.
    var turn: TurnTiming?
    /// When this reply was done being written, if known.
    var writtenAt: Date?
    var isExpanded = false
    /// A step of an opened turn: no footer, so the rail reads as one line
    /// of steps. Copy is in its menu.
    var isStep = false
    @Environment(\.messageActions) private var messageActions

    private struct Parsed {
        var blocks: [AssistantBlock]
        var summary: AssistantSummary?
    }

    private static let cache = RenderCache<Parsed>(countLimit: 1_000)

    /// Split once per text: finding the summary tries to read JSON at every
    /// `{` from the end, too slow to repeat for every message on every redraw.
    private var parsed: Parsed {
        Self.cache.value(for: "\u{0}" + text, cached: !isStreaming) {
            let (prose, summary) = AssistantSummary.split(text)
            return Parsed(blocks: AssistantBlock.parse(prose, isStreaming: isStreaming), summary: summary)
        }
    }

    var body: some View {
        let parsed = parsed
        VStack(alignment: .leading, spacing: 10) {
            FoldingMessage(id: id, text: text, folds: false,
                           isExpanded: isExpanded) {
                VStack(alignment: .leading, spacing: 10) {
                    ForEach(Array(parsed.blocks.enumerated()), id: \.offset) { index, block in
                        let isLast = index == parsed.blocks.count - 1
                        switch block {
                        case .markdown(let text):
                            MarkdownText(text: text, visualizations: true, smartUI: true, isStreaming: isStreaming && isLast)
                                .environment(\.prosePlace, ProsePlace(item: id, block: index))
                        case .thinking(let text, let isComplete):
                            ThinkingBlock(text: text, isLive: isStreaming && isLast && !isComplete)
                        }
                    }
                    if let summary = parsed.summary { SummaryCard(summary: summary) }
                }
            }
            // Never while the reply is written, so the footer cannot claim
            // it is done. Copy sits below the reply, out of its text, and
            // always shows (#2444).
            if !isStreaming, !isStep { footer }
        }
        .messageMenu { [text, messageActions] in
            var items = [
                MessageMenuItem(title: "Copy Reply", systemImage: "doc.on.doc") { Clipboard.copy(AssistantSummary.split(text).prose) },
                MessageMenuItem(title: "Copy as Markdown", systemImage: "text.document") { Clipboard.copy(text) },
            ]
            if let actions = messageActions {
                items.append(MessageMenuItem(title: "Quote in Reply", systemImage: "text.quote", action: {
                    actions.edit(AssistantSummary.split(text).prose.split(separator: "\n").map { "> " + $0 }.joined(separator: "\n") + "\n\n")
                }, startsGroup: true))
            }
            return items
        }
    }

    /// Under a reply: Copy, then how long the turn took and when it ended on
    /// a turn's last reply, or when this reply was written on any other
    /// (#2463). A reply with no known time shows Copy alone.
    private var footer: some View {
        MessageFooter(edge: .leading, label: turn.map { $0.label() } ?? writtenAt.map { TurnTiming.time($0) },
                      date: turn?.finished ?? writtenAt, help: "Copy reply") {
            Clipboard.copy(AssistantSummary.split(text).prose)
        }
        .padding(.leading, -5)
        .padding(.top, MessageFooter.gap - 10)
    }
}

/// The model's reasoning, muted and folded away, as the iOS app shows it.
/// While it is being written it stays open on its last lines; once done it
/// folds to a "Thought for 12s" header, or "Reasoning summary" for a
/// provider's summary, unless the reader opened it.
private struct ThinkingBlock: View {
    let text: String
    /// The thought is still being written.
    var isLive = false
    /// How long it took, when it was watched live or the engine recorded it.
    var duration: TimeInterval?
    /// The provider's summary of its hidden reasoning, not reasoning text.
    var isSummary = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    /// Nil until the reader toggles it: open while live, closed after.
    @State private var expanded: Bool?

    private static let liveLines = 5

    private var trimmed: String { text.trimmingCharacters(in: .whitespacesAndNewlines) }

    private var isExpanded: Binding<Bool> {
        Binding(get: { expanded ?? isLive }, set: { expanded = $0 })
    }

    private var title: String {
        TranscriptItem.thoughtTitle(isLive: isLive, isSummary: isSummary, duration: duration)
    }

    var body: some View {
        // A thought with no text has nothing to show, live or over.
        if !trimmed.isEmpty { card }
    }

    private var card: some View {
        DisclosureGroup(isExpanded: isExpanded) {
            Group {
                if trimmed.isEmpty {
                    EmptyView()
                } else if isLive, expanded == nil {
                    // The newest lines only, so a long thought does not push
                    // the chat around while it streams.
                    Text(MarkdownText.inline(liveTail))
                        .lineLimit(Self.liveLines)
                        .truncationMode(.head)
                } else {
                    MarkdownText(text: trimmed)
                }
            }
            .font(.callout)
            .foregroundStyle(.secondary)
            .textSelection(.enabled)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.top, 8)
            .contextMenu {
                Button(isSummary ? "Copy Reasoning Summary" : "Copy Thinking", systemImage: "doc.on.doc") { Clipboard.copy(trimmed) }
            }
        } label: {
            HStack(spacing: 6) {
                Text(title)
                    .contentTransition(.opacity)
                if isLive { ProgressView().controlSize(.mini) }
            }
            .font(.callout)
            .foregroundStyle(.tertiary)
        }
        .disclosureGroupStyle(QuietDisclosureStyle())
        .frame(maxWidth: .infinity, alignment: .leading)
        .animation(reduceMotion ? nil : .snappy(duration: 0.2), value: isLive)
        .accessibilityElement(children: .contain)
        .accessibilityLabel(title)
    }

    private var liveTail: String {
        trimmed.split(separator: "\n", omittingEmptySubsequences: true)
            .suffix(Self.liveLines).joined(separator: "\n")
    }
}

private struct ToolRow: View {
    let tool: ToolItem
    /// Nil until the user toggles it: at edits verbosity an edit opens on its
    /// diff; anything else starts closed.
    @State private var expanded: Bool?
    @Environment(\.projectRoot) private var projectRoot
    @Environment(\.editsOpenByDefault) private var editsOpen

    private var hasDetail: Bool { tool.input != nil || tool.output != nil || !tool.changes.isEmpty }
    private var isExpanded: Binding<Bool> {
        Binding(get: { expanded ?? (editsOpen && !tool.changes.isEmpty) }, set: { expanded = $0 })
    }

    private var isRunning: Bool { tool.status == .pending || tool.status == .inProgress }

    /// The last lines a running command printed, so it can be watched without
    /// opening the card.
    private var liveTail: String? {
        guard isRunning, !isExpanded.wrappedValue, let output = tool.output else { return nil }
        let lines = output.split(separator: "\n", omittingEmptySubsequences: false)
            .suffix(Self.liveTailLines + 1)
            .drop { $0.trimmingCharacters(in: .whitespaces).isEmpty }
        let tail = lines.suffix(Self.liveTailLines).joined(separator: "\n")
            .trimmingCharacters(in: .newlines)
        return tail.isEmpty ? nil : tail
    }

    private static let liveTailLines = 6
    /// Detail, output and diffs line up under the title, past the status.
    static let indent: CGFloat = 26

    var body: some View {
        // One quiet line, no card, so a round of calls reads as a list
        // rather than a stack of boxes (#2434); a click opens its detail.
        VStack(alignment: .leading, spacing: 4) {
            card
            if let liveTail {
                Text(liveTail)
                    .font(.system(.caption, design: .monospaced))
                    .foregroundStyle(.secondary)
                    .lineLimit(Self.liveTailLines)
                    .truncationMode(.head)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.leading, Self.indent)
                    .transition(.opacity)
            }
        }
        .contextMenu {
            if let input = tool.input { Button("Copy Input", systemImage: "arrow.down.doc") { Clipboard.copy(input) } }
            if let output = tool.output { Button("Copy Output", systemImage: "arrow.up.doc") { Clipboard.copy(output) } }
            if tool.changes.count == 1, let change = tool.changes.first {
                Divider()
                FileActions(path: change.path).menuItems
            }
        }
    }

    @ViewBuilder
    private var card: some View {
        Group {
            if hasDetail {
                DisclosureGroup(isExpanded: isExpanded) {
                    VStack(alignment: .leading, spacing: 10) {
                        if tool.changes.isEmpty {
                            if let input = tool.input { detail("Input", input) }
                            if let output = tool.output { detail("Output", output) }
                        } else {
                            ForEach(tool.changes) { change in
                                FileChangeView(change: change, showsHeader: tool.changes.count > 1)
                            }
                        }
                    }
                    .padding(.leading, Self.indent)
                    .padding(.top, 6)
                    .padding(.bottom, 4)
                } label: {
                    label
                }
                .disclosureGroupStyle(RowDisclosureStyle())
            } else {
                label
            }
        }
    }

    /// An edit of one file names it relative to the project.
    private var detailLine: String? {
        if tool.changes.count == 1, let change = tool.changes.first {
            return change.displayPath(relativeTo: projectRoot)
        }
        // A page reads as `github.com/…/rest`, as links in replies do.
        if tool.displayTitle == "Read URL", ToolIcon.siteIcon(for: tool) != nil, let detail = tool.detail, let url = URL(string: detail) {
            return Linkify.label(for: url)
        }
        return tool.detail.map { ProjectPath.relative($0, to: projectRoot) }
    }

    private var label: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            statusIcon
                .font(.system(size: 12))
                .frame(width: 18)
                .alignmentGuide(.firstTextBaseline) { $0[VerticalAlignment.center] + 4 }
            Text(tool.displayTitle)
                .font(.callout.weight(.medium))
                .lineLimit(1)
                .layoutPriority(2)
            // What the model said the call does, then what it acted on.
            if let summary = tool.summary {
                Text(summary)
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.tail)
                    .layoutPriority(1)
            }
            if let detail = detailLine {
                Text(detail)
                    .font(.caption.monospaced())
                    .foregroundStyle(.tertiary)
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .help(tool.changes.isEmpty ? tool.detail ?? detail : detail)
            }
            if !tool.changes.isEmpty {
                DiffCount(
                    added: tool.changes.reduce(0) { $0 + $1.added },
                    removed: tool.changes.reduce(0) { $0 + $1.removed })
                    .fixedSize()
            }
        }
        .frame(minHeight: 24)
    }

    /// What the call did, as an icon: the site's own icon for a page it
    /// read, else a symbol for its kind. A spinner while it runs, and red
    /// when it failed.
    @ViewBuilder
    private var statusIcon: some View {
        Group {
            switch tool.status {
            case .failed:
                Image(systemName: "xmark.circle.fill").foregroundStyle(.red)
            case .pending, .inProgress:
                ProgressView().controlSize(.mini)
            case .completed, nil:
                if let site = ToolIcon.siteIcon(for: tool) {
                    SiteIcon(url: site, fallback: ToolIcon.symbol(for: tool))
                } else {
                    Image(systemName: ToolIcon.symbol(for: tool)).foregroundStyle(.secondary)
                }
            }
        }
        .transition(.scale(scale: 0.6).combined(with: .opacity))
        .animation(.spring(duration: 0.3, bounce: 0.3), value: tool.status)
    }

    private func detail(_ title: String, _ text: String) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(title).font(.caption.weight(.semibold)).foregroundStyle(.secondary)
            ToolTextBox(text: text)
        }
    }
}

/// A website's icon, with a symbol until it loads or when it has none.
private struct SiteIcon: View {
    let url: URL
    let fallback: String

    var body: some View {
        AsyncImage(url: url, transaction: Transaction(animation: .easeOut(duration: 0.2))) { phase in
            if let image = phase.image {
                // On a light tile, as browsers show them: many site icons
                // are dark and would vanish on the dark chat.
                image.resizable().interpolation(.high).scaledToFit()
                    .frame(width: 12, height: 12)
                    .padding(2)
                    .background(.white, in: .rect(cornerRadius: 4))
            } else {
                Image(systemName: fallback).foregroundStyle(.secondary)
            }
        }
    }
}

/// A tool's input or output. Long text shows its first lines and grows in
/// place on request: a box that scrolls on its own would put a second
/// scroller inside the chat and take the wheel from it.
private struct ToolTextBox: View {
    let text: String
    @State private var showsAll = false

    private static let previewLines = 14
    /// Beyond this the box stops growing; Copy Output has the rest.
    private static let maxLines = 2_000

    var body: some View {
        let lines = text.split(separator: "\n", omittingEmptySubsequences: false)
        let limit = showsAll ? Self.maxLines : Self.previewLines
        let hidden = lines.count - min(lines.count, limit)
        VStack(alignment: .leading, spacing: 0) {
            Text(hidden > 0 ? lines.prefix(limit).joined(separator: "\n") : text)
                .font(.system(.caption, design: .monospaced))
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(10)
            if hidden > 0 || showsAll && lines.count > Self.previewLines {
                Button(showsAll && hidden == 0 ? "Show less" : showsAll ? "\(hidden) more lines — use Copy Output"
                       : "Show all \(lines.count) lines") {
                    showsAll.toggle()
                }
                .buttonStyle(.plain)
                .font(.caption.weight(.medium))
                .foregroundStyle(.secondary)
                .pointingHandCursor()
                .padding(.horizontal, 10)
                .padding(.bottom, 8)
            }
        }
        .background(.background.opacity(0.5), in: .rect(cornerRadius: 8))
    }
}

/// `+N −M` for the lines an edit added and removed.
private struct DiffCount: View {
    let added: Int
    let removed: Int

    var body: some View {
        HStack(spacing: 6) {
            if added > 0 || removed == 0 { Text("+\(added)").foregroundStyle(.green) }
            if removed > 0 { Text("\u{2212}\(removed)").foregroundStyle(.red) }
        }
        .font(.caption.monospacedDigit().weight(.medium))
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(added) lines added, \(removed) removed")
    }
}

/// Reveal or open the file an edit changed.
private struct FileActions {
    let path: String

    private var url: URL { URL(fileURLWithPath: path) }
    private var exists: Bool { FileManager.default.fileExists(atPath: path) }

    /// A deleted file reveals the folder it was in.
    func reveal() {
        if exists {
            NSWorkspace.shared.activateFileViewerSelecting([url])
        } else {
            NSWorkspace.shared.open(url.deletingLastPathComponent())
        }
    }

    func open() { NSWorkspace.shared.open(url) }

    @ViewBuilder
    var menuItems: some View {
        Button("Reveal in Finder", systemImage: "folder") { reveal() }
        Button("Open", systemImage: "arrow.up.forward.app") { open() }.disabled(!exists)
        Button("Copy Path", systemImage: "doc.on.doc") { Clipboard.copy(path) }
    }

    var buttons: some View {
        HStack(spacing: 2) {
            Button("Reveal in Finder", systemImage: "folder") { reveal() }
                .help("Reveal in Finder")
            Button("Open", systemImage: "arrow.up.forward.app") { open() }
                .help("Open")
                .disabled(!exists)
        }
        .labelStyle(.iconOnly)
        .buttonStyle(.borderless)
        .controlSize(.small)
    }
}

/// One file's changes as a unified diff: removed lines red, added lines
/// green, numbered in the old and new file. A long diff shows its head until
/// "Show all".
struct FileChangeView: View {
    let change: FileChange
    /// Whether to name the file above its diff (the card names a lone file).
    var showsHeader: Bool
    @State private var showsAll = false
    @Environment(\.projectRoot) private var projectRoot

    /// Rows shown before "Show all", and the most ever drawn.
    private static let previewRows = 12
    private static let maxRows = 2_000

    private var isLong: Bool { change.rows.count > Self.previewRows + 3 }

    private var visibleRows: ArraySlice<DiffRow> {
        change.rows.prefix(isLong && !showsAll ? Self.previewRows : Self.maxRows)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            if showsHeader {
                HStack(spacing: 8) {
                    Image(systemName: change.isNewFile ? "doc.badge.plus" : "doc.text")
                        .foregroundStyle(.secondary)
                    Text(change.displayPath(relativeTo: projectRoot))
                        .font(.caption.monospaced())
                        .lineLimit(1)
                        .truncationMode(.middle)
                    DiffCount(added: change.added, removed: change.removed)
                    Spacer(minLength: 8)
                    FileActions(path: change.path).buttons
                }
            }
            diff(Array(visibleRows))
            .background(.background.opacity(0.5), in: .rect(cornerRadius: 8))
            .clipShape(.rect(cornerRadius: 8))
            .overlay(alignment: .topTrailing) {
                if !showsHeader {
                    FileActions(path: change.path).buttons
                        .padding(2)
                        .background(.regularMaterial, in: .rect(cornerRadius: 6))
                        .padding(4)
                }
            }
            footer
        }
    }

    @ViewBuilder
    private var footer: some View {
        let hidden = change.rows.count - visibleRows.count
        if isLong, !showsAll {
            Button("Show all \(change.rows.count) lines") {
                withAnimation(.snappy(duration: 0.2)) { showsAll = true }
            }
            .buttonStyle(.link)
            .pointingHandCursor()
            .font(.caption)
        } else if showsAll {
            HStack(spacing: 10) {
                if hidden > 0 {
                    Text("\(hidden) more lines not shown").foregroundStyle(.secondary)
                }
                Button("Show less") {
                    withAnimation(.snappy(duration: 0.2)) { showsAll = false }
                }
                .buttonStyle(.link)
                .pointingHandCursor()
            }
            .font(.caption)
        }
    }

    /// The code is one text, so a selection runs across lines like in an
    /// editor; line numbers and signs sit in their own column, and each row's
    /// colour is a band behind both.
    private func diff(_ rows: [DiffRow]) -> some View {
        ZStack(alignment: .topLeading) {
            VStack(spacing: 0) {
                ForEach(Array(rows.enumerated()), id: \.offset) { _, row in
                    // A plain rectangle: a colour set as the background of
                    // the hidden line is drawn rounded on current macOS,
                    // which made each changed line a pill of its own.
                    Text(" ").hidden()
                        .frame(maxWidth: .infinity)
                        .background { Rectangle().fill(band(row)) }
                }
            }
            HStack(alignment: .top, spacing: 6) {
                Text(gutter(rows)).fixedSize()
                // A diff line can be longer than the chat. A plain ScrollView
                // keeps the card at the chat's width and lets the line scroll
                // under the pointer; ViewThatFits could pick the fixed-size
                // text here and let the card clip it instead.
                ScrollView(.horizontal) {
                    Text(code(rows))
                        .textSelection(.enabled)
                        .fixedSize(horizontal: true, vertical: false)
                        .padding(.trailing, 10)
                }
                .scrollIndicators(.visible)
                .scrollBounceBehavior(.basedOnSize, axes: .horizontal)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .padding(.leading, 6)
        }
        .font(.system(.caption, design: .monospaced))
        .padding(.vertical, 4)
    }

    private func band(_ row: DiffRow) -> Color {
        guard case .line(let line) = row, let tint = tint(line.kind) else { return .clear }
        return tint.opacity(0.13)
    }

    private func gutter(_ rows: [DiffRow]) -> AttributedString {
        let digits = change.gutterDigits
        var result = AttributedString()
        for (index, row) in rows.enumerated() {
            if index > 0 { result += AttributedString("\n") }
            guard case .line(let line) = row else {
                result += AttributedString(String(repeating: " ", count: digits + 2))
                continue
            }
            var number = AttributedString(line.gutter(digits: digits) + " ")
            number.foregroundColor = .secondary.opacity(0.6)
            var mark = AttributedString(sign(line.kind))
            mark.foregroundColor = tint(line.kind) ?? .secondary
            result += number + mark
        }
        return result
    }

    private func code(_ rows: [DiffRow]) -> AttributedString {
        let highlighter = SyntaxHighlighter(path: change.path)
        var result = AttributedString()
        var inBlockComment = false
        for (index, row) in rows.enumerated() {
            if index > 0 { result += AttributedString("\n") }
            switch row {
            case .gap(let count):
                var gap = AttributedString(count.map { "\u{22EF} \($0) unchanged line\($0 == 1 ? "" : "s")" } ?? "\u{22EF}")
                gap.foregroundColor = .secondary.opacity(0.6)
                result += gap
            case .line(let line):
                result += CodeStyle.attributed(line.text, highlighter: highlighter, inBlockComment: &inBlockComment)
            }
        }
        return result
    }

    private func sign(_ kind: DiffLine.Kind) -> String {
        switch kind {
        case .added: "+"
        case .removed: "\u{2212}"
        case .context: " "
        }
    }

    private func tint(_ kind: DiffLine.Kind) -> Color? {
        switch kind {
        case .added: .green
        case .removed: .red
        case .context: nil
        }
    }
}

/// A whole-row toggle with a trailing chevron, as the iOS transcript draws it.
private struct RowDisclosureStyle: DisclosureGroupStyle {
    func makeBody(configuration: Configuration) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            Button {
                withAnimation(.snappy(duration: 0.2)) { configuration.isExpanded.toggle() }
            } label: {
                HStack(spacing: 6) {
                    configuration.label
                    Image(systemName: "chevron.right")
                        .font(.caption2.weight(.semibold))
                        .foregroundStyle(.tertiary)
                        .rotationEffect(.degrees(configuration.isExpanded ? 90 : 0))
                    Spacer(minLength: 0)
                }
                .contentShape(.rect)
            }
            .buttonStyle(.plain)
            .hoverHighlight(cornerRadius: 6, padding: 4)
            .pointingHandCursor()
            .accessibilityHint(configuration.isExpanded ? "Hides the call's detail" : "Shows the call's detail")
            if configuration.isExpanded { configuration.content }
        }
    }
}

/// A toggle that reads as a quiet line: its label, then a small chevron.
private struct QuietDisclosureStyle: DisclosureGroupStyle {
    func makeBody(configuration: Configuration) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            Button {
                withAnimation(.snappy(duration: 0.2)) { configuration.isExpanded.toggle() }
            } label: {
                HStack(spacing: 6) {
                    configuration.label
                    Image(systemName: "chevron.right")
                        .font(.caption2.weight(.semibold))
                        .foregroundStyle(.tertiary)
                        .rotationEffect(.degrees(configuration.isExpanded ? 90 : 0))
                }
                .contentShape(.rect)
            }
            .buttonStyle(.plain)
            .hoverHighlight(cornerRadius: 6, padding: 4)
            .pointingHandCursor()
            if configuration.isExpanded { configuration.content }
        }
    }
}

/// The one live line under a running turn: what it is doing now, such as
/// `Running npm test` or the heading of the thought it is writing, and how
/// long the turn has run, with a light sweeping across it. A new activity
/// fades in over the last.
struct WorkingIndicator: View {
    /// When the turn began, for its clock.
    var startedAt: Date?
    var activity: String?
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        HStack(spacing: 6) {
            Text(activity ?? "Working")
                .lineLimit(1)
                .truncationMode(.tail)
                .contentTransition(.opacity)
                .animation(reduceMotion ? nil : .easeInOut(duration: 0.25), value: activity)
            if let startedAt {
                TimelineView(.periodic(from: startedAt, by: 1)) { context in
                    Text("· " + TurnTiming.clock(context.date.timeIntervalSince(startedAt)))
                        .monospacedDigit()
                }
            }
        }
        .font(.callout)
        .foregroundStyle(.secondary)
        .modifier(Shimmer(isOn: !reduceMotion))
        .accessibilityElement(children: .combine)
    }
}

/// A band of light sweeping left to right across the content, as a live
/// status line shows it is still working.
private struct Shimmer: ViewModifier {
    let isOn: Bool
    @State private var phase = -1.0

    func body(content: Content) -> some View {
        content
            .overlay {
                if isOn {
                    GeometryReader { geometry in
                        LinearGradient(colors: [.clear, Color.primary.opacity(0.55), .clear],
                                       startPoint: .leading, endPoint: .trailing)
                            .frame(width: geometry.size.width * 0.35)
                            .offset(x: phase * geometry.size.width)
                    }
                    .mask(content)
                    .allowsHitTesting(false)
                }
            }
            .onAppear {
                withAnimation(.linear(duration: 1.8).repeatForever(autoreverses: false)) { phase = 1 }
            }
    }
}

/// What a message's context menu can ask the chat to do.
struct MessageActions {
    var edit: (String) -> Void
    /// Run a host command again.
    var retry: (String) -> Void
    /// Send message `id` again in its place (#2278).
    var retryMessage: (Int) -> Void
}

extension EnvironmentValues {
    @Entry var messageActions: MessageActions? = nil
    /// The open chat's project directory, for paths shown relative to it.
    @Entry var projectRoot: String? = nil
    /// File edits start open on their diff (edits verbosity).
    @Entry var editsOpenByDefault = true
    /// Opens or folds the run of tool calls a summary row stands for.
    @Entry var toggleToolRun: @MainActor (Int) -> Void = { _ in }
    /// Opens or folds a long message; without it, long messages stay whole.
    @Entry var toggleMessage: MessageToggle? = nil
    /// Opens a subagent's panel by its id; without it, agent rows only show.
    @Entry var openAgent: (@MainActor (Int) -> Void)? = nil
}

/// Opens or folds a long message of the chat (#2303). One object for the
/// chat's life: a new closure on every redraw would change the environment
/// of every message and redraw them all.
@MainActor
final class MessageToggle {
    var action: (Int) -> Void = { _ in }
}

/// A long message folded to its first `MessageCollapse.maxLines` lines
/// under a fade, with Show more; opened, it ends with Show less (#2303).
/// Short messages, and every message where folding is off, are drawn as
/// they are. Folding is drawing only: the text, its selection, copy, the
/// message menu and VoiceOver keep the whole message.
///
/// Whether a message folds is decided as it is laid out, from its height at
/// the chat's width, not afterwards: a row that took its full height first
/// and folded the next pass would change height under the reader each time
/// it is drawn again, as rows are while the chat scrolls.
struct FoldingMessage<Content: View, Chrome: ViewModifier>: View {
    let id: Int
    let text: String
    /// This kind of message folds when long.
    let folds: Bool
    let isExpanded: Bool
    /// What goes around the text, such as a bubble, and how much taller it is.
    let chrome: Chrome
    var chromeHeight: Double = 0
    @ViewBuilder let content: () -> Content
    @Environment(\.toggleMessage) private var toggle

    var body: some View {
        if folds, let toggle, MessageCollapse.mayBeLong(text) {
            let lineHeight = chatLineHeight
            // The message whole when that is no taller than folded with its
            // control, else folded or opened with the control under it.
            ProposedHeight(height: MessageCollapse.foldHeight(lineHeight: lineHeight) + chromeHeight) {
                ViewThatFits(in: .vertical) {
                    whole
                    // The control inside the chrome, at its bottom right.
                    VStack(alignment: .leading, spacing: 0) {
                        if isExpanded {
                            content().fixedSize(horizontal: false, vertical: true)
                        } else {
                            folded(lineHeight)
                        }
                        control(toggle).frame(maxWidth: .infinity, alignment: .trailing)
                    }
                    .modifier(chrome)
                }
            }
        } else {
            content().modifier(chrome)
        }
    }

    private var whole: some View {
        content().fixedSize(horizontal: false, vertical: true).modifier(chrome)
    }

    private func folded(_ lineHeight: Double) -> some View {
        content()
            .fixedSize(horizontal: false, vertical: true)
            .frame(height: MessageCollapse.foldedHeight(lineHeight: lineHeight), alignment: .top)
            .clipped()
            .mask {
                VStack(spacing: 0) {
                    Rectangle()
                    LinearGradient(colors: [.black, .black.opacity(0)], startPoint: .top, endPoint: .bottom)
                        .frame(height: lineHeight * 2)
                }
            }
    }

    private func control(_ toggle: MessageToggle) -> some View {
        Button { toggle.action(id) } label: {
            HStack(spacing: 4) {
                Text(isExpanded ? "Show less" : "Show more")
                Image(systemName: isExpanded ? "chevron.up" : "chevron.down").imageScale(.small)
            }
            .font(.caption.weight(.medium))
            .foregroundStyle(.secondary)
            .padding(.horizontal, 8)
            .frame(height: MessageCollapse.controlHeight)
            .contentShape(.rect(cornerRadius: 7))
        }
        .buttonStyle(QuietPressStyle())
        .hoverHighlight(cornerRadius: 7)
        .padding(.trailing, -8)
        .help(isExpanded ? "Fold this message" : "Show the whole message")
        .accessibilityHint(isExpanded ? "Folds the message to its first lines" : "Shows the whole message")
    }
}

extension FoldingMessage where Chrome == EmptyModifier {
    init(id: Int, text: String, folds: Bool, isExpanded: Bool, @ViewBuilder content: @escaping () -> Content) {
        self.init(id: id, text: text, folds: folds, isExpanded: isExpanded, chrome: EmptyModifier(), content: content)
    }
}

/// A line of the chat's text: the body font and the rows' line spacing.
private let chatLineHeight =
    Double(NSLayoutManager().defaultLineHeight(for: .systemFont(ofSize: NSFont.systemFontSize))) + 2

/// Proposes `height` to its content, which takes whatever height it wants:
/// a `ViewThatFits` inside then picks by that height, where the chat's
/// scroll view proposes none.
private struct ProposedHeight: Layout {
    let height: Double

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        subviews.first?.sizeThatFits(ProposedViewSize(width: proposal.width, height: height)) ?? .zero
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        subviews.first?.place(at: bounds.origin, proposal: ProposedViewSize(width: bounds.width, height: height))
    }
}

extension ToolVerbosity {
    /// Where the app starts before the reader picks a level: the terminal's
    /// `verbosity` in `~/.config/rustcode/config.toml`, else high.
    static let terminalDefault: ToolVerbosity = {
        let path = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".config/rustcode/config.toml")
        return (try? String(contentsOf: path, encoding: .utf8)).flatMap(ToolVerbosity.configured(in:)) ?? .high
    }()
}

/// The end of a chat whose turn the provider or the connection stopped: why,
/// and a button that sends `continue`, so a turn that stopped while the
/// reader was away picks up from where it was.
struct StoppedTurnRow: View {
    let reason: String
    let resume: () -> Void

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: "exclamationmark.octagon.fill").foregroundStyle(.orange)
            Text(reason).font(.callout).foregroundStyle(.secondary)
            Spacer(minLength: 8)
            Button("Continue", action: resume)
                .buttonStyle(PillButtonStyle())
                .help("Send \"continue\" so the turn picks up where it stopped")
        }
        .padding(.vertical, 10)
        .padding(.horizontal, 14)
        .background(RoundedRectangle(cornerRadius: 12).fill(Color.orange.opacity(0.08)))
        .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(Color.orange.opacity(0.3), lineWidth: 1))
    }
}

extension View {
    /// A step of an opened turn: a rail down the left through every step,
    /// with a mark where each one starts.
    func pipelineStep(_ node: PipelineNode) -> some View {
        padding(.leading, 30)
            .background(alignment: .topLeading) {
                // Through the gap to the next row too, so the rail is unbroken.
                Rectangle().fill(Color.secondary.opacity(0.22)).frame(width: 1.5)
                    .padding(.bottom, -18)
                    .padding(.leading, 8.25)
            }
            .overlay(alignment: .topLeading) { PipelineDot(node: node).padding(.top, 2) }
    }
}

/// What a step of an opened turn is, as its mark on the rail shows it.
enum PipelineNode {
    case said, call, failed, thought
}

private struct PipelineDot: View {
    let node: PipelineNode

    var body: some View {
        ZStack {
            Circle().fill(Color.chatBackground).frame(width: 13, height: 13)
            switch node {
            case .said: Circle().fill(Color.primary.opacity(0.75)).frame(width: 7, height: 7)
            case .call: Circle().strokeBorder(Color.secondary, lineWidth: 1.5).frame(width: 9, height: 9)
            case .failed: Circle().strokeBorder(Color.red.opacity(0.85), lineWidth: 1.5).frame(width: 9, height: 9)
            case .thought: Circle().fill(Color.secondary.opacity(0.45)).frame(width: 5, height: 5)
            }
        }
        .frame(width: 18, height: 13)
        .accessibilityHidden(true)
    }
}

/// A thought in an opened turn: its headings on one quiet line, where a
/// reasoning box would have stood; the whole of it in the tooltip and Copy.
private struct ThoughtLine: View {
    let text: String

    private var lines: [String] {
        text.replacingOccurrences(of: "**", with: "")
            .split(whereSeparator: \.isNewline)
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .filter { !$0.isEmpty }
    }

    /// The `**Heading**` lines a summary is made of, else its first line.
    private var headings: [String] {
        let marked = text.split(whereSeparator: \.isNewline)
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .filter { $0.count > 4 && $0.hasPrefix("**") && $0.hasSuffix("**") }
            .map { $0.replacingOccurrences(of: "**", with: "") }
        return marked.isEmpty ? Array(lines.prefix(1)) : marked
    }

    var body: some View {
        let lines = lines
        Text(headings.joined(separator: " · "))
            .font(.callout)
            .italic()
            .foregroundStyle(.tertiary)
            .lineLimit(1)
            .truncationMode(.tail)
            .help(lines.joined(separator: "\n"))
            .contextMenu {
                Button("Copy Thinking", systemImage: "doc.on.doc") { Clipboard.copy(text) }
            }
            .accessibilityLabel(lines.joined(separator: ", "))
    }
}

/// A quiet row: what it says, then a small chevron that turns as it opens.
private struct ActivityRow<Extra: View>: View {
    let text: String
    let isOpen: Bool
    let help: String
    let toggle: () -> Void
    @ViewBuilder var extra: () -> Extra

    var body: some View {
        Button(action: toggle) {
            HStack(spacing: 6) {
                Text(text)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.tail)
                extra()
                Image(systemName: "chevron.right")
                    .font(.caption2.weight(.semibold))
                    .foregroundStyle(.tertiary)
                    .rotationEffect(.degrees(isOpen ? 90 : 0))
            }
            .font(.callout)
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .hoverHighlight(cornerRadius: 6, padding: 4)
        .pointingHandCursor()
        .help(help)
        .animation(.snappy(duration: 0.15), value: isOpen)
    }
}

private struct FailedCount: View {
    let count: Int
    var body: some View {
        Text(count == 1 ? "· failed" : "· \(count) failed").foregroundStyle(.red.opacity(0.85))
    }
}

/// The row a finished turn's work folds into: `Worked for 1m 29s`. A click
/// shows the work as it happened.
private struct TurnSummary: View {
    let fold: TurnFold
    @Environment(\.toggleToolRun) private var toggle

    var body: some View {
        ActivityRow(text: fold.title, isOpen: fold.isOpen,
                    help: fold.isOpen ? "Hide the work" : "Show the work") {
            toggle(fold.key)
        } extra: {
            if fold.elapsed != nil {
                Text("· \(fold.calls) tool \(fold.calls == 1 ? "call" : "calls")").foregroundStyle(.tertiary)
            }
            if fold.failed > 0 { FailedCount(count: fold.failed) }
        }
    }
}

/// The row a run of tool calls shows: what they did, in words. A click opens
/// the run to its calls, and folds it again.
private struct ToolRunSummary: View {
    let fold: ToolRunFold
    @Environment(\.toggleToolRun) private var toggle
    @Environment(\.projectRoot) private var projectRoot

    var body: some View {
        ActivityRow(text: fold.label, isOpen: fold.isOpen, help: help) {
            toggle(fold.run)
        } extra: {
            if fold.failed > 0 { FailedCount(count: fold.failed) }
            if let lines = fold.lines { DiffCount(added: lines.added, removed: lines.removed) }
        }
    }

    private var help: String {
        guard fold.edited.isEmpty else { return fold.edited.map { ProjectPath.relative($0, to: projectRoot) }.joined(separator: "\n") }
        return fold.isOpen ? "Hide these calls" : "Show these calls"
    }
}

/// Content as wide as it needs, scrolling sideways only when it is wider than
/// the chat. A nested scroller that is not needed still catches the wheel and
/// stalls the chat's own scrolling under the pointer.
struct HorizontalOverflow<Content: View>: View {
    @ViewBuilder let content: Content

    var body: some View {
        ViewThatFits(in: .horizontal) {
            content.fixedSize(horizontal: true, vertical: false)
            ScrollView(.horizontal) { content }
                .scrollBounceBehavior(.basedOnSize, axes: .horizontal)
        }
    }
}

/// Under a finished turn's answer: the files it changed and the checks it
/// ran, with how each ended as observed (#2649). Unknown and cancelled checks
/// say so; nothing here claims a check passed that did not report it. Show
/// Steps opens the turn's calls, where the output is. Off in Settings, it
/// does not show.
struct TurnCompletionCard: View {
    let completion: TurnCompletion
    @AppStorage(TurnCompletionCard.enabledKey) private var enabled = true
    @Environment(\.toggleToolRun) private var toggle
    @Environment(\.projectRoot) private var projectRoot
    @State private var showsFiles = false

    static let enabledKey = "showsTurnCompletion"

    var body: some View {
        if enabled {
            VStack(alignment: .leading, spacing: 6) {
                HStack(spacing: 10) {
                    if !completion.edited.isEmpty {
                        Button {
                            withAnimation(.snappy(duration: 0.15)) { showsFiles.toggle() }
                        } label: {
                            HStack(spacing: 5) {
                                Image(systemName: "doc.text")
                                Text("\(completion.edited.count) file\(completion.edited.count == 1 ? "" : "s") changed")
                                Text("+\(completion.added)").foregroundStyle(.green)
                                Text("\u{2212}\(completion.removed)").foregroundStyle(.red)
                            }
                        }
                        .buttonStyle(.plain)
                        .help(showsFiles ? "Hide the files" : "List the files")
                    }
                    if completion.checks.isEmpty {
                        Label("No checks ran", systemImage: "questionmark.circle")
                            .foregroundStyle(.secondary)
                            .help("This turn ran no tests, builds or lints that RustCode could see.")
                    }
                    Spacer(minLength: 0)
                    Button("Show Steps") { toggle(TurnFold.key(for: completion.anchor)) }
                        .buttonStyle(.link)
                        .help("Open the turn's calls and their output")
                }
                if !completion.checks.isEmpty {
                    VStack(alignment: .leading, spacing: 3) {
                        ForEach(Array(completion.checks.enumerated()), id: \.offset) { _, check in
                            CheckLine(check: check)
                        }
                    }
                }
                if completion.failedCommands > 0 {
                    Label("\(completion.failedCommands) other command\(completion.failedCommands == 1 ? "" : "s") failed",
                          systemImage: "exclamationmark.triangle")
                        .foregroundStyle(.orange)
                }
                if showsFiles {
                    VStack(alignment: .leading, spacing: 2) {
                        ForEach(completion.edited, id: \.self) { path in
                            Text(ProjectPath.relative(path, to: projectRoot))
                                .font(.caption.monospaced())
                                .lineLimit(1)
                                .truncationMode(.middle)
                                .textSelection(.enabled)
                        }
                    }
                }
            }
            .font(.caption)
            .padding(.horizontal, 10)
            .padding(.vertical, 8)
            .background(.fill.quaternary, in: .rect(cornerRadius: 10))
            .accessibilityElement(children: .contain)
            .accessibilityLabel("What this turn changed and checked")
        }
    }

    private struct CheckLine: View {
        let check: TurnCompletion.Check

        var body: some View {
            let (symbol, color, word): (String, Color, String) = switch check.outcome {
            case .passed: ("checkmark.circle.fill", .green, "passed")
            case .failed: ("xmark.circle.fill", .red, "failed")
            case .cancelled: ("stop.circle", .secondary, "stopped")
            case .unknown: ("questionmark.circle", .secondary, "no result")
            }
            HStack(spacing: 6) {
                Image(systemName: symbol).foregroundStyle(color)
                Text(check.command).font(.caption.monospaced()).lineLimit(1).truncationMode(.tail)
                Text(word).foregroundStyle(.secondary)
            }
            .help(check.command)
            .accessibilityElement(children: .ignore)
            .accessibilityLabel("\(check.command): \(word)")
        }
    }
}
