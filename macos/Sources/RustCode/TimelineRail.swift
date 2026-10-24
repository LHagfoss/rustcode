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
    /// The chat's inset under the title bar. The rail spans the whole scroll
    /// view, while marker positions are from the top of its visible area.
    @Published var top: Double = 0
}

/// A thin timeline in the gutter left of the chat's column (#2309): a short
/// horizontal tick per message the user sent, stacked close together and
/// centred. The turn being read has the widest tick, in the brand colour;
/// the others narrow and fade with distance from it. Click a tick, or focus it with the
/// keyboard and press Space, to go to that turn; hover shows its first
/// words. Hidden when the gutter is too narrow for it.
struct TimelineRail: View {
    @ObservedObject var state: TimelineRailState
    /// With the sidebar hidden the gutter reaches the window's edge, and the
    /// rail sits against it rather than adrift in the middle of the gutter.
    var hugsEdge = false
    let jump: (Int) -> Void

    static let width: Double = 28
    /// From the window's edge when the rail hugs it.
    static let edgeInset: Double = 10
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
                    // Centred in the gutter beside the sidebar, at the
                    // window's edge without it.
                    .offset(x: hugsEdge ? min(Self.edgeInset, (gutter - Self.width) / 2) : (gutter - Self.width) / 2)
                    .transition(.opacity)
            }
        }
        .animation(reduceMotion ? nil : .easeOut(duration: 0.2), value: state.turnCount >= 2)
    }

    private func rail(height: Double) -> some View {
        ZStack(alignment: .top) {
            ForEach(state.layout.markers) { marker in
                TimelineTick(marker: marker, reduceMotion: reduceMotion) { jump(marker.id) }
                    // Centred on its place, below the title bar's inset.
                    .offset(y: state.top + marker.y - TimelineTick.hit / 2)
            }
        }
        .animation(reduceMotion ? nil : .easeOut(duration: 0.18), value: state.layout.current)
        .frame(width: Self.width, height: height, alignment: .top)
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Session timeline")
    }
}

private struct TimelineTick: View {
    let marker: TimelineMarker
    let reduceMotion: Bool
    let action: () -> Void

    /// The height of the strip the tick takes clicks in; ticks are
    /// `TimelineLayout.step` apart, so neighbours do not overlap.
    static let hit: Double = TimelineLayout.step
    /// The current turn's tick.
    static let longest: Double = 22
    @State private var isHovered = false

    var body: some View {
        Button(action: action) {
            Capsule()
                .fill(marker.isCurrent ? Color.brand : Color.primary)
                .frame(width: Self.longest * (isHovered ? max(marker.scale, 0.8) : marker.scale), height: 2)
                .opacity(marker.isCurrent ? 1 : isHovered ? 0.8 : marker.opacity * 0.55)
                .animation(reduceMotion ? nil : .easeOut(duration: 0.18), value: marker.scale)
                .animation(reduceMotion ? nil : .easeOut(duration: 0.12), value: isHovered)
                .frame(width: TimelineRail.width, height: Self.hit)
                .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .onHover { isHovered = $0 }
        .help(marker.title)
        .accessibilityLabel("Turn \(marker.number): \(marker.title)")
        .accessibilityAddTraits(marker.isCurrent ? .isSelected : [])
        .accessibilityHint("Scrolls the chat to this message")
    }
}
