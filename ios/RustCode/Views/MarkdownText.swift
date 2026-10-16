import RustCodeRemote
import SwiftUI

/// Assistant text. Blocks come from `MarkdownBlock`; inline styling from
/// `AttributedString(markdown:)`. Tables scroll horizontally on narrow screens.
struct MarkdownText: View {
    let text: String

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            ForEach(Array(MarkdownBlock.parse(text).enumerated()), id: \.offset) { _, block in
                view(for: block)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    @ViewBuilder
    private func view(for block: MarkdownBlock) -> some View {
        switch block {
        case .paragraph(let text):
            Text(Self.inline(text))
        case .heading(let level, let text):
            Text(Self.inline(text))
                .font(level <= 1 ? .title2.bold() : level == 2 ? .title3.bold() : .headline)
                .padding(.top, 4)
                .accessibilityAddTraits(.isHeader)
        case .listItem(let marker, let text):
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(marker)
                    .foregroundStyle(.secondary)
                    .monospacedDigit()
                Text(Self.inline(text))
            }
        case .quote(let text):
            Text(Self.inline(text))
                .foregroundStyle(.secondary)
                .padding(.leading, 12)
                .overlay(alignment: .leading) {
                    Capsule().fill(.quaternary).frame(width: 3)
                }
        case .code(let language, let text):
            CodeBlock(language: language, text: text)
        case .table(let headers, let rows):
            ScrollView(.horizontal) {
                Grid(alignment: .leading, horizontalSpacing: 16, verticalSpacing: 10) {
                    GridRow {
                        ForEach(Array(headers.enumerated()), id: \.offset) { _, cell in
                            Text(Self.inline(cell)).fontWeight(.semibold)
                        }
                    }
                    Divider().gridCellUnsizedAxes(.horizontal)
                    ForEach(Array(rows.enumerated()), id: \.offset) { _, row in
                        GridRow {
                            ForEach(Array(row.enumerated()), id: \.offset) { _, cell in
                                Text(Self.inline(cell))
                            }
                        }
                    }
                }
                .padding(12)
            }
            .background(.fill.quaternary, in: .rect(cornerRadius: 12))
            .accessibilityIdentifier("markdownTable")
        case .rule:
            Divider().padding(.vertical, 4)
        }
    }

    /// The text as it reads on screen, without the Markdown markers.
    static func plainText(_ text: String) -> String {
        var result = ""
        var previousWasListItem = false
        for block in MarkdownBlock.parse(text) {
            var isListItem = false
            let line: String
            switch block {
            case .paragraph(let text), .heading(_, let text), .quote(let text):
                line = String(inline(text).characters)
            case .listItem(let marker, let text):
                line = "\(marker) \(String(inline(text).characters))"
                isListItem = true
            case .code(_, let text):
                line = text
            case .table(let headers, let rows):
                line = ([headers] + rows).map { row in
                    row.map { String(inline($0).characters) }.joined(separator: "\t")
                }.joined(separator: "\n")
            case .rule:
                line = "—"
            }
            if !result.isEmpty {
                result += isListItem && previousWasListItem ? "\n" : "\n\n"
            }
            result += line
            previousWasListItem = isListItem
        }
        return result
    }

    static func inline(_ text: String) -> AttributedString {
        let options = AttributedString.MarkdownParsingOptions(interpretedSyntax: .inlineOnlyPreservingWhitespace)
        return (try? AttributedString(markdown: text, options: options)) ?? AttributedString(text)
    }
}

struct CodeBlock: View {
    var language: String?
    let text: String
    @State private var copyCount = 0
    @State private var showsCopied = false

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text(language ?? "code")
                    .font(.caption.monospaced())
                    .foregroundStyle(.secondary)
                Spacer(minLength: 8)
                Button(action: copy) {
                    Label(showsCopied ? "Copied" : "Copy", systemImage: showsCopied ? "checkmark" : "doc.on.doc")
                        .contentTransition(.symbolEffect(.replace))
                }
                .font(.caption.weight(.medium))
                .buttonStyle(.borderless)
                .tint(.secondary)
                .accessibilityLabel("Copy code")
            }
            .padding(.horizontal, 12)
            .padding(.top, 10)
            ScrollView(.horizontal) {
                Text(text)
                    .font(.callout.monospaced())
                    .padding(12)
            }
            .scrollIndicators(.hidden)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.fill.quaternary, in: .rect(cornerRadius: 12))
        .contentShape(.contextMenuPreview, .rect(cornerRadius: 12))
        .contextMenu {
            Button("Copy Code", systemImage: "doc.on.doc", action: copy)
        }
        .sensoryFeedback(.success, trigger: copyCount)
    }

    private func copy() {
        Clipboard.copy(text)
        copyCount += 1
        showsCopied = true
        Task {
            try? await Task.sleep(for: .seconds(1.5))
            showsCopied = false
        }
    }
}
