import RustCodeRemote
import SwiftUI

struct TranscriptRow: View, Equatable {
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
        case .notice:
            EmptyView()
        }
    }
}

struct TurnFooter: View {
    var timing: TurnTiming? = nil
    var legacyCompletedAt: String? = nil

    var body: some View {
        Group {
            if let line = TimingPresentation.footerLine(timing: timing, legacyCompletedAt: legacyCompletedAt) {
                Text(line)
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
        .accessibilityIdentifier("turnFooter")
    }
}

private struct UserMessage: View {
    let text: String
    @State private var selectedImage: ImageViewerSelection?

    private var imageIDs: [String] { ImageAttachmentPresentation.attachmentIDs(in: text) }

    private var displayText: String {
        ImageAttachmentPresentation.displayText(text)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            let message = ImageAttachmentPresentation.textWithoutAttachments(text)
            if !message.isEmpty {
                Text(message)
            }
            if !imageIDs.isEmpty {
                LazyVGrid(columns: [GridItem(.flexible()), GridItem(.flexible())], spacing: 6) {
                    ForEach(imageIDs, id: \.self) { id in
                        SentImageThumbnail(attachmentID: id) {
                            selectedImage = ImageViewerSelection(id: id)
                        }
                    }
                }
                .accessibilityLabel("Attached images")
            }
        }
            .padding(.horizontal, 16)
            .padding(.vertical, 10)
            .background(.fill.tertiary, in: .rect(cornerRadius: 20))
            .contentShape(.contextMenuPreview, .rect(cornerRadius: 20))
            .contextMenu { TextActions(text: text) }
            .frame(maxWidth: .infinity, alignment: .trailing)
            .padding(.leading, 48)
            .accessibilityLabel("You: \(displayText)")
            .fullScreenCover(item: $selectedImage) { selection in
                SentImageViewer(attachmentID: selection.id)
            }
    }
}

private struct ImageViewerSelection: Identifiable {
    let id: String
}

private struct AssistantMessage: View {
    let assistant: AssistantText

    private var blocks: [AssistantBlock] {
        AssistantBlock.parse(assistant.text, isStreaming: assistant.isStreaming)
    }

    private var summary: AssistantSummaryPresentation? {
        guard !assistant.isStreaming else { return nil }
        return AssistantSummaryPresentation.parse(assistant.text)
    }

    private var readableText: String {
        if let summary {
            var text = "Summary\n\(summary.summary)"
            if let nextAction = summary.nextAction { text += "\n\nNext action\n\(nextAction)" }
            return text
        }
        return blocks.map { block in
            switch block {
            case .markdown(let text): MarkdownText.plainText(text)
            case .thinking(let text, _): "Thinking\n" + MarkdownText.plainText(text)
            }
        }.joined(separator: "\n\n")
    }

    private var previewText: String {
        if let summary { return readableText }
        for case .markdown(let text) in blocks {
            if !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                return MarkdownText.plainText(text)
            }
        }
        return readableText
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            if let summary {
                AssistantSummaryCard(presentation: summary)
            } else {
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

private struct AssistantSummaryCard: View {
    let presentation: AssistantSummaryPresentation

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Label("Summary", systemImage: "text.quote")
                .font(.caption.weight(.semibold))
                .foregroundStyle(.secondary)
            Text(presentation.summary)
                .font(.body)
                .fixedSize(horizontal: false, vertical: true)
            if let nextAction = presentation.nextAction {
                Divider()
                VStack(alignment: .leading, spacing: 3) {
                    Text("Next action")
                        .font(.caption.weight(.medium))
                        .foregroundStyle(.secondary)
                    Text(nextAction)
                        .font(.subheadline)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(12)
        .background(.fill.quaternary, in: .rect(cornerRadius: 14))
        .accessibilityElement(children: .combine)
        .accessibilityLabel("Summary. \(presentation.summary)" + (presentation.nextAction.map { ". Next action: \($0)" } ?? ""))
        .accessibilityIdentifier("assistantSummaryCard")
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
                Text(ToolPresentation.title(tool.name))
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
        .accessibilityLabel("\(ToolPresentation.title(tool.name)), \(statusDescription). \(tool.summary)")
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
