import RustCodeKit
import SwiftUI

struct TranscriptRow: View {
    let item: TranscriptItem
    var isCurrentMatch = false
    /// This is the reply the agent is still writing.
    var isStreaming = false

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
        case .user(let text): UserMessage(text: text)
        case .assistant(let text): AssistantMessage(text: text, isStreaming: isStreaming)
        case .thinking(let text): ThinkingBlock(text: text)
        case .tool(let tool): ToolRow(tool: tool)
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
                        Button("Send Again", systemImage: "arrow.clockwise") { actions.retry(text) }
                    }
                }
                .frame(maxWidth: .infinity, alignment: .trailing)
                .padding(.leading, 80)
            }
            if !message.text.isEmpty || message.images.isEmpty {
                UserBubble(text: message.text, original: text)
            }
        }
    }
}

private struct UserBubble: View {
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
                    Button("Send Again", systemImage: "arrow.clockwise") { actions.retry(original) }
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

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            let (prose, summary) = AssistantSummary.split(text)
            ForEach(Array(AssistantBlock.parse(prose).enumerated()), id: \.offset) { _, block in
                switch block {
                case .markdown(let text): MarkdownText(text: text, visualizations: true, isStreaming: isStreaming)
                case .thinking(let text, _): ThinkingBlock(text: text)
                }
            }
            if let summary { SummaryCard(summary: summary) }
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

private struct ThinkingBlock: View {
    let text: String
    @State private var isExpanded = false

    var body: some View {
        DisclosureGroup(isExpanded: $isExpanded) {
            Text(MarkdownText.inline(text.trimmingCharacters(in: .whitespacesAndNewlines)))
                .font(.callout)
                .foregroundStyle(.secondary)
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.leading, 12)
                .overlay(alignment: .leading) { Capsule().fill(.quaternary).frame(width: 2) }
                .padding(.top, 6)
        } label: {
            Label("Thinking", systemImage: "brain")
                .font(.callout)
                .foregroundStyle(.secondary)
        }
        .disclosureGroupStyle(RowDisclosureStyle())
    }
}

private struct ToolRow: View {
    let tool: ToolItem
    /// Nil until the user toggles it: an edit opens on its diff, anything
    /// else starts closed.
    @State private var expanded: Bool?
    @Environment(\.projectRoot) private var projectRoot

    private var hasDetail: Bool { tool.input != nil || tool.output != nil || !tool.changes.isEmpty }
    private var isExpanded: Binding<Bool> {
        Binding(get: { expanded ?? !tool.changes.isEmpty }, set: { expanded = $0 })
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
                Text(tool.displayTitle)
                    .font(.callout.weight(.medium))
                    .lineLimit(1)
                    .truncationMode(.middle)
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
            ScrollView([.vertical, .horizontal]) {
                Text(text)
                    .font(.system(.caption, design: .monospaced))
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(10)
            }
            .frame(maxHeight: 240)
            .fixedSize(horizontal: false, vertical: true)
            .background(.background.opacity(0.5), in: .rect(cornerRadius: 8))
        }
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

    private var numberWidth: CGFloat {
        let largest = change.rows.reduce(0) { result, row in
            guard case .line(let line) = row else { return result }
            return max(result, line.oldNumber ?? 0, line.newNumber ?? 0)
        }
        return CGFloat(max(String(largest).count, 2)) * 7.5
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
            ScrollView(.horizontal) {
                VStack(alignment: .leading, spacing: 0) {
                    ForEach(Array(visibleRows.enumerated()), id: \.offset) { _, row in
                        rowView(row)
                    }
                }
                .padding(.vertical, 4)
            }
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

    @ViewBuilder
    private func rowView(_ row: DiffRow) -> some View {
        switch row {
        case .gap(let count):
            Text(count.map { "\u{22EF} \($0) unchanged line\($0 == 1 ? "" : "s")" } ?? "\u{22EF}")
                .font(.caption.monospaced())
                .foregroundStyle(.tertiary)
                .padding(.leading, numberWidth * 2 + 28)
                .padding(.vertical, 2)
                .frame(maxWidth: .infinity, alignment: .leading)
        case .line(let line):
            HStack(spacing: 0) {
                number(line.oldNumber)
                number(line.newNumber)
                Text(sign(line.kind))
                    .foregroundStyle(tint(line.kind) ?? .secondary)
                    .frame(width: 16)
                Text(line.text.isEmpty ? " " : line.text)
                    .textSelection(.enabled)
                    .fixedSize()
                    .padding(.trailing, 10)
            }
            .font(.system(.caption, design: .monospaced))
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(tint(line.kind).map { $0.opacity(0.13) } ?? .clear)
        }
    }

    private func number(_ value: Int?) -> some View {
        Text(value.map(String.init) ?? "")
            .foregroundStyle(.tertiary)
            .frame(width: numberWidth, alignment: .trailing)
            .padding(.trailing, 6)
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
    var retry: (String) -> Void
}

extension EnvironmentValues {
    @Entry var messageActions: MessageActions? = nil
    /// The open chat's project directory, for paths shown relative to it.
    @Entry var projectRoot: String? = nil
}
