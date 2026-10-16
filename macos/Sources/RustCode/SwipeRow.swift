import AppKit
import SwiftUI

/// One button revealed by swiping a row.
struct SwipeAction: Identifiable {
    let id = UUID()
    var title: String
    var systemImage: String
    var tint: Color
    var action: () -> Void
}

/// A row that a two-finger trackpad swipe slides aside to show rounded action
/// buttons: `leading` ones when swiped right, `trailing` ones when swiped left.
/// A swipe past the first action runs it. Vertical scrolling passes through.
/// Only the row whose id is in `openRow` stays open.
struct SwipeRow<Content: View>: View {
    let id: String
    @Binding var openRow: String?
    var leading: [SwipeAction] = []
    var trailing: [SwipeAction] = []
    @ViewBuilder var content: Content

    @State private var offset: CGFloat = 0
    @State private var isHovered = false
    @State private var monitor: Any?
    @State private var tracking = false

    private static var buttonWidth: CGFloat { 58 }
    private var leadingWidth: CGFloat { CGFloat(leading.count) * Self.buttonWidth }
    private var trailingWidth: CGFloat { CGFloat(trailing.count) * Self.buttonWidth }

    var body: some View {
        ZStack {
            HStack(spacing: 4) {
                if offset > 0 { buttons(leading) }
                Spacer(minLength: 0)
                if offset < 0 { buttons(trailing) }
            }
            content
                .offset(x: offset)
                // A click on an open row closes it instead of acting.
                .overlay {
                    if offset != 0 {
                        Color.clear.contentShape(.rect).offset(x: offset).onTapGesture { close() }
                    }
                }
        }
        .clipShape(.rect(cornerRadius: 12))
        .onHover { hovering in
            isHovered = hovering
            hovering ? install() : (tracking ? () : remove())
        }
        .onChange(of: openRow) { _, row in if row != id, offset != 0 { close() } }
        .onDisappear(perform: remove)
    }

    private func buttons(_ actions: [SwipeAction]) -> some View {
        HStack(spacing: 4) {
            ForEach(actions) { action in
                Button {
                    close()
                    action.action()
                } label: {
                    VStack(spacing: 3) {
                        Image(systemName: action.systemImage).font(.system(size: 13, weight: .semibold))
                        Text(action.title).font(.caption2.weight(.medium)).lineLimit(1)
                    }
                    .foregroundStyle(action.tint)
                    .frame(width: Self.buttonWidth - 4)
                    .frame(maxHeight: .infinity)
                    .background(action.tint.opacity(0.16), in: .rect(cornerRadius: 12))
                    .contentShape(.rect(cornerRadius: 12))
                }
                .buttonStyle(.plain)
                .help(action.title)
            }
        }
        .padding(.vertical, 2)
    }

    private func close() {
        withAnimation(.snappy(duration: 0.22)) { offset = 0 }
        if openRow == id { openRow = nil }
    }

    // MARK: - Trackpad

    /// Scroll events are read while the pointer is over the row. A gesture that
    /// starts mostly sideways belongs to the row; anything else scrolls the list.
    private func install() {
        guard monitor == nil else { return }
        monitor = NSEvent.addLocalMonitorForEvents(matching: .scrollWheel) { event in
            handle(event) ? nil : event
        }
    }

    private func remove() {
        if let monitor { NSEvent.removeMonitor(monitor) }
        monitor = nil
    }

    private func handle(_ event: NSEvent) -> Bool {
        guard event.hasPreciseScrollingDeltas else { return false }
        if event.phase == .began {
            tracking = abs(event.scrollingDeltaX) > abs(event.scrollingDeltaY) && (isHovered || openRow == id)
            if tracking { openRow = id }
            return tracking
        }
        // Momentum after a sideways gesture is swallowed so the list doesn't drift.
        if event.momentumPhase != [] { return tracking }
        guard tracking else { return false }
        switch event.phase {
        case .changed:
            let limitLeading = leading.isEmpty ? 0 : leadingWidth * 1.6
            let limitTrailing = trailing.isEmpty ? 0 : trailingWidth * 1.3
            // Follow the fingers whether or not natural scrolling is on.
            let delta = event.isDirectionInvertedFromDevice ? event.scrollingDeltaX : -event.scrollingDeltaX
            offset = min(max(offset + delta, -limitTrailing), limitLeading)
        case .ended, .cancelled:
            tracking = false
            settle()
            if !isHovered { remove() }
        default:
            break
        }
        return true
    }

    private func settle() {
        withAnimation(.snappy(duration: 0.25)) {
            if let first = leading.first, offset > leadingWidth * 1.35 {
                offset = 0
                openRow = nil
                first.action()
            } else if offset > leadingWidth / 2 {
                offset = leadingWidth
            } else if offset < -trailingWidth / 2 {
                offset = -trailingWidth
            } else {
                offset = 0
                if openRow == id { openRow = nil }
            }
        }
    }
}
