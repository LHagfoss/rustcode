import RustCodeKit
import SwiftUI

/// The chat header's context meter: a short bar filling with the share of the
/// context window in use, and `34k / 128k` beside it, on the same filled
/// capsule as the toolbar clusters. The tooltip adds the session's tokens.
struct UsageCapsule: View {
    let usage: SessionUsage

    var body: some View {
        HStack(spacing: 6) {
            MeterBar(fraction: usage.fraction, tint: tint)
            Text(usage.label)
                .font(.caption.weight(.medium))
                .monospacedDigit()
                .foregroundStyle(.secondary)
                .contentTransition(.numericText())
        }
        .padding(.horizontal, 10)
        .frame(height: 28)
        .background(.fill.tertiary, in: .capsule)
        .fixedSize()
        .help(usage.detail)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Context \(usage.label), \(usage.percentLeft) percent left")
    }

    /// Brand while there is room; amber, then red, as the window fills up.
    private var tint: Color {
        switch usage.fraction {
        case ..<0.8: .brand
        case ..<0.95: .attention
        default: .red
        }
    }
}

/// A short level bar for a meter. A partial ring read as a spinner that never
/// stopped, and the composer's and header's rings, one counting what is used
/// and one what is left, looked alike.
struct MeterBar: View {
    let fraction: Double
    let tint: Color
    var width: CGFloat = 18

    var body: some View {
        ZStack(alignment: .leading) {
            Capsule().fill(.quaternary)
            Capsule()
                .fill(tint)
                // A sliver stays visible at a fresh start, so it reads as a level.
                .frame(width: max(width * min(max(fraction, 0), 1), 3))
        }
        .frame(width: width, height: 5)
        .animation(.snappy(duration: 0.3), value: fraction)
        .accessibilityHidden(true)
    }
}
