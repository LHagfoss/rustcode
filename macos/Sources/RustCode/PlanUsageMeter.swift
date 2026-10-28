import RustCodeKit
import SwiftUI

/// The plan's usage beside the composer's pills (#2545): a small ring of
/// what is left in the tightest window, amber and then red as it runs low.
/// Hovering asks for fresh numbers and floats a glass panel with each window,
/// its percentage and when it resets.
struct PlanUsageMeter: View {
    let usage: PlanUsage
    let refresh: () -> Void
    @State private var hovering = false

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
        .overlay(alignment: .bottom) {
            if hovering {
                PlanUsageRows(usage: usage, compact: true)
                    .frame(width: 250)
                    .padding(12)
                    // Glass alone let the chat's text show through; the
                    // material under it keeps the numbers readable.
                    .floatingSurface(in: .rect(cornerRadius: 14))
                    .glassSurface(in: .rect(cornerRadius: 14))
                    .fixedSize()
                    .offset(y: -48)
                    .allowsHitTesting(false)
                    .transition(.opacity.combined(with: .scale(scale: 0.96, anchor: .bottom)))
            }
        }
        .zIndex(hovering ? 1 : 0)
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
