import RustCodeKit
import SwiftUI

/// The plan's usage beside the composer's pills (#2545): a small ring of
/// what is left in the tightest window, amber and then red as it runs low.
/// Hovering asks for fresh numbers and shows `PlanUsagePanel` above it.
struct PlanUsageMeter: View {
    let usage: PlanUsage
    @Binding var hovering: Bool
    let refresh: () -> Void

    /// The window closest to running out decides the ring.
    private var tightest: PlanUsage.Window? { usage.windows.min { $0.leftPercent < $1.leftPercent } }

    var body: some View {
        ZStack {
            Circle().stroke(.quaternary, lineWidth: 2)
            if let tightest {
                Circle()
                    .trim(from: 0, to: tightest.leftFraction)
                    .stroke(PlanUsageMeter.tint(tightest), style: StrokeStyle(lineWidth: 2, lineCap: .round))
                    .rotationEffect(.degrees(-90))
            }
        }
        .frame(width: 14, height: 14)
        .padding(.horizontal, 8)
        .frame(height: 30)
        .contentShape(.rect)
        .animation(.snappy(duration: 0.3), value: usage)
        .onHover { inside in
            if inside { refresh() }
            withAnimation(.snappy(duration: 0.15)) { hovering = inside }
        }
        // The chat draws the panel at this spot, outside the composer.
        .anchorPreference(key: PlanUsageAnchor.self, value: .bounds) { $0 }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Plan usage")
        .accessibilityValue(usage.windows.map { "\($0.title) \($0.leftPercent) percent left" }.joined(separator: ", "))
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

/// Where the usage meter is, for the chat to float its panel over it.
struct PlanUsageAnchor: PreferenceKey {
    static let defaultValue: Anchor<CGRect>? = nil
    static func reduce(value: inout Anchor<CGRect>?, nextValue: () -> Anchor<CGRect>?) {
        value = value ?? nextValue()
    }
}

/// The meter's hover panel: each window, its percentage and when it resets,
/// on the same glass as the composer's model and effort panels, its bottom
/// just above the meter and kept inside the chat's edges (#2551). The chat
/// draws it over itself: inside the composer, the glass had only the
/// composer's own glass to sample and showed as a see-through sheet.
struct PlanUsagePanel: View {
    let usage: PlanUsage
    let meter: CGRect
    let bounds: CGSize

    static let width: CGFloat = 274
    private static let margin: CGFloat = 12

    var body: some View {
        let leading = Self.leading(meterMidX: meter.midX, in: bounds.width)
        PlanUsageRows(usage: usage, compact: true)
            .padding(14)
            .frame(width: Self.width, alignment: .leading)
            .glassSurface(in: .rect(cornerRadius: 18))
            .padding(.leading, leading)
            .frame(width: bounds.width, height: max(meter.minY - 10, 0), alignment: .bottomLeading)
            .allowsHitTesting(false)
    }

    /// The panel's leading edge: centred over the meter, but never past the
    /// chat's edges, so it does not clip in a narrow window.
    static func leading(meterMidX: CGFloat, in width: CGFloat) -> CGFloat {
        min(max(meterMidX - Self.width / 2, margin), max(width - Self.width - margin, margin))
    }
}

/// The windows written out, for the context panel and the hover panel.
struct PlanUsageRows: View {
    let usage: PlanUsage
    /// The hover panel's smaller title.
    var compact = false

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(usage.source == "codex" ? "Plan usage (from the Codex CLI)" : "Plan usage")
                .font(compact ? .caption.weight(.semibold) : .subheadline.weight(.semibold))
                .foregroundStyle(compact ? .secondary : .primary)
            ForEach(usage.windows) { window in
                VStack(alignment: .leading, spacing: 4) {
                    HStack {
                        Text(window.title).font(.callout)
                        Spacer()
                        Text(window.detail()).font(.callout).monospacedDigit().foregroundStyle(.secondary)
                    }
                    // What is left, in the ring's colour; `ProgressView`
                    // ignored the tint and drew grey.
                    GeometryReader { proxy in
                        Capsule().fill(.quaternary)
                            .overlay(alignment: .leading) {
                                Capsule().fill(PlanUsageMeter.tint(window))
                                    .frame(width: proxy.size.width * window.leftFraction)
                            }
                    }
                    .frame(height: 5)
                }
            }
        }
    }
}
