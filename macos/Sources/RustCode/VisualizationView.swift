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
            Label("Couldn't draw this visualization (\(reason)) — showing its source.", systemImage: "chart.bar.xaxis")
                .font(.caption)
                .foregroundStyle(.secondary)
            CodeBlock(language: Visualization.infoString, text: block.body)
        }
    }
}

private struct VisualizationCard: View {
    let visualization: Visualization
    let source: String

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Text(visualization.title).font(.headline)
                Spacer(minLength: 8)
                CopyButton(help: "Copy the visualization's source", action: copy)
                    .padding(.trailing, -6)
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
            case .lineChart(let labels, let series, let unit, let hiddenPoints, let hiddenSeries):
                LineChartWidget(labels: labels, series: series, unit: unit)
                truncation(hiddenPoints, "points", limit: Visualization.maxLinePoints)
                truncation(hiddenSeries, "series", limit: Visualization.maxLineSeries)
            case .statList(let items, let hidden):
                StatListWidget(items: items)
                truncation(hidden, "items", limit: Visualization.maxStatItems)
            case .checklist(let items, let done, let total, let hidden):
                ChecklistWidget(items: items, done: done, total: total)
                truncation(hidden, "items", limit: Visualization.maxChecklistItems)
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.fill.quaternary, in: .rect(cornerRadius: 12))
        .contextMenu {
            Button("Copy Source", systemImage: "doc.on.doc", action: copy)
            Button("Copy as Text", systemImage: "text.alignleft") { Clipboard.copy(visualization.plainText) }
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel(visualization.title)
        .accessibilityHint(visualization.accessibilitySummary)
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

/// Trends over a shared x axis, one line per series. Hovering shows a rule
/// at the nearest label and every series' value there.
private struct LineChartWidget: View {
    let labels: [String]
    let series: [Visualization.Series]
    let unit: String?
    @State private var hovered: Int?

    private struct Point: Identifiable {
        let series: String
        let index: Int
        let value: Double
        var id: String { "\(series)#\(index)" }
    }

    private var points: [Point] {
        series.flatMap { line in
            line.values.enumerated().compactMap { index, value in
                value.map { Point(series: line.name, index: index, value: $0) }
            }
        }
    }

    private func valueText(_ value: Double) -> String {
        let number = Visualization.formatNumber(value)
        return unit.map { "\(number) \($0)" } ?? number
    }

    private var hoverText: String {
        guard let hovered, labels.indices.contains(hovered) else { return "" }
        let values = series.compactMap { line in
            line.values[hovered].map { "\(line.name) \(valueText($0))" }
        }
        return "\(labels[hovered]): " + values.joined(separator: ", ")
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Chart {
                ForEach(points) { point in
                    LineMark(x: .value("Label", point.index), y: .value("Value", point.value),
                             series: .value("Series", point.series))
                        .foregroundStyle(by: .value("Series", point.series))
                        .interpolationMethod(.monotone)
                    PointMark(x: .value("Label", point.index), y: .value("Value", point.value))
                        .foregroundStyle(by: .value("Series", point.series))
                        .symbolSize(labels.count > 30 ? 0 : 24)
                        .accessibilityLabel("\(point.series), \(labels[point.index])")
                        .accessibilityValue(valueText(point.value))
                }
                if let hovered {
                    RuleMark(x: .value("Label", hovered))
                        .foregroundStyle(.secondary.opacity(0.5))
                }
            }
            .chartForegroundStyleScale(range: [Color.brand, .orange, .teal, .pink])
            .chartLegend(series.count > 1 ? .visible : .hidden)
            // Half a step of room at each end keeps the first and last labels.
            .chartXScale(domain: -0.5...(Double(max(labels.count - 1, 1)) + 0.5))
            .chartXAxis {
                AxisMarks(values: Array(stride(from: 0, to: labels.count, by: max(labels.count / 6, 1)))) { value in
                    AxisGridLine()
                    AxisValueLabel {
                        if let index = value.as(Int.self), labels.indices.contains(index) {
                            Text(labels[index]).lineLimit(1)
                        }
                    }
                }
            }
            .chartYAxis {
                AxisMarks { value in
                    AxisGridLine()
                    AxisValueLabel {
                        if let number = value.as(Double.self) { Text(valueText(number)) }
                    }
                }
            }
            .chartOverlay { proxy in
                GeometryReader { geometry in
                    Rectangle().fill(.clear).contentShape(Rectangle())
                        .onContinuousHover { phase in
                            switch phase {
                            case .active(let location):
                                guard let plot = proxy.plotFrame else { hovered = nil; return }
                                let x = location.x - geometry[plot].origin.x
                                let index = proxy.value(atX: x, as: Double.self).map { Int($0.rounded()) }
                                hovered = index.flatMap { labels.indices.contains($0) ? $0 : nil }
                            case .ended:
                                hovered = nil
                            }
                        }
                }
            }
            .frame(height: 180)
            Text(hoverText.isEmpty ? " " : hoverText)
                .font(.caption.monospacedDigit())
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .accessibilityHidden(hoverText.isEmpty)
        }
    }
}

/// Headline numbers as labeled tiles that wrap to the card's width.
private struct StatListWidget: View {
    let items: [Visualization.Stat]

    var body: some View {
        LazyVGrid(columns: [GridItem(.adaptive(minimum: 140), spacing: 8, alignment: .topLeading)],
                  alignment: .leading, spacing: 8) {
            ForEach(Array(items.enumerated()), id: \.offset) { _, item in
                VStack(alignment: .leading, spacing: 2) {
                    Text(item.label)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                    Text(item.value)
                        .font(.title3.weight(.semibold).monospacedDigit())
                        .lineLimit(1)
                        .minimumScaleFactor(0.7)
                    if let note = item.note {
                        Text(note).font(.caption).foregroundStyle(.secondary).lineLimit(2)
                    }
                }
                .padding(10)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(.fill.quaternary, in: .rect(cornerRadius: 8))
                .accessibilityElement(children: .combine)
            }
        }
        .textSelection(.enabled)
    }
}

/// Read-only progress: a bar for how many items are done, then each item
/// with its status.
private struct ChecklistWidget: View {
    let items: [Visualization.ChecklistItem]
    let done: Int
    let total: Int

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 8) {
                ProgressView(value: Double(done), total: Double(max(total, 1)))
                    .tint(Color.brand)
                Text("\(done) of \(total) done")
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(.secondary)
                    .fixedSize()
            }
            .accessibilityElement(children: .combine)
            ForEach(Array(items.enumerated()), id: \.offset) { _, item in
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Image(systemName: icon(item.status))
                        .foregroundStyle(color(item.status))
                        .frame(width: 16)
                        .accessibilityLabel(item.status.title)
                    Text(item.label)
                        .strikethrough(item.status == .skipped)
                        .foregroundStyle(item.status == .skipped ? .secondary : .primary)
                    if let note = item.note {
                        Text(note).foregroundStyle(.secondary)
                    }
                }
                .accessibilityElement(children: .combine)
            }
        }
        .font(.callout)
        .textSelection(.enabled)
    }

    private func icon(_ status: Visualization.ChecklistItem.Status) -> String {
        switch status {
        case .done: "checkmark.circle.fill"
        case .todo: "circle"
        case .inProgress: "circle.lefthalf.filled"
        case .failed: "xmark.circle.fill"
        case .skipped: "minus.circle"
        }
    }

    private func color(_ status: Visualization.ChecklistItem.Status) -> Color {
        switch status {
        case .done: .green
        case .todo, .skipped: .secondary
        case .inProgress: .brand
        case .failed: .red
        }
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
