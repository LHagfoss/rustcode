import RustCodeKit
import SwiftUI

/// Standalone showcase for trying the native visualization widgets without
/// starting an engine session. Launch with `--visualization-demo`. It shows
/// the same samples as the in-app guide (`/visualize`).
struct VisualizationDemoView: View {
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
                    ForEach(VisualizationGuide.entries) { entry in
                        VStack(alignment: .leading, spacing: 8) {
                            Text(entry.name)
                                .font(.headline)
                            Text(entry.interaction)
                                .font(.callout)
                                .foregroundStyle(.secondary)
                            MarkdownText(text: entry.sampleMarkdown, visualizations: true)
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
