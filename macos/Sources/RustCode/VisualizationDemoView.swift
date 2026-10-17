import SwiftUI

/// Standalone showcase for trying the native visualization widgets without
/// starting an engine session. Launch with `--visualization-demo`.
struct VisualizationDemoView: View {
    private let examples: [(title: String, subtitle: String, markdown: String)] = [
        (
            "Bar chart",
            "Hover a bar to inspect its exact value.",
            """
            **The engine crate dominates the test run.**

            ```rustcode-viz
            {"version":1,"type":"bar_chart","title":"Workspace test time","summary":"The engine crate accounts for most of the run","unit":"s","items":[{"label":"rustcode (engine)","value":41.8},{"label":"rustcode-tui","value":12.1},{"label":"rustcode-core","value":2.4},{"label":"rustcode-tools","value":7.6},{"label":"rustcode-tasks","value":3.2}]}
            ```
            """
        ),
        (
            "Sortable table",
            "Click a column heading to sort ascending or descending.",
            """
            Ranked findings:

            ```rustcode-viz
            {"version":1,"type":"table","title":"Slowest test suites","summary":"Sorted by duration; try sorting by suite","columns":["Suite","Tests","Duration (s)"],"rows":[["engine integration",128,41.8],["TUI transcript",642,12.1],["provider adapters",87,18.6],["filesystem tools",54,7.6],["remote gateway",39,3.2]],"sort_by":{"column":2,"descending":true}}
            ```
            """
        ),
        (
            "Collapsible tree",
            "Expand and collapse branches to explore the hierarchy.",
            """
            A simplified workspace structure:

            ```rustcode-viz
            {"version":1,"type":"tree","title":"RustCode workspace","summary":"Crates grouped by responsibility","nodes":[{"label":"rustcode","note":"workspace root","children":[{"label":"engine","note":"application and orchestration","children":[{"label":"providers","children":[{"label":"acp"},{"label":"codex"},{"label":"claude"}]},{"label":"tools"},{"label":"session"}]},{"label":"core","note":"shared domain crates","children":[{"label":"session"},{"label":"tool-protocol"},{"label":"tasks"}]},{"label":"frontends","children":[{"label":"tui"},{"label":"macOS app"}]},{"label":"docs"}]}]}
            ```
            """
        ),
    ]

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(alignment: .firstTextBaseline) {
                VStack(alignment: .leading, spacing: 4) {
                    Text("Native visualizations")
                        .font(.title2.weight(.semibold))
                    Text("Sample assistant replies rendered with RustCode’s macOS widgets")
                        .font(.callout)
                        .foregroundStyle(.secondary)
                }
                Spacer()
                Text("DEMO")
                    .font(.caption2.weight(.bold).monospaced())
                    .foregroundStyle(.secondary)
                    .padding(.horizontal, 8)
                    .padding(.vertical, 4)
                    .background(.fill.quaternary, in: .capsule)
            }
            .padding(.horizontal, 28)
            .padding(.top, 24)
            .padding(.bottom, 18)

            ScrollView {
                VStack(alignment: .leading, spacing: 22) {
                    ForEach(Array(examples.enumerated()), id: \.offset) { _, example in
                        VStack(alignment: .leading, spacing: 8) {
                            Text(example.title)
                                .font(.headline)
                            Text(example.subtitle)
                                .font(.callout)
                                .foregroundStyle(.secondary)
                            MarkdownText(text: example.markdown, visualizations: true)
                                .padding(14)
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .background(Color.chatBackground, in: .rect(cornerRadius: 14))
                                .overlay {
                                    RoundedRectangle(cornerRadius: 14)
                                        .strokeBorder(.quaternary, lineWidth: 1)
                                }
                        }
                    }
                }
                .frame(maxWidth: 780, alignment: .leading)
                .padding(.horizontal, 28)
                .padding(.bottom, 28)
                .frame(maxWidth: .infinity)
            }
        }
        .background(Color.chatBackground)
        .tint(.brand)
        .preferredColorScheme(.dark)
    }
}
