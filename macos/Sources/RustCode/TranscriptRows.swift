import RustCodeKit
import SwiftUI

struct TranscriptRow: View, Equatable {
    let item: TranscriptItem
    var isCurrentMatch = false
    /// This is the reply the agent is still writing.
    var isStreaming = false
    /// The folded run of tool calls this row starts, if any.
    var fold: ToolRunFold? = nil

    var body: some View {
        content
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

    @ViewBuilder
    private var content: some View {
        switch item.content {
        case .user(let text): UserMessage(id: item.id, text: text)
        case .assistant(let text): AssistantMessage(text: text, isStreaming: isStreaming)
        case .thinking(let text):
            ThinkingBlock(text: text, isLive: isStreaming, duration: item.duration, isSummary: item.isReasoningSummary)
        case .tool(let tool):
            if let fold {
                VStack(alignment: .leading, spacing: 18) {
                    ToolRunSummary(fold: fold)
                    if fold.isOpen, !tool.isBlank { ToolRow(tool: tool) }
                }
            } else if !tool.isBlank {
                ToolRow(tool: tool)
            }
        case .command(let input, let output): CommandCard(input: input, output: output)
        }
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
                UserBubble(id: id, text: message.text, original: text)
            }
        }
    }
}

private struct UserBubble: View {
    let id: Int
    let text: String
    /// The message as sent, image markers included, for edit and resend.
    let original: String
    @Environment(\.messageActions) private var messageActions
    @Environment(\.findQuery) private var findQuery

    var body: some View {
        Group {
            if text.hasPrefix("/"), findQuery.isEmpty {
                slashCommandText(text)
            } else {
                Text(FindHighlight.apply(AttributedString(text), query: findQuery))
            }
        }
            .textSelection(.enabled)
            .lineSpacing(2)
            .padding(.horizontal, 14)
            .padding(.vertical, 9)
            .background(.fill.tertiary, in: .rect(cornerRadius: 18))
            .contextMenu {
                Button("Copy", systemImage: "doc.on.doc") { Clipboard.copy(text) }
                if let actions = messageActions {
                    Divider()
                    Button("Edit and Resend", systemImage: "pencil") { actions.edit(original) }
                    Button("Retry", systemImage: "arrow.clockwise") { actions.retryMessage(id) }
                }
            }
            .frame(maxWidth: .infinity, alignment: .trailing)
            .padding(.leading, 80)
    }
}

/// Agent text, with `<think>` sections the engine leaves inline folded away.
private struct AssistantMessage: View {
    let text: String
    var isStreaming = false
    @Environment(\.messageActions) private var messageActions

    private struct Parsed {
        var blocks: [AssistantBlock]
        var summary: AssistantSummary?
    }

    private static let cache = RenderCache<Parsed>(countLimit: 1_000)

    /// Split once per text: finding the summary tries to read JSON at every
    /// `{` from the end, too slow to repeat for every message on every redraw.
    private var parsed: Parsed {
        Self.cache.value(for: (isStreaming ? "s\u{0}" : "\u{0}") + text) {
            let (prose, summary) = AssistantSummary.split(text)
            return Parsed(blocks: AssistantBlock.parse(prose, isStreaming: isStreaming), summary: summary)
        }
    }

    var body: some View {
        let parsed = parsed
        VStack(alignment: .leading, spacing: 10) {
            ForEach(Array(parsed.blocks.enumerated()), id: \.offset) { index, block in
                let isLast = index == parsed.blocks.count - 1
                switch block {
                case .markdown(let text):
                    MarkdownText(text: text, visualizations: true, isStreaming: isStreaming && isLast)
                case .thinking(let text, let isComplete):
                    ThinkingBlock(text: text, isLive: isStreaming && isLast && !isComplete)
                }
            }
            if let summary = parsed.summary { SummaryCard(summary: summary) }
        }
        .contextMenu {
            Button("Copy Reply", systemImage: "doc.on.doc") { Clipboard.copy(AssistantSummary.split(text).prose) }
            Button("Copy as Markdown", systemImage: "text.document") { Clipboard.copy(text) }
            if let actions = messageActions {
                Divider()
                Button("Quote in Reply", systemImage: "text.quote") {
                    actions.edit(AssistantSummary.split(text).prose.split(separator: "\n").map { "> " + $0 }.joined(separator: "\n") + "\n\n")
                }
            }
        }
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
            HStack(spacing: 8) {
                Label(title, systemImage: "brain")
                    .contentTransition(.opacity)
                if isLive { ProgressView().controlSize(.mini) }
            }
            .font(.callout.weight(.medium))
            .foregroundStyle(.secondary)
        }
        .disclosureGroupStyle(RowDisclosureStyle())
        .padding(.horizontal, 12)
        .padding(.vertical, 9)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.fill.quaternary, in: .rect(cornerRadius: 12))
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

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            card
            if let liveTail {
                Text(liveTail)
                    .font(.system(.caption, design: .monospaced))
                    .foregroundStyle(.secondary)
                    .lineLimit(Self.liveTailLines)
                    .truncationMode(.head)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.leading, 30)
                    .transition(.opacity)
            }
        }
        .card()
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
                    .padding(.top, 10)
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
        return tool.detail
    }

    private var label: some View {
        HStack(spacing: 10) {
            statusIcon.frame(width: 20)
            VStack(alignment: .leading, spacing: 2) {
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    Text(tool.displayTitle)
                        .font(.callout.weight(.medium))
                        .lineLimit(1)
                        .layoutPriority(1)
                    // What the model said the call does.
                    if let summary = tool.summary {
                        Text(summary)
                            .font(.callout)
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                            .truncationMode(.tail)
                    }
                }
                if let detail = detailLine {
                    Text(detail)
                        .font(.caption.monospaced())
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
            }
            if !tool.changes.isEmpty {
                Spacer(minLength: 8)
                DiffCount(
                    added: tool.changes.reduce(0) { $0 + $1.added },
                    removed: tool.changes.reduce(0) { $0 + $1.removed })
            }
        }
    }

    @ViewBuilder
    private var statusIcon: some View {
        switch tool.status {
        case .completed:
            Image(systemName: "checkmark.circle.fill").foregroundStyle(.green)
        case .failed:
            Image(systemName: "xmark.circle.fill").foregroundStyle(.red)
        case .pending, .inProgress:
            ProgressView().controlSize(.small)
        case nil:
            Image(systemName: "wrench.and.screwdriver").foregroundStyle(.secondary)
        }
    }

    private func detail(_ title: String, _ text: String) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(title).font(.caption.weight(.semibold)).foregroundStyle(.secondary)
            ToolTextBox(text: text)
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
private struct FileChangeView: View {
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
            HStack(spacing: 8) {
                if showsHeader {
                    Image(systemName: change.isNewFile ? "doc.badge.plus" : "doc.text")
                        .foregroundStyle(.secondary)
                    Text(change.displayPath(relativeTo: projectRoot))
                        .font(.caption.monospaced())
                        .lineLimit(1)
                        .truncationMode(.middle)
                    DiffCount(added: change.added, removed: change.removed)
                }
                Spacer(minLength: 8)
                FileActions(path: change.path).buttons
            }
            diff(Array(visibleRows))
            .background(.background.opacity(0.5), in: .rect(cornerRadius: 8))
            .clipShape(.rect(cornerRadius: 8))
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
                HStack {
                    configuration.label
                    Spacer(minLength: 8)
                    Image(systemName: "chevron.right")
                        .font(.caption.weight(.semibold))
                        .foregroundStyle(.tertiary)
                        .rotationEffect(.degrees(configuration.isExpanded ? 90 : 0))
                }
                .contentShape(.rect)
            }
            .buttonStyle(.plain)
            if configuration.isExpanded { configuration.content }
        }
    }
}

/// Shown while a turn runs and the agent has not started writing yet.
struct WorkingIndicator: View {
    var body: some View {
        HStack(spacing: 8) {
            ProgressView().controlSize(.small)
            Text("Working…").font(.callout).foregroundStyle(.secondary)
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

/// The row a folded run of tool calls shows: how many, how many failed, and
/// what they were. A click opens the run to its cards, and folds it again.
private struct ToolRunSummary: View {
    let fold: ToolRunFold
    @Environment(\.toggleToolRun) private var toggle

    var body: some View {
        Button { toggle(fold.run) } label: {
            HStack(spacing: 8) {
                Image(systemName: "chevron.right")
                    .font(.caption.weight(.semibold))
                    .rotationEffect(.degrees(fold.isOpen ? 90 : 0))
                    .foregroundStyle(.secondary)
                Image(systemName: "wrench.and.screwdriver").foregroundStyle(.secondary)
                Text("\(fold.count) tool call\(fold.count == 1 ? "" : "s")").font(.callout.weight(.medium))
                if fold.failed > 0 {
                    Text("· \(fold.failed) failed").font(.callout).foregroundStyle(.red)
                }
                if let label = fold.label {
                    Text("· \(label)")
                        .font(.callout)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .truncationMode(.tail)
                }
                Spacer(minLength: 0)
            }
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .help(fold.isOpen ? "Fold these tool calls" : "Show these tool calls")
        .animation(.snappy(duration: 0.15), value: fold.isOpen)
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
