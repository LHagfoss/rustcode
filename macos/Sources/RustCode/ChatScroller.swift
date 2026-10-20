import AppKit
import RustCodeKit
import SwiftUI

/// The chat's scroll position measured from its real row heights, and the
/// knob the chat's scroller draws from it. Rows and the scroll view report
/// into it; only the scroller observes it, so measuring never redraws the
/// chat.
@MainActor
final class ChatScrollState: ObservableObject {
    @Published private(set) var knob: ChatScrollerKnob?
    /// The chat moved a moment ago, so the scroller shows.
    @Published private(set) var isScrolling = false

    /// The gaps of the chat's stack, which the map adds between rows.
    var map = ChatScrollMap(spacing: 18, topPadding: 20, bottomPadding: 2 + 18 + 1)
    /// The rows laid out now, in the scroll view's coordinates.
    private var frames: [Int: CGRect] = [:]
    private var scrollOffset: Double = 0
    private var viewport: Double = 0
    private(set) var insetTop: Double = 0
    private var insetBottom: Double = 0
    private var hideTask: Task<Void, Never>?
    /// The AppKit scroll view under the chat's SwiftUI one, which the
    /// scroller moves directly by exact distances.
    weak var scrollView: NSScrollView?

    /// The scroller's track, between the insets with a small margin.
    static let trackMargin: Double = 6
    var track: Double { max(visible - 2 * Self.trackMargin, 0) }

    func setRows(_ rows: [Int]) {
        #if DEBUG
        Self.current = self
        #endif
        map.setRows(rows)
        setNeedsUpdate()
    }

    func rowFrame(_ row: Int, _ frame: CGRect) {
        frames[row] = frame
        map.measured(row, height: frame.height)
        holdAnchor(row, frame)
        setNeedsUpdate()
    }

    // MARK: Older rows joining at the top (`TranscriptWindow`)

    #if DEBUG
    /// The open chat's state, for the debug driver.
    static weak var current: ChatScrollState?
    /// The row at the top of the view, where it is, and the rows mapped.
    var debugDescription: String {
        let top = reference.map { "top row \($0.key) at \(Int($0.value.minY))" } ?? "no rows"
        return "\(top), rows \(map.rows.count) first \(map.rows.first.map(String.init) ?? "-")"
    }
    #endif

    /// The chat's view is near the top of the rows it has: it adds older
    /// ones. Set by the chat.
    var nearTop: () -> Void = {}
    /// Rows this close to the top, in screens, ask for older ones.
    static let nearTopScreens: Double = 1.5
    /// A row kept where it was on screen while rows are added above it:
    /// where it was in the view, the scroll offset then, and until when.
    /// Rows added above a lazy stack take a guessed height, then their real
    /// one over the next passes, and each pass pushed the reader's message
    /// down: until the window closes, every move of the row that the
    /// reader's own scrolling does not explain is scrolled back.
    private var anchor: (row: Int, minY: Double, offset: Double?, until: Date)?
    /// How long rows added above keep being corrected for.
    static let anchorDuration: TimeInterval = 1.5
    /// The row that marks earlier messages; never an anchor, as it stays on top.
    var earlierRow: Int?

    private var clipOffset: Double? { scrollView.map { Double($0.contentView.bounds.minY) } }

    /// The row at the top of the view and where it is, other than the
    /// earlier-messages row.
    func topRow() -> (row: Int, minY: Double)? {
        frames.filter { $0.key != earlierRow }.min { distance($0.value) < distance($1.value) }
            .map { ($0.key, Double($0.value.minY)) }
    }

    /// Rows are about to be added above the view: ask for no more until the
    /// row in view is held again.
    private var pausedUntil = Date.distantPast

    func pauseNearTop() {
        pausedUntil = Date().addingTimeInterval(Self.anchorDuration)
    }

    /// Hold `row` at `minY` in the view, as it was before rows were added
    /// above it and the chat was scrolled back to it.
    func hold(_ row: Int, at minY: Double) {
        let until = Date().addingTimeInterval(Self.anchorDuration)
        // The scroll back to the row lands with its first report; only
        // scrolling after that is the reader's.
        anchor = (row, minY, nil, until)
        pausedUntil = until
        // Look again once the hold ends, in case the view is still near the top.
        Task { @MainActor [weak self] in
            try? await Task.sleep(for: .seconds(Self.anchorDuration + 0.1))
            self?.setNeedsUpdate()
        }
    }

    /// Rows are being added above the view, so nothing more is asked for.
    var isAnchoring: Bool { Date() < pausedUntil }

    private func holdAnchor(_ row: Int, _ frame: CGRect) {
        guard let anchor, anchor.row == row, let offset = clipOffset else { return }
        guard Date() < anchor.until else {
            self.anchor = nil
            return
        }
        // The reader scrolling down by d moves the row up by d.
        let expected = anchor.minY - (anchor.offset.map { offset - $0 } ?? 0)
        let moved = frame.minY - expected
        guard abs(moved) > 0.5 else {
            if anchor.offset == nil { self.anchor = (row, expected, offset, anchor.until) }
            return
        }
        scroll(by: moved, animated: false)
        self.anchor = (row, expected, clipOffset ?? offset + moved, anchor.until)
    }

    func rowGone(_ row: Int) {
        frames[row] = nil
    }

    /// The scroll view's own geometry changed. `offset` is in its estimated
    /// content coordinates, used only to tell that the chat moved.
    func geometryChanged(offset: Double, viewport: Double, insetTop: Double, insetBottom: Double) {
        let moved = abs(offset - scrollOffset) > 0.5
        scrollOffset = offset
        self.viewport = viewport
        self.insetTop = insetTop
        self.insetBottom = insetBottom
        setNeedsUpdate()
        if moved { flash() }
    }

    /// The laid-out row at the top of the visible area, else the nearest.
    /// The scroll view's coordinates start at the top of that area, below
    /// the title bar's inset.
    private var reference: (key: Int, value: CGRect)? {
        frames.min { distance($0.value) < distance($1.value) }
    }
    /// Each laid-out row's top at the last update.
    private var lastPositions: [Int: Double] = [:]

    private var lastOffset: (offset: Double, revision: Int)?

    /// How far the chat scrolled since the last update. With the map
    /// unchanged that is exactly how far its offset moved; after a row's
    /// height changed the map, a row seen both times tells instead.
    private func scrolled(to offset: Double, preferring row: Int) -> Double? {
        if let last = lastOffset, last.revision == map.revision { return offset - last.offset }
        if let before = lastPositions[row], let now = frames[row] { return before - now.minY }
        for (row, before) in lastPositions {
            if let now = frames[row] { return before - now.minY }
        }
        return nil
    }

    /// How far the top of the visible area is below the top of the content.
    var mappedOffset: Double? {
        guard let (row, frame) = reference, let top = map.top(of: row) else { return nil }
        return top - frame.minY
    }

    /// The scroll view's height between its insets.
    var visible: Double { max(viewport - insetTop - insetBottom, 0) }

    private func distance(_ frame: CGRect) -> Double {
        frame.minY <= 0 && frame.maxY > 0 ? 0 : min(abs(frame.minY), abs(frame.maxY))
    }

    /// How far to move the chat so the knob's top lands at `top`.
    func delta(forKnobTop top: Double) -> Double? {
        guard let knob, let current = mappedOffset else { return nil }
        let target = ChatScrollerKnob.offset(forKnobTop: top, knobHeight: knob.height,
                                             contentHeight: map.contentHeight(), visible: visible, track: track)
        return target - current
    }

    /// A page: the visible height, less a little kept for context.
    var page: Double { TranscriptKeys.pageDistance(visible: visible) }

    private var updatePending = false

    /// Rows report one by one as a frame lays out; place the knob once they
    /// all have.
    private func setNeedsUpdate() {
        guard !updatePending else { return }
        updatePending = true
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            self.updatePending = false
            self.update()
        }
    }

    private func update() {
        // While the lazy stack re-anchors, rows can briefly report places far
        // from the visible area; wait for one that spans its top.
        guard let (row, frame) = reference, distance(frame) <= visible, let top = map.top(of: row) else { return }
        let offset = top - frame.minY
        if offset < visible * Self.nearTopScreens, !isAnchoring { nearTop() }
        let content = map.contentHeight()
        guard let target = ChatScrollerKnob(offset: offset, contentHeight: content, visible: visible, track: track) else {
            if knob != nil { knob = nil }
            lastPositions = [:]
            lastOffset = nil
            return
        }
        var knob = target
        // A row seen last time and now tells how far the chat really
        // scrolled; the top row is preferred, as rows above it may still be
        // taking their real height.
        if let previous = self.knob, let moved = scrolled(to: offset, preferring: row) {
            knob = previous.following(target, moved: moved, offset: offset,
                                      contentHeight: content, visible: visible, track: track)
        }
        lastPositions = frames.mapValues { Double($0.minY) }
        lastOffset = (offset, map.revision)
        if let old = self.knob, abs(knob.top - old.top) < 0.25, abs(knob.height - old.height) < 0.25 { return }
        self.knob = knob
    }

    /// Move the chat by `delta` points, negative up, within its bounds.
    func scroll(by delta: Double, animated: Bool) {
        guard let scrollView, let document = scrollView.documentView else { return }
        let clip = scrollView.contentView
        var bounds = clip.bounds
        bounds.origin.y += document.isFlipped ? delta : -delta
        let origin = clip.constrainBoundsRect(bounds).origin
        if animated {
            NSAnimationContext.runAnimationGroup { context in
                context.duration = 0.2
                context.allowsImplicitAnimation = true
                clip.animator().setBoundsOrigin(origin)
            }
        } else {
            clip.setBoundsOrigin(origin)
        }
        scrollView.reflectScrolledClipView(clip)
    }

    func flash() {
        if !isScrolling { isScrolling = true }
        hideTask?.cancel()
        hideTask = Task { @MainActor [weak self] in
            try? await Task.sleep(for: .seconds(1.2))
            guard !Task.isCancelled else { return }
            self?.isScrolling = false
        }
    }
}

/// An overlay scroller for the chat, drawn from `ChatScrollState` instead of
/// the lazy stack's guessed content size. It shows while the chat scrolls
/// and while the pointer is over it; drag the knob, or click the track to
/// page.
struct ChatScroller: View {
    @ObservedObject var state: ChatScrollState
    /// Moves the chat by this many points; negative is up.
    let scrollBy: (Double, Bool) -> Void

    @State private var isHovered = false
    @State private var dragStart: Double?

    private var alwaysShown: Bool { NSScroller.preferredScrollerStyle == .legacy }

    var body: some View {
        let knob = state.knob
        let shown = knob != nil && (alwaysShown || isHovered || dragStart != nil || state.isScrolling)
        let wide = isHovered || dragStart != nil
        ZStack(alignment: .topTrailing) {
            // The hit area along the edge; clicks above or below the knob page.
            Color.clear
                .frame(width: 16)
                .contentShape(.rect)
                .onTapGesture { location in page(toward: location.y) }
            if let knob {
                Capsule()
                    .fill(Color.primary.opacity(wide ? 0.45 : 0.3))
                    .frame(width: wide ? 9 : 6, height: knob.height)
                    .padding(.trailing, 3)
                    .offset(y: knob.top)
                    .gesture(drag(knob))
            }
        }
        .frame(width: 16, height: state.track, alignment: .topTrailing)
        // The overlay is already inside the title bar's inset.
        .padding(.top, ChatScrollState.trackMargin)
        .opacity(shown ? 1 : 0)
        .animation(.easeOut(duration: shown ? 0.1 : 0.4), value: shown)
        .animation(.snappy(duration: 0.15), value: wide)
        .onHover { isHovered = $0 }
        .accessibilityHidden(true)
    }

    private func drag(_ knob: ChatScrollerKnob) -> some Gesture {
        DragGesture(minimumDistance: 0, coordinateSpace: .global)
            .onChanged { value in
                if dragStart == nil { dragStart = knob.top }
                guard let start = dragStart,
                      let delta = state.delta(forKnobTop: start + value.translation.height),
                      abs(delta) > 0.5 else { return }
                scrollBy(delta, false)
            }
            .onEnded { _ in dragStart = nil }
    }

    private func page(toward y: Double) {
        guard let knob = state.knob else { return }
        if y < knob.top {
            scrollBy(-state.page, true)
        } else if y > knob.top + knob.height {
            scrollBy(state.page, true)
        }
    }
}

extension View {
    /// Reports this chat row's real frame to the chat's scroller.
    func reportsChatRow(_ row: Int, to state: ChatScrollState) -> some View {
        onGeometryChange(for: CGRect.self) { $0.frame(in: .scrollView) } action: { state.rowFrame(row, $0) }
            .onDisappear { state.rowGone(row) }
    }
}

/// Finds the AppKit scroll view a SwiftUI scroll view is drawn with.
struct EnclosingScrollView: NSViewRepresentable {
    let found: (NSScrollView) -> Void

    func makeNSView(context: Context) -> Probe { Probe(found: found) }

    func updateNSView(_ view: Probe, context: Context) {}

    final class Probe: NSView {
        let found: (NSScrollView) -> Void

        init(found: @escaping (NSScrollView) -> Void) {
            self.found = found
            super.init(frame: .zero)
        }

        @available(*, unavailable)
        required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }

        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            report()
        }

        /// SwiftUI can add the probe to a window before it puts it inside the
        /// scroll view, so the scroller had none to move: look again once
        /// the probe is laid out, and after the pass that placed it.
        override func layout() {
            super.layout()
            report()
        }

        private weak var reported: NSScrollView?

        private func report() {
            if let scrollView = enclosingScrollView {
                guard scrollView !== reported else { return }
                reported = scrollView
                found(scrollView)
            } else if window != nil {
                DispatchQueue.main.async { [weak self] in
                    guard let self, let scrollView = self.enclosingScrollView, scrollView !== self.reported else { return }
                    self.reported = scrollView
                    self.found(scrollView)
                }
            }
        }

        override func hitTest(_ point: NSPoint) -> NSView? { nil }
    }
}
