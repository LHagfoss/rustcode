import RustCodeKit
import SwiftUI

/// Assistant text. Blocks come from `MarkdownBlock`; inline styling from
/// `AttributedString(markdown:)`. Consecutive prose blocks (paragraphs, headings,
/// list items, quotes) render as one selectable text, so a drag can select across
/// them; code blocks, tables and rules stand on their own.
struct MarkdownText: View {
    let text: String
    /// Draw `rustcode-viz` blocks as widgets. Only assistant replies set it.
    var visualizations = false
    /// The reply is still streaming, so an unclosed visualization shows a placeholder.
    var isStreaming = false
    @Environment(\.findQuery) private var findQuery

    private enum Segment {
        case prose([MarkdownBlock])
        case block(MarkdownBlock)
    }

    private var segments: [Segment] {
        var segments: [Segment] = []
        for block in MarkdownBlock.parse(text, visualizations: visualizations) {
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
            // Columns share the reply's width and long cells wrap, so a table
            // never scrolls sideways.
            WrappingTable(columns: headers.count) {
                ForEach(Array(headers.enumerated()), id: \.offset) { _, cell in
                    Text(highlighted(cell)).fontWeight(.semibold)
                        .tableCell(isHeader: true)
                }
                ForEach(Array(rows.enumerated()), id: \.offset) { _, row in
                    ForEach(Array(row.enumerated()), id: \.offset) { _, cell in
                        Text(highlighted(cell)).tableCell(isHeader: false)
                    }
                }
            }
            .padding(.vertical, 6)
            .background(.fill.quaternary, in: .rect(cornerRadius: 12))
        case .rule:
            Divider().padding(.vertical, 4)
        case .image(let alt, let url):
            InlineImage(url: url, alt: alt)
        case .visualization(let visualization):
            VisualizationView(block: visualization, isStreaming: isStreaming)
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

private extension View {
    func tableCell(isHeader: Bool) -> some View {
        fixedSize(horizontal: false, vertical: true)
            .frame(maxWidth: .infinity, alignment: .topLeading)
            .padding(.horizontal, 10)
            .padding(.vertical, 6)
            .overlay(alignment: .bottom) {
                if isHeader { Rectangle().fill(.separator).frame(height: 1) }
            }
    }
}

/// Lays cells out row by row in `columns` columns. Columns get the width their
/// longest cell needs when the table fits (the last one takes what is left); otherwise every column keeps up to
/// `minimumWidth` and the rest of the width goes to the columns that need more,
/// in proportion, and their cells wrap.
struct WrappingTable: Layout {
    let columns: Int
    var minimumWidth: CGFloat = 160

    private func columnWidths(_ cells: Subviews, available: CGFloat?) -> [CGFloat] {
        guard columns > 0 else { return [] }
        var ideal = Array(repeating: CGFloat(0), count: columns)
        for (index, cell) in cells.enumerated() {
            ideal[index % columns] = max(ideal[index % columns], ceil(cell.sizeThatFits(.unspecified).width))
        }
        let total = ideal.reduce(0, +)
        guard let available else { return ideal }
        // A narrow table spans the reply anyway; the last column takes the rest.
        if total <= available {
            ideal[columns - 1] += available - total
            return ideal
        }
        var floor = ideal.map { min($0, minimumWidth) }
        // Too many columns even for that: share the width by need.
        let floorTotal = floor.reduce(0, +)
        if floorTotal > available { floor = floor.map { $0 * available / floorTotal } }
        let spare = max(available - floor.reduce(0, +), 0)
        let wanted = zip(ideal, floor).map { $0 - $1 }
        let wantedTotal = max(wanted.reduce(0, +), 1)
        return zip(floor, wanted).map { $0 + spare * $1 / wantedTotal }
    }

    private func rowHeights(_ cells: Subviews, widths: [CGFloat]) -> [CGFloat] {
        guard columns > 0 else { return [] }
        var heights = Array(repeating: CGFloat(0), count: (cells.count + columns - 1) / columns)
        for (index, cell) in cells.enumerated() {
            let size = cell.sizeThatFits(ProposedViewSize(width: widths[index % columns], height: nil))
            heights[index / columns] = max(heights[index / columns], ceil(size.height))
        }
        return heights
    }

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let widths = columnWidths(subviews, available: proposal.width)
        let width = proposal.width ?? widths.reduce(0, +)
        return CGSize(width: width, height: rowHeights(subviews, widths: widths).reduce(0, +))
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        let widths = columnWidths(subviews, available: bounds.width)
        let heights = rowHeights(subviews, widths: widths)
        var y = bounds.minY
        for row in heights.indices {
            var x = bounds.minX
            for column in 0..<columns {
                let index = row * columns + column
                guard index < subviews.count else { break }
                subviews[index].place(at: CGPoint(x: x, y: y), anchor: .topLeading,
                                      proposal: ProposedViewSize(width: widths[column], height: heights[row]))
                x += widths[column]
            }
            y += heights[row]
        }
    }
}
