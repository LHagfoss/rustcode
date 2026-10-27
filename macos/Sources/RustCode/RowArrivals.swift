import RustCodeKit
import SwiftUI

/// Which chat rows arrived while the chat was open. A message sent, a reply
/// or a tool call that comes in animates into place; rows that were already
/// there when the chat opened, or that are drawn again later, do not.
@MainActor
final class RowArrivals {
    private var identity: UUID?
    /// Rows up to this id were there when the chat was first drawn.
    private var floor = Int.min
    private var seen: Set<Int> = []

    /// Whether row `id` is new since the chat with `identity` was first drawn
    /// with `last` as its last row. True once per row.
    func isNew(_ id: Int, in identity: UUID, last: Int?) -> Bool {
        if self.identity != identity {
            self.identity = identity
            floor = last ?? Int.min
            seen = []
        }
        guard id > floor else { return false }
        return seen.insert(id).inserted
    }
}

extension TranscriptItem {
    var isUser: Bool {
        if case .user = content { return true }
        return false
    }
}

extension View {
    /// Moves a new row into place: a sent message rises from the composer,
    /// anything else fades up a little. With Reduce Motion it fades only.
    func arrives(_ isNew: Bool, from edge: HorizontalEdge) -> some View {
        modifier(Arrival(isNew: isNew, edge: edge))
    }
}

private struct Arrival: ViewModifier {
    let edge: HorizontalEdge
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    /// Taken once, when the row is first drawn: whether it is new is only
    /// true on that first pass.
    @State private var settled: Bool

    init(isNew: Bool, edge: HorizontalEdge) {
        self.edge = edge
        _settled = State(initialValue: !isNew)
    }

    func body(content: Content) -> some View {
        let moves = !settled && !reduceMotion
        let rises = edge == .trailing
        content
            .opacity(settled ? 1 : 0)
            .scaleEffect(moves && rises ? 0.94 : 1, anchor: rises ? .bottomTrailing : .topLeading)
            .offset(y: moves ? (rises ? 28 : 8) : 0)
            .onAppear {
                guard !settled else { return }
                withAnimation(rises ? .spring(duration: 0.45, bounce: 0.25) : .easeOut(duration: 0.3)) {
                    settled = true
                }
            }
    }
}

extension Animation {
    /// Opening or folding a turn or a run of tool calls.
    static let rowReveal = Animation.snappy(duration: 0.3)
}

extension AnyTransition {
    /// The steps of a turn or the calls of a run as it opens: they fade in
    /// and drop a little into place, and fade out as it folds. Only a change
    /// made with an animation uses it, so rows streaming in do not.
    static var rowReveal: AnyTransition {
        .asymmetric(insertion: .opacity.combined(with: .offset(y: -6)), removal: .opacity)
    }
}
