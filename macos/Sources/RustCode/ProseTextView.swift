import AppKit
import RustCodeKit
import SwiftUI

/// A finished run of prose, styled once for AppKit, with the heights it was
/// measured at. Kept per run of markdown, so a reply redrawn unchanged, or
/// the settled paragraphs above a streaming tail, are not laid out again.
/// Used on the main thread only, as views are.
final class ProseContent {
    /// The run as SwiftUI styled it; the streaming tail draws this.
    let text: AttributedString
    private var appKit: NSAttributedString?
    private var highlighted: (query: String, string: NSAttributedString)?
    private var heights: [CGFloat: CGFloat] = [:]

    init(_ text: AttributedString) {
        self.text = text
    }

    /// The run for the text view, with find matches marked when `query` is set.
    func attributed(highlighting query: String) -> NSAttributedString {
        let base = appKit ?? ProseStyle.appKit(text)
        appKit = base
        let query = query.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !query.isEmpty else { return base }
        if let highlighted, highlighted.query == query { return highlighted.string }
        let string = ProseStyle.highlight(base, query: query)
        highlighted = (query, string)
        return string
    }

    /// The height the run takes at `width`. A find highlight does not change
    /// the layout, so both share it.
    @MainActor func height(for width: CGFloat, measure: ((CGFloat) -> CGFloat)? = nil) -> CGFloat {
        if let height = heights[width] { return height }
        let height = measure?(width) ?? ProseMeasure.height(of: attributed(highlighting: ""), width: width)
        if heights.count > 8 { heights.removeAll() }
        heights[width] = height
        return height
    }
}

/// How a run of prose, styled with SwiftUI's attributes, reads in AppKit:
/// the chat's body font at its weights and the heading sizes, its colors,
/// links underlined in the brand color, and two points between lines.
enum ProseStyle {
    static let lineSpacing: CGFloat = 2

    static func appKit(_ text: AttributedString) -> NSAttributedString {
        let result = NSMutableAttributedString(string: String(text.characters))
        let paragraph = NSMutableParagraphStyle()
        paragraph.lineSpacing = lineSpacing
        // The small blank line between blocks sits a point taller than
        // AppKit sets a 5 pt line, as `Text` draws it, so blocks keep their spacing.
        let gap = NSMutableParagraphStyle()
        gap.lineSpacing = lineSpacing
        gap.minimumLineHeight = 7
        let body = NSFont.preferredFont(forTextStyle: .body)
        for run in text.runs {
            let range = NSRange(run.range, in: text)
            let isGap = run.font == Font.system(size: 5)
            var attributes: [NSAttributedString.Key: Any] = [.paragraphStyle: isGap ? gap : paragraph]
            let intent = run.inlinePresentationIntent ?? []
            attributes[.font] = font(base: run.font.flatMap(font(for:)) ?? body, intent: intent)
            attributes[.foregroundColor] = run.foregroundColor.map(color(for:)) ?? NSColor.labelColor
            if intent.contains(.strikethrough) { attributes[.strikethroughStyle] = NSUnderlineStyle.single.rawValue }
            if let link = run.link {
                attributes[.link] = link
                attributes[.toolTip] = link.absoluteString
                attributes[.underlineStyle] = NSUnderlineStyle.single.rawValue
                attributes[.underlineColor] = NSColor(Color.brand).withAlphaComponent(0.45)
            }
            result.setAttributes(attributes, range: range)
        }
        return result
    }

    /// The fonts `MarkdownText` sets on a run: its heading sizes and the
    /// small blank line between blocks.
    private static func font(for font: Font) -> NSFont? {
        if font == Font.title2.bold() { return bold(.title2) }
        if font == Font.title3.bold() { return bold(.title3) }
        if font == Font.headline { return NSFont.preferredFont(forTextStyle: .headline) }
        if font == Font.system(size: 5) { return NSFont.systemFont(ofSize: 5) }
        return nil
    }

    private static func bold(_ style: NSFont.TextStyle) -> NSFont {
        let font = NSFont.preferredFont(forTextStyle: style)
        return NSFont(descriptor: font.fontDescriptor.withSymbolicTraits(.bold), size: font.pointSize) ?? font
    }

    private static func font(base: NSFont, intent: InlinePresentationIntent) -> NSFont {
        let bold = intent.contains(.stronglyEmphasized) || base.fontDescriptor.symbolicTraits.contains(.bold)
        var font = base
        if intent.contains(.code) {
            font = NSFont.monospacedSystemFont(ofSize: base.pointSize, weight: bold ? .bold : .regular)
        }
        var traits = font.fontDescriptor.symbolicTraits
        if bold { traits.insert(.bold) }
        if intent.contains(.emphasized) { traits.insert(.italic) }
        guard traits != font.fontDescriptor.symbolicTraits else { return font }
        return NSFont(descriptor: font.fontDescriptor.withSymbolicTraits(traits), size: font.pointSize) ?? font
    }

    private static func color(for color: Color) -> NSColor {
        if color == Color.secondary { return .secondaryLabelColor }
        if color == Color.primary { return .labelColor }
        return NSColor(color)
    }

    /// The find bar's matches marked, as `FindHighlight` marks them in a `Text`.
    static func highlight(_ string: NSAttributedString, query: String) -> NSAttributedString {
        let result = NSMutableAttributedString(attributedString: string)
        let text = string.string as NSString
        let color = NSColor(Color.attention).withAlphaComponent(0.35)
        var search = NSRange(location: 0, length: text.length)
        while search.length > 0 {
            let found = text.range(of: query, options: [.caseInsensitive, .diacriticInsensitive], range: search)
            guard found.location != NSNotFound, found.length > 0 else { break }
            result.addAttribute(.backgroundColor, value: color, range: found)
            search = NSRange(location: NSMaxRange(found), length: text.length - NSMaxRange(found))
        }
        return result
    }
}

/// Lays text out as the prose view does, to know its size before a view
/// shows it: one prose view, never drawn, reused.
@MainActor
enum ProseMeasure {
    private static let view = ProseNSTextView.make()

    static func height(of string: NSAttributedString, width: CGFloat) -> CGFloat {
        view.textStorage?.setAttributedString(string)
        return view.height(for: width)
    }

    /// The width of the run's longest line, for a proposal with no width.
    static func width(of string: NSAttributedString) -> CGFloat {
        view.textStorage?.setAttributedString(string)
        guard let container = view.textContainer, let manager = view.layoutManager else { return 0 }
        container.size = NSSize(width: 100_000, height: CGFloat.greatestFiniteMagnitude)
        manager.ensureLayout(for: container)
        return ceil(manager.usedRect(for: container).width)
    }
}

/// Which item of the transcript the prose belongs to and which of its
/// blocks, so a run can say where it is in `TranscriptSelection`.
struct ProsePlace: Equatable {
    var item: Int
    var block: Int

    /// A run's index within its item: blocks hold far fewer than a thousand
    /// runs of prose, so this keeps the item's runs in order.
    func run(_ segment: Int) -> TranscriptSelection.Run {
        TranscriptSelection.Run(item: item, segment: block * 1_000 + segment)
    }
}

extension EnvironmentValues {
    @Entry var prosePlace: ProsePlace?
}

/// A finished run of prose in a non-editable text view, so the
/// chat owns its layout and selection (#2440): a SwiftUI `Text` keeps both
/// to itself. Links open on a click with the pointing hand over them and
/// their address on hover; the message's own menu, from `messageMenu`, still
/// takes a right-click above it.
struct ProseTextView: NSViewRepresentable {
    let content: ProseContent
    var run: TranscriptSelection.Run?
    @Environment(\.findQuery) private var findQuery

    func makeNSView(context: Context) -> ProseNSTextView {
        let view = ProseNSTextView.make()
        view.delegate = ProseSelection.shared
        ProseSelection.shared.register(view)
        apply(to: view)
        return view
    }

    func updateNSView(_ view: ProseNSTextView, context: Context) {
        apply(to: view)
    }

    static func dismantleNSView(_ view: ProseNSTextView, coordinator: ()) {
        ProseSelection.shared.unregister(view)
    }

    private func apply(to view: ProseNSTextView) {
        let string = content.attributed(highlighting: findQuery)
        guard view.shown !== string || view.run != run else { return }
        view.shown = string
        view.content = content
        view.run = run
        ProseSelection.shared.replacing(view) {
            view.textStorage?.setAttributedString(string)
        }
    }

    func sizeThatFits(_ proposal: ProposedViewSize, nsView: ProseNSTextView, context: Context) -> CGSize? {
        let width: CGFloat
        if let proposed = proposal.width, proposed.isFinite {
            width = proposed
        } else {
            width = ProseMeasure.width(of: content.attributed(highlighting: ""))
        }
        // Measured by the view itself when it shows this run, so the layout
        // made to measure is the one it draws.
        let shows = nsView.content === content
        return CGSize(width: width, height: content.height(for: width, measure: shows ? nsView.height(for:) : nil))
    }
}

final class ProseNSTextView: NSTextView {
    /// What it shows, to tell a new run from a redraw of the same one.
    var shown: NSAttributedString?
    var content: ProseContent?
    var run: TranscriptSelection.Run?

    /// A view set up for a run of prose. TextKit 1: for short static runs
    /// it lays out and draws in about half the time TextKit 2 took, which
    /// doubled the time to scroll new rows in (measured with `timescroll`).
    static func make() -> ProseNSTextView {
        let view = ProseNSTextView(usingTextLayoutManager: false)
        view.isEditable = false
        view.isSelectable = true
        view.isRichText = true
        view.importsGraphics = false
        view.allowsUndo = false
        view.drawsBackground = false
        view.backgroundColor = .clear
        view.focusRingType = .none
        view.textContainerInset = .zero
        view.isVerticallyResizable = false
        view.isHorizontallyResizable = false
        view.textContainer?.lineFragmentPadding = 0
        view.textContainer?.widthTracksTextView = true
        view.textContainer?.size = NSSize(width: 100, height: CGFloat.greatestFiniteMagnitude)
        // The run's own link colour and underline; only the cursor is added.
        view.linkTextAttributes = [.cursor: NSCursor.pointingHand]
        return view
    }

    /// Lays its text out at `width` and returns the height it takes.
    func height(for width: CGFloat) -> CGFloat {
        guard let container = textContainer, let manager = layoutManager else { return 0 }
        container.size = NSSize(width: max(width, 1), height: CGFloat.greatestFiniteMagnitude)
        manager.ensureLayout(for: container)
        return ceil(manager.usedRect(for: container).height)
    }

    override var intrinsicContentSize: NSSize {
        NSSize(width: NSView.noIntrinsicMetric, height: NSView.noIntrinsicMetric)
    }

    // The chat scrolls itself: a selection never moves it to show its end.
    override func scrollRangeToVisible(_ range: NSRange) {}

    // A selection ends when the keyboard leaves its text, as a SwiftUI
    // `Text`'s does, so only one shows at a time.
    override func resignFirstResponder() -> Bool {
        guard super.resignFirstResponder() else { return false }
        if !ProseSelection.shared.isTracking { ProseSelection.shared.clear() }
        return true
    }

    // A press that starts a drag selects through the model, so the drag can
    // carry on into the runs and messages around this one (#2440). A link,
    // a double or triple click and a shift-click keep the text view's own
    // handling.
    override func mouseDown(with event: NSEvent) {
        guard event.clickCount == 1, !event.modifierFlags.contains(.shift), run != nil,
              !hasLink(at: event.locationInWindow) else { return super.mouseDown(with: event) }
        ProseSelection.shared.track(from: self, press: event)
    }

    // A selection over several runs copies from the model, a blank line
    // between runs; within one run the text view copies as it always has.
    override func copy(_ sender: Any?) {
        guard let text = ProseSelection.shared.textAcrossRuns() else { return super.copy(sender) }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
    }

    /// The UTF-16 offset a point of the window falls at, held to the text.
    func offset(at windowPoint: NSPoint) -> Int {
        let index = characterIndexForInsertion(at: convert(windowPoint, from: nil))
        return min(max(index, 0), (string as NSString).length)
    }

    private func hasLink(at windowPoint: NSPoint) -> Bool {
        guard let manager = layoutManager, let container = textContainer, let storage = textStorage,
              storage.length > 0 else { return false }
        let point = convert(windowPoint, from: nil)
        var fraction: CGFloat = 0
        let index = manager.characterIndex(for: point, in: container, fractionOfDistanceBetweenInsertionPoints: &fraction)
        guard index < storage.length, manager.boundingRect(forGlyphRange: manager.glyphRange(
            forCharacterRange: NSRange(location: index, length: 1), actualCharacterRange: nil), in: container)
            .insetBy(dx: -2, dy: -2).contains(point) else { return false }
        return storage.attribute(.link, at: index, effectiveRange: nil) != nil
    }

    // A menu of the views around it answers a right-click before the text
    // view's own. A message's menu needs none of this: `messageMenu` lays
    // a view over the message that takes the right-click first.
    override func menu(for event: NSEvent) -> NSMenu? {
        var view = superview
        while let current = view {
            if let menu = current.menu(for: event) { return menu }
            view = current.superview
        }
        return super.menu(for: event)
    }
}

/// The chat's prose selection: the text views drawn now and the selection
/// in `TranscriptSelection` terms, which a redrawn run restores its
/// highlight from. The chat's dismissal (a click outside text #2447, Esc
/// #2468) ends it through `clear()`.
@MainActor
final class ProseSelection: NSObject, NSTextViewDelegate {
    static let shared = ProseSelection()

    private(set) var selection = TranscriptSelection()
    private let views = NSHashTable<ProseNSTextView>.weakObjects()
    /// What each run drawn so far shows, so a copy can take in runs that
    /// have undrawn since the drag passed them.
    private var texts: [TranscriptSelection.Run: String] = [:]
    /// A drag is selecting: a view losing the keyboard to it keeps the
    /// selection.
    private(set) var isTracking = false
    /// Set while this class changes a view's selection, so the view's
    /// notice of it is not taken for the reader's.
    private var applying = false

    func register(_ view: ProseNSTextView) {
        views.add(view)
        remember(view)
    }

    private func remember(_ view: ProseNSTextView) {
        guard let run = view.run else { return }
        texts[run] = view.string
        // Kept small: the runs of a long chat come and go.
        if texts.count > 4_000, let bounds = selection.bounds {
            texts = texts.filter { $0.key >= bounds.start.run && $0.key <= bounds.end.run }
        } else if texts.count > 4_000 {
            texts.removeAll()
        }
    }

    func unregister(_ view: ProseNSTextView) {
        views.remove(view)
    }

    /// Changes a view's text without that dropping the selection, then shows
    /// the part of the selection that falls in its run.
    func replacing(_ view: ProseNSTextView, _ change: () -> Void) {
        applying = true
        defer { applying = false }
        change()
        remember(view)
        restore(view)
    }

    /// A press in `view` and the drag after it: the selection runs from the
    /// press to wherever the pointer is, across every run in between, and
    /// the chat scrolls when the pointer holds near its edge.
    func track(from view: ProseNSTextView, press: NSEvent) {
        guard let window = view.window, let run = view.run else { return }
        isTracking = true
        defer { isTracking = false }
        window.makeFirstResponder(view)
        clear()
        selection.begin(at: .init(run, offset: view.offset(at: press.locationInWindow)))
        var last = press
        NSEvent.startPeriodicEvents(afterDelay: 0.15, withPeriod: 0.05)
        defer { NSEvent.stopPeriodicEvents() }
        while let event = window.nextEvent(matching: [.leftMouseDragged, .leftMouseUp, .periodic]) {
            if event.type == .leftMouseUp { break }
            if event.type == .leftMouseDragged { last = event }
            // Held near an edge, the periodic events keep the chat moving.
            view.autoscroll(with: last)
            if let position = position(at: last.locationInWindow, in: window) {
                selection.extend(to: position)
                showAll()
            }
        }
        if selection.isEmpty { selection.clear() }
    }

    /// The place in the chat's prose a point of the window is at: in the run
    /// under it, or, between runs, the end of the run above or the start of
    /// the run below, whichever is nearer.
    private func position(at point: NSPoint, in window: NSWindow) -> TranscriptSelection.Position? {
        var best: (distance: CGFloat, position: TranscriptSelection.Position)?
        for view in views.allObjects where view.window === window {
            guard let run = view.run else { continue }
            let frame = view.convert(view.bounds, to: nil)
            let length = (view.string as NSString).length
            let position: TranscriptSelection.Position
            let distance: CGFloat
            if point.y > frame.maxY {
                // Above the run (window points grow upward): its start.
                position = .init(run, offset: 0)
                distance = point.y - frame.maxY
            } else if point.y < frame.minY {
                position = .init(run, offset: length)
                distance = frame.minY - point.y
            } else {
                return .init(run, offset: view.offset(at: point))
            }
            if best.map({ distance < $0.distance }) ?? true { best = (distance, position) }
        }
        return best?.position
    }

    /// Every drawn run shows its part of the selection.
    private func showAll() {
        applying = true
        defer { applying = false }
        for view in views.allObjects {
            let length = (view.string as NSString).length
            if let run = view.run, let range = selection.range(in: run, length: length) {
                let selected = NSRange(location: range.lowerBound, length: range.count)
                if view.selectedRange() != selected { view.setSelectedRange(selected) }
            } else if view.selectedRange().length > 0 {
                view.setSelectedRange(NSRange(location: view.selectedRange().location, length: 0))
            }
        }
    }

    /// The text of a selection that spans runs, or nil for one inside a
    /// single run, which its text view copies itself.
    func textAcrossRuns() -> String? {
        guard let bounds = selection.bounds, bounds.start.run != bounds.end.run else { return nil }
        for view in views.allObjects { remember(view) }
        let runs = texts.filter { selection.contains($0.key) }.map { (run: $0.key, text: $0.value) }
        return selection.text(of: runs)
    }

    private func restore(_ view: ProseNSTextView) {
        guard let run = view.run,
              let range = selection.range(in: run, length: (view.string as NSString).length) else { return }
        view.setSelectedRange(NSRange(location: range.lowerBound, length: range.count))
    }

    /// Ends the selection wherever it shows. Returns whether there was one.
    @discardableResult
    func clear() -> Bool {
        var had = !selection.isEmpty
        selection.clear()
        applying = true
        defer { applying = false }
        for view in views.allObjects where view.selectedRange().length > 0 {
            had = true
            view.setSelectedRange(NSRange(location: view.selectedRange().location, length: 0))
        }
        return had
    }

    /// Whether `view` holds a selection the reader made.
    func holds(_ view: NSResponder?) -> Bool {
        guard let view = view as? ProseNSTextView else { return false }
        return view.selectedRange().length > 0
    }

    nonisolated func textViewDidChangeSelection(_ notification: Notification) {
        MainActor.assumeIsolated {
            guard let view = notification.object as? ProseNSTextView else { return }
            changed(view)
        }
    }

    /// The reader selected in `view`: that is now the selection, and any
    /// other run's highlight goes.
    private func changed(_ view: ProseNSTextView) {
        guard !applying else { return }
        let range = view.selectedRange()
        guard range.length > 0 else {
            if let run = view.run, selection.bounds?.start.run == run { selection.clear() }
            return
        }
        applying = true
        defer { applying = false }
        for other in views.allObjects where other !== view && other.selectedRange().length > 0 {
            other.setSelectedRange(NSRange(location: other.selectedRange().location, length: 0))
        }
        if let run = view.run {
            selection.select(from: .init(run, offset: range.location), to: .init(run, offset: NSMaxRange(range)))
        } else {
            selection.clear()
        }
    }
}
