import RustCodeKit
import SwiftUI

/// The chat header's context meter: a thin ring filling with the share of the
/// context window in use, and `34k / 128k` beside it, on the same Liquid Glass
/// capsule as the toolbar clusters. The tooltip adds the session's tokens.
struct UsageCapsule: View {
    let usage: SessionUsage

    var body: some View {
        HStack(spacing: 6) {
            ZStack {
                Circle().stroke(.quaternary, lineWidth: 2)
                Circle()
                    .trim(from: 0, to: usage.fraction)
                    .stroke(tint, style: StrokeStyle(lineWidth: 2, lineCap: .round))
                    .rotationEffect(.degrees(-90))
            }
            .frame(width: 12, height: 12)
            .animation(.snappy(duration: 0.3), value: usage.fraction)
            Text(usage.label)
                .font(.caption.weight(.medium))
                .monospacedDigit()
                .foregroundStyle(.secondary)
                .contentTransition(.numericText())
        }
        .padding(.horizontal, 10)
        .frame(height: 28)
        .glassSurface(in: .capsule)
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
