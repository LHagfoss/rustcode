import Charts
import RustCodeKit
import SwiftUI

/// A `rustcode-viz` block in an assistant reply: a native widget once the
/// fence is complete and valid, a placeholder while it streams, and the
/// source with a short note otherwise. Nothing in the payload is executed.
struct VisualizationView: View {
    let block: VisualizationBlock
    /// The reply is still arriving, so an open fence may yet close.
    var isStreaming = false

    var body: some View {
        switch block.state {
        case .rendered(let visualization):
            VisualizationCard(visualization: visualization, source: block.source)
        case .invalid(let reason):
            fallback(reason)
        case .incomplete:
            if isStreaming {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text("Building visualization…").foregroundStyle(.secondary)
                }
                .font(.callout)
                .padding(.vertical, 6)
                .accessibilityElement(children: .combine)
            } else {
                fallback("incomplete block")
            }
        }
    }

    private func fallback(_ reason: String) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Label("Visualization not rendered (\(reason)); showing source", systemImage: "chart.bar.xaxis")
                .font(.caption)
                .foregroundStyle(.secondary)
            CodeBlock(language: Visualization.infoString, text: block.body)
        }
    }
}

private struct VisualizationCard: View {
    let visualization: Visualization
    let source: String
    @State private var showsCopied = false

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .firstTextBaseline) {
                Text(visualization.title).font(.headline)
                Spacer(minLength: 8)
                Button(action: copy) {
                    Label(showsCopied ? "Copied" : "Copy", systemImage: showsCopied ? "checkmark" : "doc.on.doc")
                        .contentTransition(.symbolEffect(.replace))
                }
                .font(.caption.weight(.medium))
                .buttonStyle(.borderless)
                .foregroundStyle(.secondary)
                .help("Copy the visualization's source")
            }
            if let summary = visualization.summary {
                Text(summary).font(.callout).foregroundStyle(.secondary)
            }
            switch visualization.content {
            case .barChart(let bars, let unit, let hidden):
                BarChartWidget(bars: bars, unit: unit)
                truncation(hidden, "items", limit: Visualization.maxBarItems)
            case .table(let columns, let rows, let numeric, let sortColumn, let descending, let hiddenColumns):
                TableWidget(columns: columns, rows: rows, numeric: numeric,
                            sortColumn: sortColumn, descending: descending)
                truncation(rows.count - Visualization.maxTableRows, "rows", limit: Visualization.maxTableRows)
                truncation(hiddenColumns, "columns", limit: Visualization.maxTableColumns)
            case .tree(let nodes, let hidden):
                TreeWidget(nodes: nodes)
                truncation(hidden, "nodes", limit: Visualization.maxTreeNodes)
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.fill.quaternary, in: .rect(cornerRadius: 12))
        .contextMenu { Button("Copy Source", systemImage: "doc.on.doc", action: copy) }
        .accessibilityElement(children: .contain)
        .accessibilityLabel(visualization.title)
    }

    @ViewBuilder
    private func truncation(_ hidden: Int, _ noun: String, limit: Int) -> some View {
        if hidden > 0 {
            Text("… \(hidden) more \(noun) not shown (limit \(limit))")
                .font(.caption)
                .italic()
                .foregroundStyle(.secondary)
        }
    }

    private func copy() {
        Clipboard.copy(source)
        showsCopied = true
        Task {
            try? await Task.sleep(for: .seconds(1.5))
            showsCopied = false
        }
    }
}

/// Horizontal bars. Hovering a bar highlights it and shows its exact value.
private struct BarChartWidget: View {
    let bars: [Visualization.Bar]
    let unit: String?
    @State private var hovered: Int?

    private func valueText(_ value: Double) -> String {
        let number = Visualization.formatNumber(value)
        return unit.map { "\(number) \($0)" } ?? number
    }

    var body: some View {
        Chart(Array(bars.enumerated()), id: \.offset) { index, bar in
            BarMark(x: .value("Value", bar.value), y: .value("Label", "\(index)"))
                .foregroundStyle(bar.value < 0 ? Color.red.opacity(0.8) : Color.brand)
                .opacity(hovered == nil || hovered == index ? 1 : 0.45)
                .annotation(position: bar.value < 0 ? .leading : .trailing, spacing: 4) {
                    Text(valueText(bar.value))
                        .font(.caption.monospacedDigit())
                        .foregroundStyle(hovered == index ? .primary : .secondary)
                        .fontWeight(hovered == index ? .semibold : .regular)
                }
                .accessibilityLabel(bar.label)
                .accessibilityValue(valueText(bar.value))
        }
        .chartYAxis {
            AxisMarks { value in
                AxisValueLabel {
                    if let key = value.as(String.self), let index = Int(key), bars.indices.contains(index) {
                        Text(bars[index].label).lineLimit(1)
                    }
                }
            }
        }
        .chartXAxis(.hidden)
        .chartOverlay { proxy in
            GeometryReader { geometry in
                Rectangle().fill(.clear).contentShape(Rectangle())
                    .onContinuousHover { phase in
                        switch phase {
                        case .active(let location):
                            guard let plot = proxy.plotFrame else { hovered = nil; return }
                            let y = location.y - geometry[plot].origin.y
                            hovered = proxy.value(atY: y, as: String.self).flatMap(Int.init)
                        case .ended:
                            hovered = nil
                        }
                    }
            }
        }
        .frame(height: CGFloat(bars.count) * 26 + 8)
        .help(hovered.map { "\(bars[$0].label): \(valueText(bars[$0].value))" } ?? "")
    }
}

/// A table whose headers sort it: click once for ascending, again for
/// descending. Numeric columns sort by value and align right.
private struct TableWidget: View {
    let columns: [String]
    let rows: [[Visualization.Cell]]
    let numeric: [Bool]
    @State private var sortColumn: Int?
    @State private var descending: Bool

    init(columns: [String], rows: [[Visualization.Cell]], numeric: [Bool], sortColumn: Int?, descending: Bool) {
        self.columns = columns
        self.rows = rows
        self.numeric = numeric
        _sortColumn = State(initialValue: sortColumn)
        _descending = State(initialValue: descending)
    }

    private var shownRows: [[Visualization.Cell]] {
        Array(Visualization.Content.sortedRows(rows, column: sortColumn, descending: descending)
            .prefix(Visualization.maxTableRows))
    }

    var body: some View {
        WrappingTable(columns: columns.count, minimumWidth: 100) {
            ForEach(Array(columns.enumerated()), id: \.offset) { index, header in
                Button { toggle(index) } label: {
                    HStack(spacing: 4) {
                        Text(header).fontWeight(.semibold)
                        if sortColumn == index {
                            Image(systemName: descending ? "chevron.down" : "chevron.up")
                                .font(.caption2.weight(.bold))
                                .foregroundStyle(.secondary)
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: numeric[index] ? .trailing : .leading)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .help("Sort by \(header)")
                .vizCell(isHeader: true)
            }
            ForEach(Array(shownRows.enumerated()), id: \.offset) { _, row in
                ForEach(Array(row.enumerated()), id: \.offset) { index, cell in
                    Text(cell.text)
                        .font(numeric[index] ? .body.monospacedDigit() : .body)
                        .frame(maxWidth: .infinity, alignment: numeric[index] ? .trailing : .leading)
                        .vizCell(isHeader: false)
                }
            }
        }
        .textSelection(.enabled)
    }

    private func toggle(_ column: Int) {
        if sortColumn == column {
            descending.toggle()
        } else {
            sortColumn = column
            descending = false
        }
    }
}

/// A hierarchy with collapsible branches; the first two levels start open.
private struct TreeWidget: View {
    let nodes: [Visualization.TreeNode]
    @State private var collapsed: Set<Int>

    init(nodes: [Visualization.TreeNode]) {
        self.nodes = nodes
        var collapsed = Set<Int>()
        func walk(_ nodes: [Visualization.TreeNode], depth: Int) {
            for node in nodes {
                if depth >= 1, !node.children.isEmpty { collapsed.insert(node.id) }
                walk(node.children, depth: depth + 1)
            }
        }
        walk(nodes, depth: 0)
        _collapsed = State(initialValue: collapsed)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            ForEach(visibleRows, id: \.node.id) { row in
                HStack(spacing: 4) {
                    if row.node.children.isEmpty {
                        Image(systemName: "circle.fill").font(.system(size: 4))
                            .foregroundStyle(.tertiary)
                            .frame(width: 14)
                    } else {
                        Button { toggle(row.node.id) } label: {
                            Image(systemName: "chevron.right")
                                .font(.caption.weight(.semibold))
                                .rotationEffect(.degrees(collapsed.contains(row.node.id) ? 0 : 90))
                                .frame(width: 14, height: 14)
                                .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .foregroundStyle(.secondary)
                        .help(collapsed.contains(row.node.id) ? "Expand" : "Collapse")
                        .accessibilityLabel(collapsed.contains(row.node.id) ? "Expand \(row.node.label)" : "Collapse \(row.node.label)")
                    }
                    Text(row.node.label)
                        .fontWeight(row.node.children.isEmpty ? .regular : .semibold)
                    if let note = row.node.note {
                        Text(note).foregroundStyle(.secondary)
                    }
                }
                .padding(.leading, CGFloat(row.depth) * 16)
            }
        }
        .font(.callout)
        .textSelection(.enabled)
        .animation(.snappy(duration: 0.15), value: collapsed)
    }

    private struct Row {
        let node: Visualization.TreeNode
        let depth: Int
    }

    private var visibleRows: [Row] {
        var rows: [Row] = []
        func walk(_ nodes: [Visualization.TreeNode], depth: Int) {
            for node in nodes {
                rows.append(Row(node: node, depth: depth))
                if !collapsed.contains(node.id) { walk(node.children, depth: depth + 1) }
            }
        }
        walk(nodes, depth: 0)
        return rows
    }

    private func toggle(_ id: Int) {
        if collapsed.contains(id) { collapsed.remove(id) } else { collapsed.insert(id) }
    }
}

private extension View {
    func vizCell(isHeader: Bool) -> some View {
        fixedSize(horizontal: false, vertical: true)
            .padding(.horizontal, 8)
            .padding(.vertical, 5)
            .overlay(alignment: .bottom) {
                if isHeader { Rectangle().fill(.separator).frame(height: 1) }
            }
    }
}
