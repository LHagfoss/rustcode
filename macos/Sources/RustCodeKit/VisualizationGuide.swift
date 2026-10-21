import Foundation

/// What the in-app visualization guide shows (`/visualize`, Help menu, the +
/// menu and the empty chat): each widget the app draws, how to interact with
/// it, a prompt the user can insert to ask for one, and a sample block that
/// renders with the real widget code. Tests check every sample renders.
public enum VisualizationGuide {
    public struct Entry: Identifiable, Sendable, Equatable {
        /// The `type` in the payload.
        public let type: String
        public let name: String
        /// An SF Symbol for the guide row.
        public let systemImage: String
        /// What it is good for.
        public let purpose: String
        /// How to use the rendered widget.
        public let interaction: String
        /// A request to insert into the composer.
        public let prompt: String
        /// A `rustcode-viz` payload that shows the widget.
        public let sample: String

        public var id: String { type }

        /// The sample as an assistant reply's fence.
        public var sampleMarkdown: String { "```\(Visualization.infoString)\n\(sample)\n```" }
    }

    /// Shown above the entries.
    public static let overview = """
        Ask in plain words: name the data and the shape you want, such as \
        "as a chart", "as a table" or "as a tree". The agent replies with a \
        small data block the app draws natively; nothing in it runs as code.
        """

    /// Shown below the entries.
    public static let tips = [
        "Right-click a widget to copy its source block or a plain-text version.",
        "A block that fails to validate shows its source with the reason, so nothing is lost.",
        "Limits keep widgets small: 30 bars, 50 table rows, 100 line points, 120 tree nodes.",
    ]

    public static let entries: [Entry] = [
        Entry(
            type: "bar_chart", name: "Bar chart", systemImage: "chart.bar.xaxis",
            purpose: "Compare sizes: build or test times, file sizes, counts per module.",
            interaction: "Hover a bar to see its exact value.",
            prompt: "Which tests in this workspace are slowest? Show them as a bar chart.",
            sample: #"{"version":1,"type":"bar_chart","title":"Test time by crate","summary":"The engine crate takes most of the run","unit":"s","items":[{"label":"engine","value":41.8},{"label":"tui","value":12.1},{"label":"tools","value":7.6},{"label":"core","value":2.4}]}"#
        ),
        Entry(
            type: "line_chart", name: "Line chart", systemImage: "chart.xyaxis.line",
            purpose: "Show a trend: benchmark or size history across commits, releases or runs.",
            interaction: "Hover the chart to read every series at that point.",
            prompt: "How has the release binary size changed over the last few tags? Show it as a line chart.",
            sample: #"{"version":1,"type":"line_chart","title":"Release binary size","summary":"Size dropped after enabling LTO in v0.84","unit":"MB","labels":["v0.80","v0.81","v0.82","v0.83","v0.84","v0.85"],"series":[{"name":"macOS","values":[21.4,21.9,22.6,23.1,18.2,18.4]},{"name":"Linux","values":[19.8,20.1,20.9,21.3,16.9,17.0]}]}"#
        ),
        Entry(
            type: "table", name: "Sortable table", systemImage: "tablecells",
            purpose: "Rank or list results with several columns.",
            interaction: "Click a column heading to sort; click again to reverse.",
            prompt: "List the largest source files in this project as a sortable table with their line counts.",
            sample: #"{"version":1,"type":"table","title":"Largest source files","columns":["File","Lines","Tests"],"rows":[["engine/src/acp.rs",1420,true],["tui/src/ui/tests.rs",12900,true],["engine/src/tools/schema.rs",2100,false]],"sort_by":{"column":1,"descending":true}}"#
        ),
        Entry(
            type: "stat_list", name: "Stat list", systemImage: "square.grid.2x2",
            purpose: "Headline numbers at a glance: coverage, counts, sizes, durations.",
            interaction: "Each tile shows a value with an optional note; select text to copy it.",
            prompt: "Summarize this repository's key numbers (crates, source files, lines of code, tests) as a stat list.",
            sample: #"{"version":1,"type":"stat_list","title":"Workspace at a glance","items":[{"label":"Crates","value":9},{"label":"Rust files","value":412},{"label":"Lines of code","value":"186k"},{"label":"Tests","value":3104,"note":"all passing"}]}"#
        ),
        Entry(
            type: "checklist", name: "Checklist", systemImage: "checklist",
            purpose: "Track progress: migration steps, CI checks, review items.",
            interaction: "Read-only; the bar shows how many items are done.",
            prompt: "Make a checklist of what is left to finish this change, marking what is already done.",
            sample: #"{"version":1,"type":"checklist","title":"Release checklist","items":[{"label":"Bump versions","status":"done"},{"label":"Run the test suite","status":"done","note":"3104 passed"},{"label":"Update the changelog","status":"in_progress"},{"label":"Tag and publish","status":"todo"}]}"#
        ),
        Entry(
            type: "tree", name: "Collapsible tree", systemImage: "list.bullet.indent",
            purpose: "Show a hierarchy: module layout, call trees, dependencies.",
            interaction: "Click a chevron to expand or collapse a branch.",
            prompt: "Show the module layout of this project as a collapsible tree.",
            sample: #"{"version":1,"type":"tree","title":"Workspace layout","nodes":[{"label":"rustcode","children":[{"label":"engine","note":"agent loop and tools","children":[{"label":"providers"},{"label":"tools"}]},{"label":"tui","note":"terminal UI"}]},{"label":"macos","note":"SwiftUI app"}]}"#
        ),
    ]
}
