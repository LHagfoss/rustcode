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
    weak var scrollView: NSScrollView? {
        didSet {
            guard scrollView !== oldValue else { return }
            observeDocumentSize()
        }
    }
    private var documentObserver: NSObjectProtocol?

    /// The scroller's track, between the insets with a small margin.
    static let trackMargin: Double = 6
    var track: Double { max(visible - 2 * Self.trackMargin, 0) }

    func setRows(_ rows: [Int]) {
        #if DEBUG
        Self.current = self
        #endif
        map.setRows(rows)
        drawnRows.invalidate()
        setNeedsUpdate()
    }

    // MARK: The session timeline (`TimelineRail`, #2309)

    /// The timeline's markers; only the rail observes it.
    let rail = TimelineRailState()
    private var turns: [TimelineTurn] = []
    /// The turns' tops in the content, for the map's revision they are of.
    private var turnTops: (revision: Int, tops: [Double?])?

    /// The messages the user sent, oldest first, for the timeline.
    func setTurns(_ turns: [TimelineTurn]) {
        guard turns != self.turns else { return }
        self.turns = turns
        turnTops = nil
        if rail.turnCount != turns.count { rail.turnCount = turns.count }
        setNeedsUpdate()
    }

    /// Places the timeline's markers for the view at `offset`. The turns'
    /// tops are worked out again only when the map changed.
    private func updateRail(offset: Double) {
        guard turns.count >= 2 else {
            if rail.layout != .empty { rail.layout = .empty }
            return
        }
        if turnTops?.revision != map.revision {
            turnTops = (map.revision, map.tops(of: turns.map(\.id)))
        }
        let layout = TimelineLayout(turns: turns, tops: turnTops?.tops ?? [], offset: offset,
                                    visible: visible, follows: !anchoredToTop)
        if layout.differs(from: rail.layout) { rail.layout = layout }
    }

    func rowFrame(_ row: Int, _ frame: CGRect) {
        let before = map.laidOutHeight(of: row)
        frames[row] = frame
        map.measured(row, height: frame.height)
        holdAnchor(row, frame)
        keepToggled(row)
        keepPlace(above: row, frame, before: before)
        setNeedsUpdate()
    }

    // MARK: Older rows drawn in full or as stand-ins (`DrawnRows`)

    /// The older rows drawn in full; the rest stand in at their height.
    let drawn = DrawnRowsState()
    private var drawnRows = DrawnRows()
    /// The chat is let go of its end and anchored to its top. Set by the chat.
    var anchoredToTop = false {
        // The timeline's current turn follows the end while held.
        didSet { if anchoredToTop != oldValue { setNeedsUpdate() } }
    }
    /// The content offset less the scroll view's own, to tell where the
    /// view is when no row drawn in full is in it.
    private var clipShift: Double?
    /// Rows moved by a scroll back since they last reported.
    private var keptPlace = false
    /// A scroll down still owed: rows report a new height before the
    /// document takes it, and the scroll waits for the room.
    private var pendingShift: Double?

    /// Until when the chat is going somewhere on purpose, so rows taking
    /// their height on the way are not scrolled back: that would hold the
    /// chat where it was.
    private var jumpUntil = Date.distantPast

    /// The chat is about to go somewhere on purpose: Home, a find match, or
    /// back to the row in view once older rows joined.
    func willJump() {
        jumpUntil = Date().addingTimeInterval(0.5)
    }

    /// The height `row` stands in at while not drawn in full.
    func standInHeight(of row: Int) -> Double { map.standInHeight(of: row) }

    /// A row wholly above the view took another height, measured for the
    /// first time or again: everything below it moved by the difference,
    /// which is scrolled back so the reader's place stays.
    private func keepPlace(above row: Int, _ frame: CGRect, before: Double?) {
        guard anchoredToTop, anchor == nil, toggled == nil, Date() >= jumpUntil, let before else { return }
        let delta = frame.height - before
        guard abs(delta) > 0.5, frame.minY + before <= 0.5 else { return }
        shift(by: delta)
    }

    private func shift(by delta: Double) {
        let before = clipOffset ?? 0
        scroll(by: delta, animated: false)
        let applied = (clipOffset ?? before) - before
        let owed = delta - applied
        pendingShift = abs(owed) > 0.5 ? (pendingShift ?? 0) + owed : pendingShift
        keptPlace = true
    }

    private func observeDocumentSize() {
        documentObserver.map(NotificationCenter.default.removeObserver)
        documentObserver = nil
        guard let document = scrollView?.documentView else { return }
        document.postsFrameChangedNotifications = true
        documentObserver = NotificationCenter.default.addObserver(
            forName: NSView.frameDidChangeNotification, object: document, queue: nil
        ) { [weak self] _ in
            MainActor.assumeIsolated {
                // The document took the toggled message's new height: the
                // scroll view kept its offset, so put the message back
                // before the frame is drawn.
                if let self, self.toggled != nil { self.scrollToggledBack() }
                guard let self, let owed = self.pendingShift else { return }
                self.pendingShift = nil
                self.shift(by: owed)
                // As far as this height allows; a later one owes no more.
                self.pendingShift = nil
            }
        }
    }

    private func updateDrawn() {
        guard visible > 0 else { return }
        var offset: Double?
        if !keptPlace, let (row, frame) = reference, distance(frame) <= visible, let top = map.top(of: row) {
            offset = top - frame.minY
            if let clip = clipOffset { clipShift = top - frame.minY - clip }
        } else if let clip = clipOffset, let clipShift {
            offset = clip + clipShift
        }
        keptPlace = false
        guard let offset else { return }
        if drawnRows.update(map: &map, offset: offset, visible: visible) {
            drawn.ids = drawnRows.ids
        }
    }

    // MARK: Older rows joining at the top (`TranscriptWindow`)

    #if DEBUG
    /// The open chat's state, for the debug driver.
    static weak var current: ChatScrollState?
    /// Lets go of the end, as the reader scrolling up does.
    var debugLetGo: () -> Void = {}
    /// Whether the chat follows its end.
    var debugFollows: () -> Bool = { false }
    /// Opens or folds a long message, as its Show more control does.
    var debugToggleMessage: (Int) -> Void = { _ in }
    /// Goes to a turn, as its timeline marker does.
    var debugJumpToTurn: (Int) -> Void = { _ in }
    /// Where `row` is in the view, as it last reported.
    func debugFrame(_ row: Int) -> CGRect? { frames[row] }
    /// The row at the top of the view, where it is, and the rows mapped.
    var debugDescription: String {
        let top = reference.map { "top row \($0.key) at \(Int($0.value.minY))" } ?? "no rows"
        return "\(top), rows \(map.rows.count) first \(map.rows.first.map(String.init) ?? "-")"
    }
    /// Where `row` is drawn now: its top in the content, which layout
    /// reports, less the scroll view's own offset. Unlike its frame, never
    /// stale after the chat was scrolled outside a layout pass.
    func debugScreenTop(_ row: Int) -> Double? {
        guard let top = contentTops[row], let offset = clipOffset else { return nil }
        return top - offset
    }
    /// Each row's top in the chat's content, as layout last placed it.
    private var contentTops: [Int: Double] = [:]
    /// The name of the chat content's coordinate space.
    static let contentSpace = "chatContent"
    func debugContentTop(_ row: Int, _ top: Double) { contentTops[row] = top }
    /// The laid-out rows that reach into the visible area, and the clip's
    /// place and the document's height.
    var debugVisible: String {
        let shown = frames.filter { $0.value.maxY > 0 && $0.value.minY < visible }.keys.sorted()
        let clip = scrollView.map { "clip y \(Int($0.contentView.bounds.minY)) doc \(Int($0.documentView?.frame.height ?? 0))" } ?? "-"
        let rows = shown.map { ($0 == Int.max ? "W" : String($0)) + "@\(Int(frames[$0]!.minY))+\(Int(frames[$0]!.height))" }
        return "\(debugFollows() ? "follows" : "let go") \(clip) visible \(Int(visible)) first \(map.rows.dropFirst().first ?? -1) hold \(isAnchoring) laid \(frames.count) shown [\(rows.joined(separator: ","))]"
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

    /// A long message the reader opened or folded (#2303), the edge of it
    /// kept in place, where in the view, and until when.
    private var toggled: (row: Int, bottom: Bool, y: Double, until: Date)?
    /// A scroll that keeps the toggled message in place is on its way.
    private var togglePending = false

    /// The reader opened or folded long message `row` (#2303): it keeps its
    /// top where it is as it takes its new height. Folding one whose top is
    /// above the view keeps its end instead, where Show less was, so the
    /// reader is not left in the rows that came after it.
    func holdToggled(_ row: Int, folding: Bool) {
        guard let frame = frames[row] else { return }
        let bottom = folding && frame.minY < 0
        toggled = (row, bottom, Double(bottom ? frame.maxY : frame.minY), Date().addingTimeInterval(0.5))
    }

    /// The toggled message reported its place: scroll it back to where it
    /// was, once the layout pass it reported in is over. Scrolled during
    /// it, the scroll view puts back the offset it had before as the
    /// document takes its new height.
    private func keepToggled(_ row: Int) {
        guard let toggled, toggled.row == row, !togglePending else { return }
        guard Date() < toggled.until, !ChatScrollInput.shared.isActive else {
            self.toggled = nil
            return
        }
        togglePending = true
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            self.togglePending = false
            self.scrollToggledBack()
        }
    }

    private func scrollToggledBack() {
        guard let toggled, let frame = frames[toggled.row] else { return }
        let moved = (toggled.bottom ? frame.maxY : frame.minY) - toggled.y
        guard abs(moved) > 0.5 else { return }
        let before = clipOffset ?? 0
        scroll(by: moved, animated: false)
        // Until it reports again, the row is where the scroll put it.
        frames[toggled.row] = frame.offsetBy(dx: 0, dy: -((clipOffset ?? before) - before))
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
        updateDrawn()
        // While the lazy stack re-anchors, rows can briefly report places far
        // from the visible area; wait for one that spans its top.
        guard let (row, frame) = reference, distance(frame) <= visible, let top = map.top(of: row) else { return }
        let offset = top - frame.minY
        updateRail(offset: offset)
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

/// The older rows the chat draws in full (`DrawnRows`). Only the rows of
/// older messages observe it.
@MainActor
final class DrawnRowsState: ObservableObject {
    @Published var ids: Set<Int> = []
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
            #if DEBUG
            // Where layout puts the row in the content, for the debug
            // driver's measures; unlike its place in the view, never stale.
            .onGeometryChange(for: Double.self) {
                $0.frame(in: .named(ChatScrollState.contentSpace)).minY
            } action: { state.debugContentTop(row, $0) }
            #endif
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
