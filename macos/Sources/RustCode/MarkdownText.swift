import RustCodeKit
import SwiftUI

/// Assistant text. Blocks come from `MarkdownBlock`; inline styling from
/// `AttributedString(markdown:)`.
struct MarkdownText: View {
    let text: String
    @Environment(\.findQuery) private var findQuery

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            ForEach(Array(MarkdownBlock.parse(text).enumerated()), id: \.offset) { _, block in
                view(for: block)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .textSelection(.enabled)
    }

    @ViewBuilder
    private func view(for block: MarkdownBlock) -> some View {
        switch block {
        case .paragraph(let text):
            Text(highlighted(text)).lineSpacing(2)
        case .heading(let level, let text):
            Text(highlighted(text))
                .font(level <= 1 ? .title2.bold() : level == 2 ? .title3.bold() : .headline)
                .padding(.top, 4)
                .accessibilityAddTraits(.isHeader)
        case .listItem(let marker, let text):
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(marker).foregroundStyle(.secondary).monospacedDigit()
                Text(highlighted(text)).lineSpacing(2)
            }
        case .quote(let text):
            Text(highlighted(text))
                .foregroundStyle(.secondary)
                .padding(.leading, 12)
                .overlay(alignment: .leading) { Capsule().fill(.quaternary).frame(width: 3) }
        case .code(let language, let text):
            CodeBlock(language: language, text: text)
        case .table(let headers, let rows):
            ScrollView(.horizontal) {
                Grid(alignment: .leading, horizontalSpacing: 16, verticalSpacing: 8) {
                    GridRow {
                        ForEach(Array(headers.enumerated()), id: \.offset) { _, cell in
                            Text(highlighted(cell)).fontWeight(.semibold)
                        }
                    }
                    Divider().gridCellUnsizedAxes(.horizontal)
                    ForEach(Array(rows.enumerated()), id: \.offset) { _, row in
                        GridRow {
                            ForEach(Array(row.enumerated()), id: \.offset) { _, cell in
                                Text(highlighted(cell))
                            }
                        }
                    }
                }
                .padding(12)
            }
            .background(.fill.quaternary, in: .rect(cornerRadius: 12))
        case .rule:
            Divider().padding(.vertical, 4)
        }
    }

    private func highlighted(_ text: String) -> AttributedString {
        FindHighlight.apply(Self.inline(text), query: findQuery)
    }

    static func inline(_ text: String) -> AttributedString {
        let options = AttributedString.MarkdownParsingOptions(interpretedSyntax: .inlineOnlyPreservingWhitespace)
        return (try? AttributedString(markdown: text, options: options)) ?? AttributedString(text)
    }
}

struct CodeBlock: View {
    var language: String?
    let text: String
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
                .foregroundStyle(.secondary)
                .help("Copy code")
            }
            .padding(.horizontal, 12)
            .padding(.top, 8)
            ScrollView(.horizontal) {
                Text(text)
                    .font(.system(.callout, design: .monospaced))
                    .textSelection(.enabled)
                    .padding(12)
            }
            .scrollIndicators(.never)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.fill.quaternary, in: .rect(cornerRadius: 12))
        .contextMenu { Button("Copy Code", systemImage: "doc.on.doc", action: copy) }
    }

    private func copy() {
        Clipboard.copy(text)
        showsCopied = true
        Task {
            try? await Task.sleep(for: .seconds(1.5))
            showsCopied = false
        }
    }
}

/// The find bar's text, so every message can mark where it matches.
extension EnvironmentValues {
    @Entry var findQuery = ""
}

enum FindHighlight {
    static func apply(_ text: AttributedString, query: String) -> AttributedString {
        let query = query.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !query.isEmpty else { return text }
        var text = text
        var searchStart = text.startIndex
        while searchStart < text.endIndex,
              let range = text[searchStart...].range(of: query, options: [.caseInsensitive, .diacriticInsensitive]) {
            text[range].backgroundColor = Color.attention.opacity(0.35)
            searchStart = range.upperBound
        }
        return text
    }
}
