import RustCodeKit
import SwiftUI

/// The plan's usage windows beside the composer's pills (#2545): one thin
/// bar per window, filled with what is left, amber and then red as it runs
/// low. Hovering asks for fresh numbers and shows them with reset times.
struct PlanUsageMeter: View {
    let usage: PlanUsage
    let refresh: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            ForEach(usage.windows) { window in
                Capsule()
                    .fill(.quaternary)
                    .frame(width: 30, height: 3)
                    .overlay(alignment: .leading) {
                        Capsule()
                            .fill(PlanUsageMeter.tint(window))
                            .frame(width: 30 * window.leftFraction, height: 3)
                    }
            }
        }
        .padding(.horizontal, 8)
        .frame(height: 30)
        .contentShape(.rect)
        .help(usage.summary())
        .onHover { if $0 { refresh() } }
        .animation(.snappy(duration: 0.3), value: usage)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(usage.windows.map { "\($0.title) \($0.leftPercent) percent left" }.joined(separator: ", "))
    }

    /// Brand while there is room; amber under a fifth, red under a twentieth.
    static func tint(_ window: PlanUsage.Window) -> Color {
        switch window.leftPercent {
        case ..<5: .red
        case ..<20: .orange
        default: .brand
        }
    }
}

/// The same windows written out, for the context panel.
struct PlanUsageRows: View {
    let usage: PlanUsage

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(usage.source == "codex" ? "Plan usage (from the Codex CLI)" : "Plan usage")
                .font(.subheadline.weight(.semibold))
            ForEach(usage.windows) { window in
                VStack(alignment: .leading, spacing: 4) {
                    HStack {
                        Text(window.title).font(.callout)
                        Spacer()
                        Text(window.detail()).font(.callout).monospacedDigit().foregroundStyle(.secondary)
                    }
                    ProgressView(value: window.leftFraction)
                        .tint(PlanUsageMeter.tint(window))
                }
            }
        }
    }
}
