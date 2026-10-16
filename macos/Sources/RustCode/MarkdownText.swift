import RustCodeKit
import SwiftUI

/// Assistant text. Blocks come from `MarkdownBlock`; inline styling from
/// `AttributedString(markdown:)`. Consecutive prose blocks (paragraphs, headings,
/// list items, quotes) render as one selectable text, so a drag can select across
/// them; code blocks, tables and rules stand on their own.
struct MarkdownText: View {
    let text: String
    @Environment(\.findQuery) private var findQuery

    private enum Segment {
        case prose([MarkdownBlock])
        case block(MarkdownBlock)
    }

    private var segments: [Segment] {
        var segments: [Segment] = []
        for block in MarkdownBlock.parse(text) {
            switch block {
            case .paragraph, .heading, .listItem, .quote:
                if case .prose(let blocks) = segments.last {
                    segments[segments.count - 1] = .prose(blocks + [block])
                } else {
                    segments.append(.prose([block]))
                }
            default:
                segments.append(.block(block))
            }
        }
        return segments
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            ForEach(Array(segments.enumerated()), id: \.offset) { _, segment in
                switch segment {
                case .prose(let blocks):
                    Text(prose(blocks)).lineSpacing(2)
                case .block(let block):
                    view(for: block)
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .textSelection(.enabled)
    }

    /// One attributed string for a run of prose blocks. Blank lines between
    /// blocks are set small, so blocks sit about as far apart as the stack spacing.
    private func prose(_ blocks: [MarkdownBlock]) -> AttributedString {
        var result = AttributedString()
        var previous: MarkdownBlock?
        for block in blocks {
            if let previous {
                var gap = AttributedString("\n")
                if !(previous.isListItem && block.isListItem) {
                    var spacer = AttributedString("\n")
                    spacer.font = .system(size: 5)
                    gap += spacer
                }
                result += gap
            }
            switch block {
            case .paragraph(let text):
                result += highlighted(text)
            case .heading(let level, let text):
                var heading = highlighted(text)
                heading.font = level <= 1 ? .title2.bold() : level == 2 ? .title3.bold() : .headline
                result += heading
            case .listItem(let marker, let text):
                var bullet = AttributedString(marker + " ")
                bullet.foregroundColor = .secondary
                result += bullet + highlighted(text)
            case .quote(let text):
                var bar = AttributedString("▍ ")
                bar.foregroundColor = Color.secondary.opacity(0.5)
                var quote = highlighted(text)
                quote.foregroundColor = .secondary
                result += bar + quote
            default:
                break
            }
            previous = block
        }
        return result
    }

    @ViewBuilder
    private func view(for block: MarkdownBlock) -> some View {
        switch block {
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
        case .image(let alt, let url):
            InlineImage(url: url, alt: alt)
        default:
            EmptyView()
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

private extension MarkdownBlock {
    var isListItem: Bool {
        if case .listItem = self { return true }
        return false
    }
}
