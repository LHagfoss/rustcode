import RustCodeKit
import SwiftUI

/// `/context` as a sheet: one bar split by what fills the window, a legend
/// with each category's tokens and share, then the configured reserves. The
/// same breakdown the terminal's context panel shows.
struct ContextSheet: View {
    let context: ContextBreakdown
    @Environment(\.dismiss) private var dismiss

    /// One colour per category, in the engine's legend order.
    static let palette: [Color] = [.blue, .purple, .orange, .teal, .indigo, .pink, .green]

    private func color(_ index: Int) -> Color { Self.palette[index % Self.palette.count] }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            header
            bar
            legend
            if let reserves = context.reserves {
                Divider()
                reservesView(reserves)
            }
            HStack {
                Text(context.estimated ? "Used is estimated from the saved history." : "Used is the provider's count of the latest request.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                Spacer()
                Button("Done") { dismiss() }
                    .buttonStyle(PillButtonStyle(prominent: true))
                    .keyboardShortcut(.defaultAction)
            }
        }
        .padding(20)
        .frame(width: 440)
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(alignment: .firstTextBaseline) {
                Text("Context").font(.headline)
                if let model = context.model {
                    Text(model).font(.callout).foregroundStyle(.secondary).lineLimit(1)
                }
            }
            Text("\(context.estimated ? "~" : "")\(SessionUsage.compact(context.used)) of \(SessionUsage.compact(context.window)) tokens · \(Int((context.share(context.used) * 100).rounded()))% used")
                .font(.title3.weight(.semibold))
                .monospacedDigit()
        }
    }

    private var bar: some View {
        GeometryReader { geometry in
            HStack(spacing: 1) {
                ForEach(Array(context.segments.enumerated()), id: \.offset) { _, segment in
                    let index = context.categories.firstIndex(of: segment.category) ?? 0
                    Rectangle()
                        .fill(color(index))
                        .frame(width: max(geometry.size.width * segment.share - 1, 1))
                        .help("\(segment.category.name): \(SessionUsage.compact(segment.category.tokens))")
                }
                Spacer(minLength: 0)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Color.primary.opacity(0.08))
        }
        .frame(height: 14)
        .clipShape(.capsule)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(Int((context.share(context.used) * 100).rounded())) percent of the context window used")
    }

    private var legend: some View {
        Grid(alignment: .leading, horizontalSpacing: 10, verticalSpacing: 6) {
            ForEach(Array(context.categories.enumerated()), id: \.offset) { index, category in
                row(color: color(index), name: category.name, tokens: category.tokens)
            }
            row(color: Color.primary.opacity(0.15), name: "Free", tokens: context.free)
        }
    }

    private func row(color: Color, name: String, tokens: Int) -> some View {
        GridRow {
            Circle().fill(color).frame(width: 8, height: 8)
            Text(name)
            Text(SessionUsage.compact(tokens))
                .monospacedDigit()
                .gridColumnAlignment(.trailing)
            Text(context.percent(tokens))
                .monospacedDigit()
                .foregroundStyle(.secondary)
                .gridColumnAlignment(.trailing)
        }
        .font(.callout)
    }

    private func reservesView(_ reserves: ContextBreakdown.Reserves) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("Configured reserves").font(.subheadline.weight(.medium))
            Text("Output \(SessionUsage.compact(reserves.output)) incl. thinking \(SessionUsage.compact(reserves.thinking)) · tools \(SessionUsage.compact(reserves.tools)) · safety \(SessionUsage.compact(reserves.safety)) · provider overhead \(SessionUsage.compact(reserves.providerOverhead))")
                .font(.caption)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            Text("Reserves are limits kept out of the window, not usage.")
                .font(.caption)
                .foregroundStyle(.tertiary)
        }
    }
}
