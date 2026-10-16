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
        }
    }
}

private struct UserMessage: View {
    let text: String
    @Environment(\.findQuery) private var findQuery

    var body: some View {
        Text(FindHighlight.apply(AttributedString(text), query: findQuery))
            .textSelection(.enabled)
            .lineSpacing(2)
            .padding(.horizontal, 14)
            .padding(.vertical, 9)
            .background(.fill.tertiary, in: .rect(cornerRadius: 18))
            .contextMenu { Button("Copy", systemImage: "doc.on.doc") { Clipboard.copy(text) } }
            .frame(maxWidth: .infinity, alignment: .trailing)
            .padding(.leading, 80)
    }
}

/// Agent text, with `<think>` sections the engine leaves inline folded away.
private struct AssistantMessage: View {
    let text: String

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            ForEach(Array(AssistantBlock.parse(text).enumerated()), id: \.offset) { _, block in
                switch block {
                case .markdown(let text): MarkdownText(text: text)
                case .thinking(let text, _): ThinkingBlock(text: text)
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
