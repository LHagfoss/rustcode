import RustCodeRemote
import SwiftUI

struct TranscriptRow: View {
    let item: TranscriptItem

    var body: some View {
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
                .frame(maxWidth: .infinity, alignment: .center)
        }
    }
}

private struct UserMessage: View {
    let text: String

    var body: some View {
        Text(text)
            .textSelection(.enabled)
            .padding(.horizontal, 16)
            .padding(.vertical, 10)
            .background(.fill.tertiary, in: .rect(cornerRadius: 20))
            .frame(maxWidth: .infinity, alignment: .trailing)
            .padding(.leading, 48)
            .accessibilityLabel("You: \(text)")
    }
}

private struct AssistantMessage: View {
    let assistant: AssistantText

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            MarkdownText(text: assistant.text)
                .textSelection(.enabled)
            if assistant.isStreaming {
                StreamingIndicator()
            }
        }
        .accessibilityElement(children: .combine)
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
    @State private var isExpanded = false

    private var hasDetail: Bool { tool.input != nil || tool.output != nil }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Button {
                withAnimation(.snappy(duration: 0.2)) { isExpanded.toggle() }
            } label: {
                HStack(spacing: 10) {
                    statusIcon
                        .frame(width: 22)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(tool.name)
                            .font(.subheadline.weight(.medium))
                        Text(tool.summary)
                            .font(.caption.monospaced())
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                            .truncationMode(.middle)
                    }
                    Spacer(minLength: 8)
                    if hasDetail {
                        Image(systemName: "chevron.right")
                            .font(.footnote.weight(.semibold))
                            .foregroundStyle(.tertiary)
                            .rotationEffect(.degrees(isExpanded ? 90 : 0))
                    }
                }
                .contentShape(.rect)
            }
            .buttonStyle(.plain)
            .disabled(!hasDetail)
            .accessibilityLabel("Tool \(tool.name), \(statusDescription). \(tool.summary)")
            .accessibilityHint(hasDetail ? (isExpanded ? "Collapses the details" : "Expands the details") : "")

            if isExpanded {
                VStack(alignment: .leading, spacing: 10) {
                    if let input = tool.input {
                        detail("Input", input)
                    }
                    if let output = tool.output {
                        detail(tool.isOutputTruncated ? "Output (truncated)" : "Output", output)
                    }
                }
                .padding(.top, 12)
            }
        }
        .padding(12)
        .background(.fill.quaternary, in: .rect(cornerRadius: 14))
    }

    private func detail(_ title: String, _ text: String) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(title)
                .font(.caption.weight(.semibold))
                .foregroundStyle(.secondary)
            ScrollView(.horizontal) {
                Text(text)
                    .font(.caption.monospaced())
                    .textSelection(.enabled)
            }
            .scrollIndicators(.hidden)
        }
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
