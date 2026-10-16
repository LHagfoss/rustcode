import RustCodeKit
import SwiftUI

struct TranscriptRow: View {
    let item: TranscriptItem
    var isCurrentMatch = false

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
        case .assistant(let text): AssistantMessage(text: text)
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
                    Button("Edit and Resend", systemImage: "pencil") { actions.edit(text) }
                    Button("Send Again", systemImage: "arrow.clockwise") { actions.retry(text) }
                }
            }
            .frame(maxWidth: .infinity, alignment: .trailing)
            .padding(.leading, 80)
    }
}

/// Agent text, with `<think>` sections the engine leaves inline folded away.
private struct AssistantMessage: View {
    let text: String
    @Environment(\.messageActions) private var messageActions

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            let (prose, summary) = AssistantSummary.split(text)
            ForEach(Array(AssistantBlock.parse(prose).enumerated()), id: \.offset) { _, block in
                switch block {
                case .markdown(let text): MarkdownText(text: text)
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
    @State private var isExpanded = false

    private var hasDetail: Bool { tool.input != nil || tool.output != nil }

    var body: some View {
        Group {
            if hasDetail {
                DisclosureGroup(isExpanded: $isExpanded) {
                    VStack(alignment: .leading, spacing: 10) {
                        if let input = tool.input { detail("Input", input) }
                        if let output = tool.output { detail("Output", output) }
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
        .card()
        .contextMenu {
            if let input = tool.input { Button("Copy Input", systemImage: "arrow.down.doc") { Clipboard.copy(input) } }
            if let output = tool.output { Button("Copy Output", systemImage: "arrow.up.doc") { Clipboard.copy(output) } }
        }
    }

    private var label: some View {
        HStack(spacing: 10) {
            statusIcon.frame(width: 20)
            VStack(alignment: .leading, spacing: 2) {
                Text(tool.displayTitle)
                    .font(.callout.weight(.medium))
                    .lineLimit(1)
                    .truncationMode(.middle)
                if let detail = tool.detail {
                    Text(detail)
                        .font(.caption.monospaced())
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
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
}
