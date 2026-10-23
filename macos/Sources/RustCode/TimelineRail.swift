import RustCodeKit
import SwiftUI

/// The session timeline's markers, from where the chat is (`TimelineLayout`).
/// Only the rail observes it: the chat publishes into it as it scrolls,
/// which redraws the rail and nothing else (#2260).
@MainActor
final class TimelineRailState: ObservableObject {
    @Published var layout = TimelineLayout.empty
    /// The turns there are; the rail hides with fewer than two.
    @Published var turnCount = 0
}

/// A thin timeline in the gutter left of the chat's column (#2309): a dot
/// per message the user sent, level with it while in view, the rest stacked
/// at the ends. The turn being read is in the brand colour; the others
/// shrink and fade with distance from it. Click a dot, or focus it with the
/// keyboard and press Space, to go to that turn; hover shows its first
/// words. Hidden when the gutter is too narrow for it.
struct TimelineRail: View {
    @ObservedObject var state: TimelineRailState
    let jump: (Int) -> Void

    static let width: Double = 22
    /// The chat's column and the padding either side of it (`ChatView`).
    static let column: Double = 760
    static let padding: Double = 28

    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        GeometryReader { geometry in
            let width = geometry.size.width
            if TimelineLayout.shows(width: width, column: Self.column, padding: Self.padding, turns: state.turnCount) {
                let gutter = TimelineLayout.gutter(width: width, column: Self.column, padding: Self.padding)
                rail(height: geometry.size.height)
                    // Centred in the gutter.
                    .offset(x: (gutter - Self.width) / 2)
                    .transition(.opacity)
            }
        }
        .animation(reduceMotion ? nil : .easeOut(duration: 0.2), value: state.turnCount >= 2)
    }

    private func rail(height: Double) -> some View {
        let markers = state.layout.markers
        return ZStack(alignment: .top) {
            if let first = markers.first, let last = markers.last, last.y > first.y {
                // The line between the outermost markers, fading at its ends.
                Rectangle()
                    .fill(LinearGradient(stops: [
                        .init(color: .primary.opacity(0), location: 0),
                        .init(color: .primary.opacity(0.14), location: 0.15),
                        .init(color: .primary.opacity(0.14), location: 0.85),
                        .init(color: .primary.opacity(0), location: 1),
                    ], startPoint: .top, endPoint: .bottom))
                    .frame(width: 1, height: last.y - first.y)
                    .offset(y: first.y)
                    .allowsHitTesting(false)
            }
            ForEach(markers) { marker in
                TimelineDot(marker: marker, reduceMotion: reduceMotion) { jump(marker.id) }
                    // Centred on its place; not animated, so it keeps up with the scroll.
                    .offset(y: marker.y - TimelineDot.hit / 2)
            }
        }
        .frame(width: Self.width, height: height, alignment: .top)
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Session timeline")
    }
}

private struct TimelineDot: View {
    let marker: TimelineMarker
    let reduceMotion: Bool
    let action: () -> Void

    /// The square the dot takes clicks in.
    static let hit: Double = 14
    @State private var isHovered = false

    var body: some View {
        Button(action: action) {
            Circle()
                .fill(marker.isCurrent ? Color.brand : Color.primary)
                .frame(width: 7, height: 7)
                .scaleEffect(marker.isCurrent ? 1.3 : isHovered ? max(marker.scale, 0.9) : marker.scale)
                .opacity(marker.isCurrent ? 1 : isHovered ? 0.8 : marker.opacity * 0.55)
                .animation(reduceMotion ? nil : .easeOut(duration: 0.18), value: marker.isCurrent)
                .animation(reduceMotion ? nil : .easeOut(duration: 0.12), value: isHovered)
                .frame(width: Self.hit, height: Self.hit)
                .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .frame(width: TimelineRail.width)
        .onHover { isHovered = $0 }
        .help(marker.title)
        .accessibilityLabel("Turn \(marker.number): \(marker.title)")
        .accessibilityAddTraits(marker.isCurrent ? .isSelected : [])
        .accessibilityHint("Scrolls the chat to this message")
    }
}
