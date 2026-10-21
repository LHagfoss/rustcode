import Foundation

/// Textual forms of a visualization: the plain-text export behind "Copy as
/// Text", and the one-sentence summary VoiceOver reads for the card. Both are
/// built from the validated, bounded model, never from the raw payload.
extension Visualization {
    /// The widget as readable text: Markdown tables for tabular data, an
    /// indented list for trees, and `label: value` lines otherwise.
    public var plainText: String {
        var lines = [title]
        if let summary { lines.append(summary) }
        lines.append("")
        switch content {
        case .barChart(let bars, let unit, let hidden):
            lines += bars.map { "\($0.label): \(Self.withUnit($0.value, unit))" }
            lines += Self.more(hidden, "items")
        case .table(let columns, let rows, _, let sortColumn, let descending, let hiddenColumns):
            let sorted = Content.sortedRows(rows, column: sortColumn, descending: descending)
            lines += Self.markdownTable(columns, sorted.map { $0.map(\.text) })
            lines += Self.more(hiddenColumns, "columns")
        case .tree(let nodes, let hidden):
            func walk(_ nodes: [TreeNode], depth: Int) {
                for node in nodes {
                    let note = node.note.map { " — \($0)" } ?? ""
                    lines.append(String(repeating: "  ", count: depth) + "- \(node.label)\(note)")
                    walk(node.children, depth: depth + 1)
                }
            }
            walk(nodes, depth: 0)
            lines += Self.more(hidden, "nodes")
        case .lineChart(let labels, let series, let unit, let hiddenPoints, let hiddenSeries):
            let header = [unit.map { "Label (\($0))" } ?? "Label"] + series.map(\.name)
            let rows = labels.enumerated().map { index, label in
                [label] + series.map { $0.values[index].map(Visualization.formatNumber) ?? "" }
            }
            lines += Self.markdownTable(header, rows)
            lines += Self.more(hiddenPoints, "points") + Self.more(hiddenSeries, "series")
        case .statList(let items, let hidden):
            lines += items.map { item in
                "\(item.label): \(item.value)" + (item.note.map { " (\($0))" } ?? "")
            }
            lines += Self.more(hidden, "items")
        case .checklist(let items, let done, let total, let hidden):
            lines.append("\(done) of \(total) done")
            lines += items.map { item in
                "\(item.status.marker) \(item.label)" + (item.note.map { " — \($0)" } ?? "")
            }
            lines += Self.more(hidden, "items")
        }
        return lines.joined(separator: "\n")
    }

    /// One sentence describing the widget's data, for VoiceOver.
    public var accessibilitySummary: String {
        switch content {
        case .barChart(let bars, let unit, let hidden):
            let top = bars.max { abs($0.value) < abs($1.value) }
            let largest = top.map { " Largest: \($0.label), \(Self.withUnit($0.value, unit))." } ?? ""
            return "Bar chart, \(Self.count(bars.count + hidden, "item")).\(largest)"
        case .table(let columns, let rows, _, _, _, let hiddenColumns):
            return "Table, \(Self.count(rows.count, "row")) and \(Self.count(columns.count + hiddenColumns, "column"))."
        case .tree(let nodes, let hidden):
            func count(_ nodes: [TreeNode]) -> Int { nodes.reduce(0) { $0 + 1 + count($1.children) } }
            return "Tree, \(Self.count(count(nodes) + hidden, "node"))."
        case .lineChart(let labels, let series, let unit, let hiddenPoints, _):
            let range = labels.count > 1 ? " from \(labels[0]) to \(labels[labels.count - 1])" : ""
            let lines = series.map { line -> String in
                let values = line.values.compactMap { $0 }
                guard let first = values.first, let last = values.last,
                      let low = values.min(), let high = values.max() else { return "\(line.name): no values" }
                return "\(line.name): \(Self.withUnit(first, unit)) to \(Self.withUnit(last, unit)), "
                    + "low \(Self.withUnit(low, unit)), high \(Self.withUnit(high, unit))"
            }
            return "Line chart, \(Self.count(labels.count + hiddenPoints, "point"))\(range). "
                + lines.joined(separator: "; ") + "."
        case .statList(let items, _):
            return items.map { "\($0.label): \($0.value)" }.joined(separator: ", ") + "."
        case .checklist(let items, let done, let total, _):
            let failed = items.filter { $0.status == .failed }.count
            return "Checklist, \(done) of \(total) done" + (failed > 0 ? ", \(failed) failed." : ".")
        }
    }

    private static func withUnit(_ value: Double, _ unit: String?) -> String {
        let number = formatNumber(value)
        return unit.map { "\(number) \($0)" } ?? number
    }

    private static func count(_ value: Int, _ noun: String) -> String {
        "\(value) \(noun)\(value == 1 ? "" : "s")"
    }

    private static func more(_ hidden: Int, _ noun: String) -> [String] {
        hidden > 0 ? ["… \(hidden) more \(noun) not shown"] : []
    }

    private static func markdownTable(_ header: [String], _ rows: [[String]]) -> [String] {
        func row(_ cells: [String]) -> String {
            "| " + cells.map { $0.replacingOccurrences(of: "|", with: "\\|") }.joined(separator: " | ") + " |"
        }
        return [row(header), row(header.map { _ in "---" })] + rows.map(row)
    }
}

extension Visualization.ChecklistItem.Status {
    /// The Markdown-style marker used in the plain-text export.
    public var marker: String {
        switch self {
        case .done: "[x]"
        case .todo: "[ ]"
        case .inProgress: "[~]"
        case .failed: "[!]"
        case .skipped: "[-]"
        }
    }

    public var title: String {
        switch self {
        case .done: "Done"
        case .todo: "To do"
        case .inProgress: "In progress"
        case .failed: "Failed"
        case .skipped: "Skipped"
        }
    }
}
