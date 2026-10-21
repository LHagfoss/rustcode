import Foundation

/// An agent-authored `rustcode-viz` block, validated against the version 1
/// schema in `docs/visualizations.md`. The limits match the TUI renderer so a
/// block looks the same, and falls back for the same reasons, in both.
public struct Visualization: Sendable, Equatable {
    public static let infoString = "rustcode-viz"
    /// The payload `version` every block carries. New widget types are
    /// additive, so it stays 1; a renderer that predates a type falls back to
    /// the source with "unsupported type".
    public static let version = 1
    /// The `_meta.rustcode.visualizations` level the app advertises: 1 is
    /// `bar_chart`, `table` and `tree`; 2 adds `line_chart`, `stat_list` and
    /// `checklist`. The engine teaches the agent only the types a level covers.
    public static let capabilityLevel = 2
    /// Every type this renderer draws, in the order the guide lists them.
    public static let supportedTypes = ["bar_chart", "line_chart", "table", "stat_list", "checklist", "tree"]

    public static let maxPayloadBytes = 16 * 1024
    public static let maxTitleCharacters = 80
    public static let maxSummaryCharacters = 300
    public static let maxLabelCharacters = 40
    public static let maxCellCharacters = 60
    public static let maxUnitCharacters = 12
    public static let maxBarItems = 30
    public static let maxTableRows = 50
    public static let maxTableColumns = 8
    public static let maxTreeNodes = 120
    public static let maxTreeDepth = 8
    public static let maxLinePoints = 100
    public static let maxLineSeries = 4
    public static let maxStatItems = 12
    public static let maxChecklistItems = 50

    public struct Bar: Sendable, Equatable {
        public let label: String
        public let value: Double
    }

    public struct Cell: Sendable, Equatable {
        public let text: String
        /// Set for numeric cells, which sort numerically and align right.
        public let number: Double?
    }

    public struct TreeNode: Sendable, Equatable, Identifiable {
        /// Pre-order position, unique within the tree.
        public let id: Int
        public let label: String
        public let note: String?
        public let children: [TreeNode]
    }

    /// One line of a `line_chart`; `nil` values are gaps.
    public struct Series: Sendable, Equatable {
        public let name: String
        public let values: [Double?]
    }

    /// One tile of a `stat_list`. `number` is set for numeric values.
    public struct Stat: Sendable, Equatable {
        public let label: String
        public let value: String
        public let number: Double?
        public let note: String?
    }

    public struct ChecklistItem: Sendable, Equatable {
        public enum Status: String, Sendable, CaseIterable {
            case done, todo, inProgress = "in_progress", failed, skipped
        }

        public let label: String
        public let status: Status
        public let note: String?
    }

    public enum Content: Sendable, Equatable {
        /// `hiddenItems` are items past the limit.
        case barChart(bars: [Bar], unit: String?, hiddenItems: Int)
        /// Every row is kept so sorting covers the whole table; the view
        /// shows at most `maxTableRows`. `hiddenColumns` are past the limit.
        case table(columns: [String], rows: [[Cell]], numericColumns: [Bool],
                   sortColumn: Int?, sortDescending: Bool, hiddenColumns: Int)
        /// `hiddenNodes` were past the node or depth limit.
        case tree(nodes: [TreeNode], hiddenNodes: Int)
        /// Capability level 2. `labels` is the shared x axis; every series
        /// has one value (or gap) per label. `hiddenPoints` and
        /// `hiddenSeries` were past the limits.
        case lineChart(labels: [String], series: [Series], unit: String?, hiddenPoints: Int, hiddenSeries: Int)
        /// Capability level 2: a few headline numbers as labeled tiles.
        case statList(items: [Stat], hiddenItems: Int)
        /// Capability level 2: read-only progress. `doneCount` and
        /// `totalCount` cover every item, shown or not.
        case checklist(items: [ChecklistItem], doneCount: Int, totalCount: Int, hiddenItems: Int)
    }

    public let type: String
    public let title: String
    public let summary: String?
    public let content: Content

    public enum ParseError: Error, Equatable, CustomStringConvertible {
        case empty
        case tooLarge(bytes: Int)
        case invalidJSON
        case notAnObject
        case missingVersion
        case unsupportedVersion(Int)
        case missingType
        case unsupportedType(String)
        case invalid(String)

        /// Short reason for the fallback note; matches the TUI's wording.
        public var description: String {
            switch self {
            case .empty: "empty block"
            case .tooLarge(let bytes):
                "payload is \((bytes + 1023) / 1024) KiB, limit is \(Visualization.maxPayloadBytes / 1024) KiB"
            case .invalidJSON: "invalid JSON"
            case .notAnObject: "payload is not a JSON object"
            case .missingVersion: "missing \"version\""
            case .unsupportedVersion(let version): "unsupported version \(version)"
            case .missingType: "missing \"type\""
            case .unsupportedType(let type): "unsupported type \"\(type)\""
            case .invalid(let detail): detail
            }
        }
    }

    public static func isInfoString(_ info: String?) -> Bool {
        info?.trimmingCharacters(in: .whitespaces) == infoString
    }

    public static func parse(_ source: String) throws -> Visualization {
        guard !source.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { throw ParseError.empty }
        let bytes = source.utf8.count
        guard bytes <= maxPayloadBytes else { throw ParseError.tooLarge(bytes: bytes) }
        guard let value = try? JSONDecoder().decode(JSONValue.self, from: Data(source.utf8)) else {
            throw ParseError.invalidJSON
        }
        guard let object = value.objectValue else { throw ParseError.notAnObject }
        guard let version = object["version"]?.intValue, version >= 0 else { throw ParseError.missingVersion }
        guard version == Self.version else { throw ParseError.unsupportedVersion(version) }
        guard let type = object["type"]?.stringValue else { throw ParseError.missingType }
        let title = try optionalText(object, "title")
        let summary = try optionalText(object, "summary")

        let content: Content
        switch type {
        case "bar_chart": content = try barChart(object)
        case "table": content = try table(object)
        case "tree": content = try tree(object)
        case "line_chart": content = try lineChart(object)
        case "stat_list": content = try statList(object)
        case "checklist": content = try checklist(object)
        default: throw ParseError.unsupportedType(truncate(type, maxLabelCharacters))
        }
        return Visualization(
            type: type,
            title: truncate(title ?? type.replacingOccurrences(of: "_", with: " "), maxTitleCharacters),
            summary: summary.map { truncate($0, maxSummaryCharacters) },
            content: content
        )
    }

    // MARK: - Widgets

    private static func barChart(_ object: [String: JSONValue]) throws -> Content {
        guard let items = object["items"]?.arrayValue else {
            throw ParseError.invalid("bar_chart needs an \"items\" array")
        }
        guard !items.isEmpty else { throw ParseError.invalid("bar_chart has no items") }
        let unit = try optionalText(object, "unit").map { truncate($0, maxUnitCharacters) }
        let bars = try items.prefix(maxBarItems).map { item -> Bar in
            guard let label = item["label"]?.stringValue else {
                throw ParseError.invalid("every bar_chart item needs a string \"label\"")
            }
            guard let value = item["value"]?.doubleValue, value.isFinite else {
                throw ParseError.invalid("every bar_chart item needs a finite numeric \"value\"")
            }
            return Bar(label: truncate(singleLine(label), maxLabelCharacters), value: value)
        }
        return .barChart(bars: bars, unit: unit, hiddenItems: max(items.count - maxBarItems, 0))
    }

    private static func table(_ object: [String: JSONValue]) throws -> Content {
        guard let columns = object["columns"]?.arrayValue else {
            throw ParseError.invalid("table needs a \"columns\" array")
        }
        guard !columns.isEmpty else { throw ParseError.invalid("table has no columns") }
        guard let rows = object["rows"]?.arrayValue else { throw ParseError.invalid("table needs a \"rows\" array") }
        let shown = min(columns.count, maxTableColumns)
        let headers = try columns.prefix(shown).map { column -> String in
            guard let text = column.stringValue else { throw ParseError.invalid("table columns must be strings") }
            return truncate(singleLine(text), maxLabelCharacters)
        }
        let cells = try rows.map { row -> [Cell] in
            guard let values = row.arrayValue else { throw ParseError.invalid("table rows must be arrays") }
            return try (0..<shown).map { index in try cell(index < values.count ? values[index] : .null) }
        }

        var sortColumn: Int?
        var sortDescending = false
        // `sort_by` is only a hint: a column that is unknown or not shown keeps
        // the payload's order instead of failing the whole table.
        if let sort = object["sort_by"],
           let column = sort["column"].flatMap({ resolveSortColumn($0, shown: Array(columns.prefix(shown))) }) {
            sortColumn = column
            sortDescending = sort["descending"]?.boolValue ?? false
        }
        let numeric = (0..<shown).map { column in
            !cells.isEmpty && cells.allSatisfy { $0[column].number != nil || $0[column].text.isEmpty }
        }
        return .table(columns: headers, rows: cells, numericColumns: numeric,
                      sortColumn: sortColumn, sortDescending: sortDescending,
                      hiddenColumns: columns.count - shown)
    }

    /// Resolve a table's `sort_by.column` to a shown column: an index, a
    /// numeric string, or a column name (trimmed, case-insensitive).
    static func resolveSortColumn(_ column: JSONValue, shown: [JSONValue]) -> Int? {
        let index: Int?
        if let number = column.intValue {
            index = number
        } else if let text = column.stringValue?.trimmingCharacters(in: .whitespacesAndNewlines) {
            index = Int(text) ?? shown.firstIndex {
                $0.stringValue?.trimmingCharacters(in: .whitespacesAndNewlines).lowercased() == text.lowercased()
            }
        } else {
            index = nil
        }
        return index.flatMap { shown.indices.contains($0) ? $0 : nil }
    }

    private static func cell(_ value: JSONValue) throws -> Cell {
        switch value {
        case .null: return Cell(text: "", number: nil)
        case .bool(let flag): return Cell(text: flag ? "true" : "false", number: nil)
        case .number(let number): return Cell(text: formatNumber(number), number: number)
        case .string(let text): return Cell(text: truncate(singleLine(text), maxCellCharacters), number: nil)
        default: throw ParseError.invalid("table cells must be strings, numbers, booleans, or null")
        }
    }

    private static func tree(_ object: [String: JSONValue]) throws -> Content {
        guard let nodes = object["nodes"]?.arrayValue else { throw ParseError.invalid("tree needs a \"nodes\" array") }
        guard !nodes.isEmpty else { throw ParseError.invalid("tree has no nodes") }
        try validateTree(nodes, depth: 0)
        var shown = 0
        var hidden = 0
        func build(_ nodes: [JSONValue], depth: Int) -> [TreeNode] {
            var result: [TreeNode] = []
            for (index, node) in nodes.enumerated() {
                guard shown < maxTreeNodes else {
                    hidden += countNodes(Array(nodes[index...]))
                    break
                }
                let id = shown
                shown += 1
                let children = node["children"]?.arrayValue ?? []
                var built: [TreeNode] = []
                if !children.isEmpty {
                    if depth + 1 > maxTreeDepth {
                        hidden += countNodes(children)
                    } else {
                        built = build(children, depth: depth + 1)
                    }
                }
                let note = node["note"]?.stringValue.map { truncate(singleLine($0), maxCellCharacters) }
                result.append(TreeNode(
                    id: id,
                    label: truncate(singleLine(node["label"]?.stringValue ?? ""), maxCellCharacters),
                    note: note?.isEmpty == false ? note : nil,
                    children: built
                ))
            }
            return result
        }
        let built = build(nodes, depth: 0)
        return .tree(nodes: built, hiddenNodes: hidden)
    }

    private static func validateTree(_ nodes: [JSONValue], depth: Int) throws {
        guard depth <= maxTreeDepth * 4 else { throw ParseError.invalid("tree is nested too deeply") }
        for node in nodes {
            guard node["label"]?.stringValue != nil else {
                throw ParseError.invalid("every tree node needs a string \"label\"")
            }
            switch node["note"] {
            case nil, .null?, .string?: break
            default: throw ParseError.invalid("tree \"note\" must be a string")
            }
            switch node["children"] {
            case nil, .null?: break
            case .array(let children)?: try validateTree(children, depth: depth + 1)
            default: throw ParseError.invalid("tree \"children\" must be an array")
            }
        }
    }

    private static func lineChart(_ object: [String: JSONValue]) throws -> Content {
        guard let labels = object["labels"]?.arrayValue else {
            throw ParseError.invalid("line_chart needs a \"labels\" array")
        }
        guard !labels.isEmpty else { throw ParseError.invalid("line_chart has no labels") }
        guard let series = object["series"]?.arrayValue else {
            throw ParseError.invalid("line_chart needs a \"series\" array")
        }
        guard !series.isEmpty else { throw ParseError.invalid("line_chart has no series") }
        let unit = try optionalText(object, "unit").map { truncate($0, maxUnitCharacters) }
        let points = min(labels.count, maxLinePoints)
        let axis = try labels.prefix(points).map { label -> String in
            switch label {
            case .string(let text): return truncate(singleLine(text), maxLabelCharacters)
            case .number(let number): return formatNumber(number)
            default: throw ParseError.invalid("line_chart labels must be strings or numbers")
            }
        }
        let lines = try series.prefix(maxLineSeries).enumerated().map { index, entry -> Series in
            guard entry.objectValue != nil else { throw ParseError.invalid("line_chart series must be objects") }
            let name: String
            switch entry["name"] {
            case nil, .null?: name = "series \(index + 1)"
            case .string(let text)?: name = truncate(singleLine(text), maxLabelCharacters)
            default: throw ParseError.invalid("line_chart series \"name\" must be a string")
            }
            guard let values = entry["values"]?.arrayValue else {
                throw ParseError.invalid("every line_chart series needs a \"values\" array")
            }
            let shown = try (0..<points).map { point -> Double? in
                guard point < values.count else { return nil }
                switch values[point] {
                case .null: return nil
                case .number(let number) where number.isFinite: return number
                default: throw ParseError.invalid("line_chart values must be finite numbers or null")
                }
            }
            return Series(name: name.isEmpty ? "series \(index + 1)" : name, values: shown)
        }
        guard lines.contains(where: { $0.values.contains { $0 != nil } }) else {
            throw ParseError.invalid("line_chart has no values")
        }
        return .lineChart(labels: axis, series: lines, unit: unit,
                          hiddenPoints: labels.count - points, hiddenSeries: max(series.count - maxLineSeries, 0))
    }

    private static func statList(_ object: [String: JSONValue]) throws -> Content {
        guard let items = object["items"]?.arrayValue else {
            throw ParseError.invalid("stat_list needs an \"items\" array")
        }
        guard !items.isEmpty else { throw ParseError.invalid("stat_list has no items") }
        let stats = try items.prefix(maxStatItems).map { item -> Stat in
            guard let label = item["label"]?.stringValue else {
                throw ParseError.invalid("every stat_list item needs a string \"label\"")
            }
            let value: String
            var number: Double?
            switch item["value"] {
            case .string(let text)?: value = truncate(singleLine(text), maxLabelCharacters)
            case .number(let figure)? where figure.isFinite:
                value = formatNumber(figure)
                number = figure
            default: throw ParseError.invalid("every stat_list item needs a string or finite numeric \"value\"")
            }
            let unit = try optionalText(item.objectValue ?? [:], "unit").map { truncate($0, maxUnitCharacters) }
            let note = try optionalText(item.objectValue ?? [:], "note").map { truncate($0, maxCellCharacters) }
            return Stat(label: truncate(singleLine(label), maxLabelCharacters),
                        value: unit.map { "\(value) \($0)" } ?? value, number: number, note: note)
        }
        return .statList(items: stats, hiddenItems: max(items.count - maxStatItems, 0))
    }

    private static func checklist(_ object: [String: JSONValue]) throws -> Content {
        guard let items = object["items"]?.arrayValue else {
            throw ParseError.invalid("checklist needs an \"items\" array")
        }
        guard !items.isEmpty else { throw ParseError.invalid("checklist has no items") }
        let all = try items.map { item -> ChecklistItem in
            guard let label = item["label"]?.stringValue else {
                throw ParseError.invalid("every checklist item needs a string \"label\"")
            }
            let status: ChecklistItem.Status
            switch item["status"] {
            case nil, .null?: status = .todo
            case .string(let raw)? where ChecklistItem.Status(rawValue: raw) != nil:
                status = ChecklistItem.Status(rawValue: raw)!
            default:
                throw ParseError.invalid("checklist \"status\" must be done, todo, in_progress, failed, or skipped")
            }
            let note = try optionalText(item.objectValue ?? [:], "note").map { truncate($0, maxCellCharacters) }
            return ChecklistItem(label: truncate(singleLine(label), maxCellCharacters), status: status, note: note)
        }
        return .checklist(items: Array(all.prefix(maxChecklistItems)),
                          doneCount: all.filter { $0.status == .done }.count, totalCount: all.count,
                          hiddenItems: max(all.count - maxChecklistItems, 0))
    }

    private static func countNodes(_ nodes: [JSONValue]) -> Int {
        nodes.reduce(0) { $0 + 1 + countNodes($1["children"]?.arrayValue ?? []) }
    }

    // MARK: - Text

    private static func optionalText(_ object: [String: JSONValue], _ key: String) throws -> String? {
        switch object[key] {
        case nil, .null?: return nil
        case .string(let text)?:
            let line = singleLine(text)
            return line.isEmpty ? nil : line
        default: throw ParseError.invalid("\"\(key)\" must be a string")
        }
    }

    /// Control characters and newlines become single spaces, so text cannot
    /// break the layout.
    static func singleLine(_ text: String) -> String {
        var result = ""
        for scalar in text.unicodeScalars {
            if CharacterSet.controlCharacters.contains(scalar) || CharacterSet.newlines.contains(scalar) {
                if !result.hasSuffix(" ") { result.append(" ") }
            } else {
                result.unicodeScalars.append(scalar)
            }
        }
        return result.trimmingCharacters(in: .whitespaces)
    }

    static func truncate(_ text: String, _ limit: Int) -> String {
        guard text.count > limit else { return text }
        return String(text.prefix(limit - 1)) + "…"
    }

    public static func formatNumber(_ value: Double) -> String {
        if value.rounded() == value, abs(value) < 1e15 { return String(Int64(value)) }
        var text = String(format: "%.2f", value)
        while text.hasSuffix("0") { text.removeLast() }
        if text.hasSuffix(".") { text.removeLast() }
        return text
    }
}

extension Visualization.Content {
    /// Rows of a table in display order for a column sort.
    public static func sortedRows(_ rows: [[Visualization.Cell]], column: Int?, descending: Bool) -> [[Visualization.Cell]] {
        guard let column else { return rows }
        let sorted = rows.enumerated().sorted { left, right in
            let a = left.element[column], b = right.element[column]
            let ordering: ComparisonResult
            if let x = a.number, let y = b.number {
                ordering = x < y ? .orderedAscending : x > y ? .orderedDescending : .orderedSame
            } else {
                ordering = a.text.compare(b.text)
            }
            // Ties keep the source order in either direction.
            if ordering == .orderedSame { return left.offset < right.offset }
            return descending ? ordering == .orderedDescending : ordering == .orderedAscending
        }
        return sorted.map(\.element)
    }
}

/// A `rustcode-viz` fence found in assistant Markdown.
public struct VisualizationBlock: Sendable, Equatable {
    public enum State: Sendable, Equatable {
        case rendered(Visualization)
        /// Shown as source with this reason.
        case invalid(String)
        /// The closing fence has not arrived: a placeholder while streaming,
        /// the source once the reply is final.
        case incomplete
    }

    /// The whole fence as written, which copying reproduces.
    public let source: String
    /// The JSON between the fences.
    public let body: String
    public let state: State

    init(opening: String, lines: [String], closing: String?) {
        body = lines.joined(separator: "\n")
        source = ([opening] + lines + (closing.map { [$0] } ?? [])).joined(separator: "\n")
        if closing == nil {
            state = .incomplete
        } else {
            do {
                state = .rendered(try Visualization.parse(body))
            } catch let error as Visualization.ParseError {
                state = .invalid(error.description)
            } catch {
                state = .invalid("invalid JSON")
            }
        }
    }
}
