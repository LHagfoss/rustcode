import RustCodeRemote
import SwiftUI

struct TranscriptRow: View {
    let item: TranscriptItem
    var footer: TurnTiming? = nil

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            content
            if let footer {
                TurnFooter(timing: footer)
            } else if item.turnTiming == nil, case .assistant(let assistant) = item.content,
                      let completedAt = assistant.completedAt {
                TurnFooter(legacyCompletedAt: completedAt)
            }
        }
    }

    @ViewBuilder
    private var content: some View {
        switch item.content {
        case .user(let text):
            UserMessage(text: text)
        case .assistant(let assistant):
            AssistantMessage(assistant: assistant)
        case .tool(let tool):
            ToolRow(tool: tool)
        case .subagent(let subagent):
            SubagentRow(subagent: subagent)
        case .notice(let text):
            Text(text)
                .font(.footnote)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
                .frame(maxWidth: .infinity, alignment: .center)
                .contextMenu {
                    Button("Copy", systemImage: "doc.on.doc") { Clipboard.copy(text) }
                }
        }
    }
}

struct TurnFooter: View {
    var timing: TurnTiming? = nil
    var legacyCompletedAt: String? = nil

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            if let timing, timing.outcome != nil {
                Text(TimingPresentation.turnLabel(timing))
                    .font(.caption).foregroundStyle(.secondary)
            }
            if let timestamp = TimingPresentation.endTimestamp(timing: timing, legacy: legacyCompletedAt) {
                let prefix = TimingPresentation.endLabel(outcome: timing?.outcome)
                Group {
                    if let date = TimingPresentation.date(timestamp) {
                        Text("\(prefix) \(date.formatted(date: .abbreviated, time: .standard))")
                    } else {
                        Text("\(prefix) \(timestamp)")
                    }
                }
                .font(.caption).foregroundStyle(.tertiary)
            }
        }
        .accessibilityIdentifier("turnFooter")
    }
}

private struct UserMessage: View {
    let text: String

    private var displayText: String { ImageAttachmentPresentation.displayText(text) }

    var body: some View {
        Text(displayText)
            .padding(.horizontal, 16)
            .padding(.vertical, 10)
            .background(.fill.tertiary, in: .rect(cornerRadius: 20))
            .contentShape(.contextMenuPreview, .rect(cornerRadius: 20))
            .contextMenu { TextActions(text: text) }
            .frame(maxWidth: .infinity, alignment: .trailing)
            .padding(.leading, 48)
            .accessibilityLabel("You: \(displayText)")
    }
}

private struct AssistantMessage: View {
    let assistant: AssistantText

    private var blocks: [AssistantBlock] {
        AssistantBlock.parse(assistant.text, isStreaming: assistant.isStreaming)
    }

    private var readableText: String {
        blocks.map { block in
            switch block {
            case .markdown(let text): MarkdownText.plainText(text)
            case .thinking(let text, _): "Thinking\n" + MarkdownText.plainText(text)
            }
        }.joined(separator: "\n\n")
    }

    private var previewText: String {
        for case .markdown(let text) in blocks {
            if !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                return MarkdownText.plainText(text)
            }
        }
        return readableText
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            ForEach(Array(blocks.enumerated()), id: \.offset) { index, block in
                switch block {
                case .markdown(let text):
                    MarkdownText(text: text)
                case .thinking(let text, let isComplete):
                    ThinkingBlock(text: text, isThinking: assistant.isStreaming && !isComplete,
                        title: TimingPresentation.thinkingLabel(at: index, in: blocks,
                            milliseconds: assistant.thoughtTimeMilliseconds,
                            tokens: assistant.thoughtTokens, estimated: assistant.thoughtTokensEstimated) ?? "Thinking")
                }
            }
            if assistant.isStreaming {
                StreamingIndicator()
            }
            if let response = assistant.responseTimeMilliseconds {
                Text("Response · \(TimingPresentation.duration(response))")
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
        .contentShape(.contextMenuPreview, .rect(cornerRadius: 12))
        .contextMenu {
            TextActions(text: assistant.text, readableText: readableText)
        } preview: {
            MessageActionPreview(text: previewText)
        }
    }
}

private struct ThinkingBlock: View {
    let text: String
    let isThinking: Bool
    var title = "Thinking"
    @State private var isExpanded = false

    var body: some View {
        DisclosureGroup(isExpanded: $isExpanded) {
            MarkdownText(text: text)
                .foregroundStyle(.secondary)
                .padding(.top, 8)
                .padding(.bottom, 12)
                .fixedSize(horizontal: false, vertical: true)
                .contextMenu { TextActions(text: text, readableText: MarkdownText.plainText(text)) } preview: {
                    MessageActionPreview(text: MarkdownText.plainText(text))
                }
        } label: {
            HStack(spacing: 8) {
                Label(title, systemImage: "brain")
                if isThinking { ProgressView().controlSize(.mini).accessibilityHidden(true) }
                Spacer(minLength: 0)
            }
            .font(.subheadline.weight(.medium))
            .foregroundStyle(.secondary)
            .frame(minHeight: 44)
        }
        .disclosureGroupStyle(ToolDisclosureStyle())
        .sensoryFeedback(.selection, trigger: isExpanded)
        .padding(.horizontal, 12)
        .background(.fill.quaternary, in: .rect(cornerRadius: 12))
    }
}

struct StreamingIndicator: View {
    var body: some View {
        Image(systemName: "ellipsis")
            .font(.title3)
            .foregroundStyle(.secondary)
            .symbolEffect(.variableColor.iterative, options: .repeating)
            .accessibilityLabel("Working")
    }
}

private struct ToolRow: View {
    let tool: ToolCall
    @State private var isExpanded = LaunchOptions.current.expandsTools

    private var hasDetail: Bool { tool.input != nil || tool.output != nil }

    var body: some View {
        Group {
            if hasDetail {
                DisclosureGroup(isExpanded: $isExpanded) {
                    VStack(alignment: .leading, spacing: 10) {
                        if let input = tool.input {
                            detail("Input", input)
                        }
                        if let output = tool.output {
                            detail(tool.isOutputTruncated ? "Output (truncated)" : "Output", output)
                        }
                    }
                    .padding(.top, 12)
                } label: {
                    label
                }
                .disclosureGroupStyle(ToolDisclosureStyle())
            } else {
                label
            }
        }
        .padding(12)
        .background(.fill.quaternary, in: .rect(cornerRadius: 14))
        .contentShape(.contextMenuPreview, .rect(cornerRadius: 14))
        .contextMenu {
            Button("Copy Summary", systemImage: "doc.on.doc") {
                Clipboard.copy(tool.summary)
            }
            if let input = tool.input {
                Button("Copy Input", systemImage: "arrow.down.doc") {
                    Clipboard.copy(input)
                }
            }
            if let output = tool.output {
                Button("Copy Output", systemImage: "arrow.up.doc") {
                    Clipboard.copy(output)
                }
            }
            if hasDetail {
                Divider()
                Button(
                    isExpanded ? "Collapse" : "Expand",
                    systemImage: isExpanded ? "rectangle.compress.vertical" : "rectangle.expand.vertical"
                ) {
                    withAnimation(.snappy(duration: 0.2)) { isExpanded.toggle() }
                }
            }
        }
    }

    private var label: some View {
        HStack(spacing: 10) {
            statusIcon
                .frame(width: 22)
            VStack(alignment: .leading, spacing: 2) {
                Text(tool.name)
                    .font(.subheadline.weight(.medium))
                    .foregroundStyle(.primary)
                Text(tool.summary)
                    .font(.caption.monospaced())
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            Spacer(minLength: 8)
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Tool \(tool.name), \(statusDescription). \(tool.summary)")
    }

    private func detail(_ title: String, _ text: String) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(title)
                .font(.caption.weight(.semibold))
                .foregroundStyle(.secondary)
            ScrollView(.horizontal) {
                Text(text)
                    .font(.caption.monospaced())
                    .foregroundStyle(.primary)
                    .textSelection(.enabled)
            }
            .scrollIndicators(.hidden)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    @ViewBuilder
    private var statusIcon: some View {
        switch tool.status {
        case .running:
            ProgressView().controlSize(.small)
        case .succeeded:
            Image(systemName: "checkmark.circle.fill").foregroundStyle(.green)
        case .failed:
            Image(systemName: "xmark.circle.fill").foregroundStyle(.red)
        }
    }

    private var statusDescription: String {
        switch tool.status {
        case .running: "running"
        case .succeeded: "succeeded"
        case .failed: "failed"
        }
    }
}

/// The system disclosure row, with a quiet chevron instead of a tinted one.
private struct ToolDisclosureStyle: DisclosureGroupStyle {
    func makeBody(configuration: Configuration) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            Button {
                withAnimation(.snappy(duration: 0.2)) { configuration.isExpanded.toggle() }
            } label: {
                HStack(spacing: 8) {
                    configuration.label
                    Image(systemName: "chevron.right")
                        .font(.footnote.weight(.semibold))
                        .foregroundStyle(.tertiary)
                        .rotationEffect(.degrees(configuration.isExpanded ? 90 : 0))
                        .accessibilityHidden(true)
                }
                .contentShape(.rect)
            }
            .buttonStyle(.plain)
            .accessibilityValue(configuration.isExpanded ? "Expanded" : "Collapsed")
            .accessibilityHint(configuration.isExpanded ? "Collapses the details" : "Expands the details")

            if configuration.isExpanded {
                configuration.content
            }
        }
    }
}

struct SubagentRow: View {
    let subagent: SubagentSummary

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: "person.2.fill")
                .foregroundStyle(.tint)
                .frame(width: 22)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                    Text(subagent.name)
                        .font(.subheadline.weight(.medium))
                    Text(statusDescription)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    if subagent.status == .running {
                        ProgressView().controlSize(.mini)
                    }
                }
                Text(subagent.task)
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                if let detail = subagent.detail {
                    Text(detail)
                        .font(.footnote)
                        .foregroundStyle(.tertiary)
                }
            }
            Spacer(minLength: 0)
        }
        .padding(12)
        .background(.fill.quaternary, in: .rect(cornerRadius: 14))
        .contentShape(.contextMenuPreview, .rect(cornerRadius: 14))
        .contextMenu {
            Button("Copy Task", systemImage: "doc.on.doc") {
                Clipboard.copy(subagent.task)
            }
            if let detail = subagent.detail {
                Button("Copy Details", systemImage: "doc.on.doc") {
                    Clipboard.copy(detail)
                }
            }
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel("Sub-agent \(subagent.name), \(statusDescription). \(subagent.task)")
    }

    private var statusDescription: String {
        switch subagent.status {
        case .queued: "queued"
        case .running: "running"
        case .interrupted: "interrupted"
        case .completed: "completed"
        case .failed: "failed"
        case .cancelled: "cancelled"
        }
    }
}
