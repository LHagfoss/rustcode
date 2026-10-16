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
    /// Sideways movement since the gesture began, before it is claimed.
    @State private var pending: CGSize = .zero
    @State private var decided = false

    private static var buttonWidth: CGFloat { 58 }
    private var leadingWidth: CGFloat { CGFloat(leading.count) * Self.buttonWidth }
    private var trailingWidth: CGFloat { CGFloat(trailing.count) * Self.buttonWidth }

    var body: some View {
        ZStack {
            // The buttons are uncovered as the row slides, never drawn under it.
            HStack(spacing: 0) {
                buttons(leading)
                    .frame(width: max(0, offset), alignment: .leading)
                    .clipped()
                    .opacity(offset > 0 ? 1 : 0)
                Spacer(minLength: 0)
                buttons(trailing)
                    .frame(width: max(0, -offset), alignment: .trailing)
                    .clipped()
                    .opacity(offset < 0 ? 1 : 0)
            }
            content
                .background(Color.sidebarBackground)
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
        .padding(.horizontal, 2)
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

    /// The first events of a gesture often carry no movement, so the axis is
    /// decided once the fingers have moved a few points: mostly sideways makes
    /// it the row's, anything else is left to the list.
    private func handle(_ event: NSEvent) -> Bool {
        guard event.hasPreciseScrollingDeltas else { return false }
        if event.momentumPhase != [] { return tracking }
        switch event.phase {
        case .began:
            tracking = false
            decided = false
            pending = .zero
            return false
        case .changed:
            let dx = event.isDirectionInvertedFromDevice ? event.scrollingDeltaX : -event.scrollingDeltaX
            if !decided {
                pending.width += dx
                pending.height += event.scrollingDeltaY
                guard abs(pending.width) + abs(pending.height) > 6 else { return false }
                decided = true
                tracking = abs(pending.width) > abs(pending.height) * 1.5 && (isHovered || openRow == id)
                guard tracking else { return false }
                openRow = id
                move(by: pending.width)
                return true
            }
            guard tracking else { return false }
            move(by: dx)
            return true
        case .ended, .cancelled:
            guard tracking else { return false }
            tracking = false
            settle()
            if !isHovered { remove() }
            return true
        default:
            return tracking
        }
    }

    /// Follow the fingers, with resistance past the buttons.
    private func move(by delta: CGFloat) {
        let limitLeading = leading.isEmpty ? 0 : leadingWidth * 1.6
        let limitTrailing = trailing.isEmpty ? 0 : trailingWidth * 1.25
        let beyond = offset > leadingWidth || offset < -trailingWidth
        offset = min(max(offset + (beyond ? delta * 0.4 : delta), -limitTrailing), limitLeading)
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
