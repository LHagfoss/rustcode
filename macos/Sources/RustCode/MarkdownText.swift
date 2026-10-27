import RustCodeKit
import SwiftUI

/// Assistant text. Blocks come from `MarkdownBlock`; inline styling from
/// `AttributedString(markdown:)`. Consecutive prose blocks (paragraphs, headings,
/// list items, quotes) render as one selectable text, so a drag can select across
/// them; code blocks, tables and rules stand on their own.
struct MarkdownText: View {
    let text: String
    /// Draw `rustcode-viz` blocks as widgets. Only assistant replies set it.
    var visualizations = false
    /// The reply is still streaming, so an unclosed visualization shows a placeholder.
    var isStreaming = false
    @Environment(\.findQuery) private var findQuery

    private enum Segment {
        /// A run of prose blocks, already styled.
        case prose(AttributedString)
        case block(MarkdownBlock)
    }

    private static let segmentCache = RenderCache<[Segment]>(countLimit: 2_000)
    private static let inlineCache = RenderCache<AttributedString>(countLimit: 8_000)

    /// Parsed once per text: a message redrawn unchanged reuses its blocks,
    /// and a growing one re-styles only the paragraphs that are new.
    private var segments: [Segment] {
        Self.segmentCache.value(for: (visualizations ? "v\u{0}" : "\u{0}") + text) {
            var segments: [Segment] = []
            var run: [MarkdownBlock] = []
            func flush() {
                if !run.isEmpty { segments.append(.prose(Self.prose(run))) }
                run.removeAll()
            }
            for block in MarkdownBlock.parse(text, visualizations: visualizations) {
                switch block {
                case .paragraph, .heading, .listItem, .quote:
                    run.append(block)
                default:
                    flush()
                    segments.append(.block(block))
                }
            }
            flush()
            return segments
        }
    }

    var body: some View {
        let segments = segments
        VStack(alignment: .leading, spacing: 10) {
            ForEach(Array(segments.enumerated()), id: \.offset) { index, segment in
                switch segment {
                case .prose(let text):
                    if isStreaming, index == segments.count - 1 {
                        StreamingProse(text: text).lineSpacing(2)
                    } else {
                        let shown = FindHighlight.apply(text, query: findQuery)
                        Text(shown).lineSpacing(2).linkCursor(shown, lineSpacing: 2)
                    }
                case .block(let block):
                    view(for: block)
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .textSelection(.enabled)
    }

    /// One attributed string for a run of prose blocks. Blank lines between
    /// blocks are set small, so blocks sit about as far apart as the stack spacing.
    private static func prose(_ blocks: [MarkdownBlock]) -> AttributedString {
        var result = AttributedString()
        var previous: MarkdownBlock?
        for block in blocks {
            if let previous {
                var gap = AttributedString("\n")
                if !(previous.isListItem && block.isListItem) {
                    var spacer = AttributedString("\n")
                    spacer.font = .system(size: 5)
                    gap += spacer
                }
                result += gap
            }
            switch block {
            case .paragraph(let text):
                result += inline(text)
            case .heading(let level, let text):
                var heading = inline(text)
                heading.font = level <= 1 ? .title2.bold() : level == 2 ? .title3.bold() : .headline
                result += heading
            case .listItem(let marker, let text):
                var bullet = AttributedString(marker + " ")
                bullet.foregroundColor = .secondary
                result += bullet + inline(text)
            case .quote(let text):
                var bar = AttributedString("▍ ")
                bar.foregroundColor = Color.secondary.opacity(0.5)
                var quote = inline(text)
                // Links keep their colour, so they read as links here too.
                for run in quote.runs where run.link == nil {
                    quote[run.range].foregroundColor = .secondary
                }
                result += bar + quote
            default:
                break
            }
            previous = block
        }
        return result
    }

    @ViewBuilder
    private func view(for block: MarkdownBlock) -> some View {
        switch block {
        case .code(let language, let text):
            CodeBlock(language: language, text: text)
        case .table(let headers, let rows):
            // Columns share the reply's width and long cells wrap, so a table
            // never scrolls sideways.
            WrappingTable(columns: headers.count) {
                ForEach(Array(headers.enumerated()), id: \.offset) { _, cell in
                    let shown = highlighted(cell)
                    Text(shown).fontWeight(.semibold).linkCursor(shown, fontWeight: .semibold)
                        .tableCell(isHeader: true)
                }
                ForEach(Array(rows.enumerated()), id: \.offset) { _, row in
                    ForEach(Array(row.enumerated()), id: \.offset) { _, cell in
                        let shown = highlighted(cell)
                        Text(shown).linkCursor(shown).tableCell(isHeader: false)
                    }
                }
            }
            .padding(.vertical, 6)
            .background(.fill.quaternary, in: .rect(cornerRadius: 12))
        case .rule:
            Divider().padding(.vertical, 4)
        case .image(let alt, let url):
            InlineImage(url: url, alt: alt)
        case .visualization(let visualization):
            VisualizationView(block: visualization, isStreaming: isStreaming)
        default:
            EmptyView()
        }
    }

    private func highlighted(_ text: String) -> AttributedString {
        FindHighlight.apply(Self.inline(text), query: findQuery)
    }

    static func inline(_ text: String) -> AttributedString {
        inlineCache.value(for: text) {
            let options = AttributedString.MarkdownParsingOptions(interpretedSyntax: .inlineOnlyPreservingWhitespace)
            var styled = (try? AttributedString(markdown: text, options: options)) ?? AttributedString(text)
            ChatLinks.style(&styled)
            return styled
        }
    }
}

/// How links read in the chat: underlined in the brand colour, so they look
/// pressable before they are pressed (#2300). Addresses written out bare
/// become links too; code never does.
enum ChatLinks {
    /// A user's message, as sent, with its addresses linked.
    static func linked(_ text: String) -> AttributedString {
        var string = AttributedString(text)
        // From the end, so shortening an address leaves the earlier offsets right.
        for link in Linkify.links(in: text, skipCodeSpans: true).reversed() {
            shorten(characterRange(link.range, in: string), to: link.url, in: &string)
        }
        underline(&string)
        return string
    }

    /// Link the bare addresses outside code, then underline every link.
    static func style(_ string: inout AttributedString) {
        let plain = String(string.characters)
        for link in Linkify.links(in: plain).reversed() {
            let range = characterRange(link.range, in: string)
            let written = String(string[range].characters)
            // Markdown links bare addresses itself; only an address inside
            // code, or inside a link with a label of its own, stays as written.
            let keeps = string[range].runs.contains { run in
                run.inlinePresentationIntent?.contains(.code) == true
                    || run.link.map { $0 != link.url && $0.absoluteString != written } ?? false
            }
            if !keeps { shorten(range, to: link.url, in: &string) }
        }
        underline(&string)
    }

    /// A bare address reads as its host and path rather than the whole
    /// address (#2460); the link still goes to all of it.
    private static func shorten(_ range: Range<AttributedString.Index>, to url: URL, in string: inout AttributedString) {
        var label = AttributedString(Linkify.label(for: url))
        if let attributes = string[range].runs.first?.attributes { label.mergeAttributes(attributes) }
        label.link = url
        string.replaceSubrange(range, with: label)
    }

    /// Keep link underlines restrained and the same thickness for regular,
    /// bold, and heading text (#2413).
    private static func underline(_ string: inout AttributedString) {
        for run in string.runs where run.link != nil {
            string[run.range].underlineStyle = Text.LineStyle(pattern: .solid, color: Color.brand.opacity(0.45))
            string[run.range].appKit.underlineStyle = .single
            string[run.range].appKit.underlineColor = NSColor(Color.brand).withAlphaComponent(0.45)
            string[run.range].foregroundColor = Color.brand
        }
    }

    private static func characterRange(_ offsets: Range<Int>, in string: AttributedString) -> Range<AttributedString.Index> {
        let lower = string.characters.index(string.startIndex, offsetBy: offsets.lowerBound)
        let upper = string.characters.index(lower, offsetBy: offsets.count)
        return lower..<upper
    }
}

extension View {
    /// The pointing hand, the address on hover and Copy Link over the links of
    /// `text`, this view's own text (#2413). Text with no link is left as is.
    @ViewBuilder
    func linkCursor(_ text: AttributedString, fontWeight: Font.Weight? = nil, lineSpacing: CGFloat = 0) -> some View {
        if #available(macOS 15, *), text.runs.contains(where: { $0.link != nil }) {
            modifier(LinkCursor(text: text, fontWeight: fontWeight, lineSpacing: lineSpacing))
        } else {
            self
        }
    }
}

/// A run of text that is a link to `url`, so the measuring renderer can find it.
private struct LinkRun: TextAttribute {
    let url: URL
}

/// Where the links were laid out, written as the text is drawn. A class, so
/// writing it from the renderer does not redraw the view; the AppKit view
/// over the text reads it.
private final class LinkAreas {
    struct Area: Equatable {
        var rect: CGRect
        var url: URL
    }

    var areas: [Area] = []
    weak var view: LinkHitView?

    func update(_ new: [Area]) {
        guard new != areas else { return }
        areas = new
        DispatchQueue.main.async { [weak self] in self?.view?.areasChanged() }
    }
}

/// SwiftUI does not say where a link run of a `Text` lies, so a copy of the
/// text, laid out the same and drawn by a renderer that draws nothing, keeps
/// the frames of its link runs.
///
/// Selectable text is an AppKit text field SwiftUI lays over the text, and
/// its I-beam cursor rect won over every hover handler that set the hand
/// (#2413). So an AppKit view above it takes the pointer over the links
/// alone: its cursor rects show the hand, it opens the link on a click, and
/// it shows the address on hover and Copy Link on a right-click. Anywhere
/// else it is not there for the pointer, so selection works as before.
@available(macOS 15, *)
private struct LinkCursor: ViewModifier {
    let text: AttributedString
    let fontWeight: Font.Weight?
    let lineSpacing: CGFloat
    @State private var areas = LinkAreas()

    func body(content: Content) -> some View {
        content
            .overlay {
                marked
                    .fontWeight(fontWeight)
                    .lineSpacing(lineSpacing)
                    .textRenderer(LinkFrameRenderer(areas: areas))
                    // Selectable text is drawn by AppKit, which never calls a
                    // renderer: with the chat's selection inherited, no link
                    // was ever measured.
                    .textSelection(.disabled)
                    .allowsHitTesting(false)
                    .accessibilityHidden(true)
            }
            .overlay { LinkHitArea(areas: areas).accessibilityHidden(true) }
    }

    /// The text with its link runs tagged.
    private var marked: Text {
        var result = Text("")
        for run in text.runs {
            let piece = Text(AttributedString(text[run.range]))
            result = result + (run.link.map { piece.customAttribute(LinkRun(url: $0)) } ?? piece)
        }
        return result
    }
}

@available(macOS 15, *)
private struct LinkFrameRenderer: TextRenderer {
    let areas: LinkAreas

    func draw(layout: Text.Layout, in context: inout GraphicsContext) {
        var found: [LinkAreas.Area] = []
        for line in layout {
            for run in line {
                if let link = run[LinkRun.self] { found.append(.init(rect: run.typographicBounds.rect, url: link.url)) }
            }
        }
        areas.update(found)
    }
}

private struct LinkHitArea: NSViewRepresentable {
    let areas: LinkAreas

    func makeNSView(context: Context) -> LinkHitView {
        let view = LinkHitView()
        view.areas = areas
        areas.view = view
        return view
    }

    func updateNSView(_ view: LinkHitView, context: Context) {
        view.areas = areas
        areas.view = view
        view.areasChanged()
    }
}

private final class LinkHitView: NSView, NSViewToolTipOwner {
    var areas: LinkAreas?
    private var pressed: LinkAreas.Area?

    override var isFlipped: Bool { true }

    private func area(at point: NSPoint) -> LinkAreas.Area? {
        areas?.areas.first { $0.rect.insetBy(dx: -1, dy: -1).contains(point) }
    }

    func areasChanged() {
        window?.invalidateCursorRects(for: self)
        removeAllToolTips()
        for area in areas?.areas ?? [] { addToolTip(area.rect, owner: self, userData: nil) }
    }

    // Only the links take the pointer; the text field below gets the rest.
    override func hitTest(_ point: NSPoint) -> NSView? {
        guard let superview, area(at: convert(point, from: superview)) != nil else { return nil }
        return self
    }

    override func resetCursorRects() {
        for area in areas?.areas ?? [] { addCursorRect(area.rect.intersection(bounds), cursor: .pointingHand) }
    }

    func view(_ view: NSView, stringForToolTip tag: NSView.ToolTipTag, point: NSPoint, userData data: UnsafeMutableRawPointer?) -> String {
        area(at: point)?.url.absoluteString ?? ""
    }

    override func mouseDown(with event: NSEvent) {
        pressed = area(at: convert(event.locationInWindow, from: nil))
    }

    // A click opens the link; a press that leaves it opens nothing.
    override func mouseUp(with event: NSEvent) {
        defer { pressed = nil }
        guard let pressed, area(at: convert(event.locationInWindow, from: nil)) == pressed else { return }
        NSWorkspace.shared.open(pressed.url)
    }

    override func menu(for event: NSEvent) -> NSMenu? {
        guard let area = area(at: convert(event.locationInWindow, from: nil)) else { return nil }
        let menu = NSMenu()
        menu.addItem(LinkMenuItem(title: "Open Link") { NSWorkspace.shared.open(area.url) })
        menu.addItem(LinkMenuItem(title: "Copy Link") { Clipboard.copy(area.url.absoluteString) })
        return menu
    }
}

private final class LinkMenuItem: NSMenuItem {
    private let run: () -> Void

    init(title: String, run: @escaping () -> Void) {
        self.run = run
        super.init(title: title, action: #selector(runAction), keyEquivalent: "")
        target = self
    }

    required init(coder: NSCoder) { fatalError("init(coder:) is not used") }

    @objc private func runAction() { run() }
}

struct CodeBlock: View {
    var language: String?
    let text: String

    private static let highlightCache = RenderCache<AttributedString>(countLimit: 1_000)

    /// Highlighted once per language and text, not on every redraw.
    private var highlighted: AttributedString {
        Self.highlightCache.value(for: (language ?? "") + "\u{0}" + text) {
            CodeStyle.attributed(text.components(separatedBy: "\n"),
                                 highlighter: language.flatMap(SyntaxHighlighter.init(language:)))
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text(language ?? "code")
                    .font(.caption.monospaced())
                    .foregroundStyle(.secondary)
                Spacer(minLength: 8)
                CopyButton(help: "Copy code") { Clipboard.copy(text) }
            }
            .padding(.leading, 12)
            .padding(.trailing, 6)
            .padding(.top, 6)
            HorizontalOverflow {
                Text(highlighted)
                    .font(.system(.callout, design: .monospaced))
                    // Every line shows: offered less height, a short block
                    // could cut to its first line and an ellipsis (#2432).
                    .fixedSize(horizontal: false, vertical: true)
                    .textSelection(.enabled)
                    .padding(12)
            }
            .scrollIndicators(.never)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.fill.quaternary, in: .rect(cornerRadius: 12))
        .contextMenu { Button("Copy Code", systemImage: "doc.on.doc") { Clipboard.copy(text) } }
    }
}

/// Copy, then a checkmark for a moment. The button grows to fit "Copied"
/// and shrinks back, cross-fading between the labels.
struct CopyButton: View {
    let help: String
    var height: CGFloat = 26
    /// Just the icon, in a square, for a quiet Copy under every message.
    var iconOnly = false
    let action: () -> Void
    @State private var copiedAt: Date?
    @State private var isHovered = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private var showsCopied: Bool { copiedAt != nil }

    var body: some View {
        Button {
            action()
            withAnimation(reduceMotion ? nil : .easeOut(duration: 0.15)) { copiedAt = Date() }
        } label: {
            Group {
                if showsCopied {
                    Label("Copied", systemImage: "checkmark")
                        .foregroundStyle(.green)
                } else {
                    Label("Copy", systemImage: "doc.on.doc")
                }
            }
            .transition(.opacity)
            .labelStyle(CopyLabelStyle(iconOnly: iconOnly))
            .font(.caption.weight(.medium))
            .foregroundStyle(.secondary)
            .padding(.horizontal, iconOnly ? 0 : 8)
            .frame(width: iconOnly ? height : nil, height: height)
            .background(isHovered ? AnyShapeStyle(.fill.tertiary) : AnyShapeStyle(.clear), in: .rect(cornerRadius: 7))
            .contentShape(.rect(cornerRadius: 7))
        }
        .buttonStyle(QuietPressStyle())
        .onHover { isHovered = $0 }
        .pointingHandCursor()
        .help(help)
        .accessibilityLabel(showsCopied ? "Copied" : help)
        // A second press restarts the moment rather than cutting it short.
        .task(id: copiedAt) {
            guard copiedAt != nil else { return }
            try? await Task.sleep(for: .seconds(1.5))
            guard !Task.isCancelled else { return }
            withAnimation(reduceMotion ? nil : .easeOut(duration: 0.2)) { copiedAt = nil }
        }
    }
}

/// Icon and title with a fixed icon slot, so both copy labels line up.
private struct CopyLabelStyle: LabelStyle {
    var iconOnly = false

    func makeBody(configuration: Configuration) -> some View {
        HStack(spacing: 4) {
            configuration.icon.frame(width: 14)
            if !iconOnly { configuration.title }
        }
    }
}

/// Dims while pressed, without scaling, so a button never nudges its neighbours.
struct QuietPressStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label.opacity(configuration.isPressed ? 0.6 : 1)
    }
}

/// The end of a reply still being written. Text that just arrived fades in
/// instead of landing a chunk at a time, and only this last run of prose
/// redraws while it does. With Reduce Motion it shows at once.
struct StreamingProse: View {
    let text: AttributedString
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    /// Settings → General: new text fades in, or appears at once.
    @AppStorage(StreamingProse.revealKey) private var reveals = true
    /// What is settled and which chunks are still fading in.
    @State private var arrivals = StreamingArrivals()

    static let revealKey = "streamingTextReveal"
    private var settled: Int { arrivals.settled }

    var body: some View {
        let count = text.characters.count
        // Only the paragraph still arriving goes through the fade renderer. It
        // draws its text as one layer, and a layer far taller than the window
        // draws nothing: a long reply streamed this way went blank. Settled
        // paragraphs above are plain text, laid out once rather than per frame.
        let head = StreamingSplit.settledLength(of: text.characters, settled: settled)
        VStack(alignment: .leading, spacing: 0) {
            if head > 0 {
                let settledText = AttributedString(text[text.startIndex..<index(head, count: count)])
                Text(settledText).linkCursor(settledText, lineSpacing: 2)
            }
            Group {
                if #available(macOS 15, *), reveals, !reduceMotion {
                    TimelineView(.animation(minimumInterval: 1.0 / 60, paused: !arrivals.isAnimating)) { context in
                        faded(from: head > 0 ? head + 1 : 0, count: count)
                            .textRenderer(FadeInRenderer(now: context.date))
                            .linkCursor(AttributedString(text[index(head > 0 ? head + 1 : 0, count: count)...]), lineSpacing: 2)
                    }
                } else {
                    let tailText = AttributedString(text[index(head > 0 ? head + 1 : 0, count: count)...])
                    Text(tailText).linkCursor(tailText, lineSpacing: 2)
                }
            }
        }
        .onAppear { arrivals = StreamingArrivals(appearingWith: count, at: Date()) }
        .onChange(of: count) { _, new in arrivals.textChanged(to: new, at: Date()) }
        // Stop redrawing once the last chunk is fully in.
        .task(id: arrivals.pending.last?.time) {
            try? await Task.sleep(for: .seconds(StreamingArrivals.fade))
            guard !Task.isCancelled, arrivals.isAnimating else { return }
            arrivals.settleAll()
        }
    }

    /// The text cut where chunks arrived, each piece tagged with its arrival.
    /// Text past the last known chunk has not been counted yet, so it starts
    /// hidden rather than flashing in for a frame.
    @available(macOS 15, *)
    private func faded(from start: Int, count: Int) -> Text {
        var position = min(max(settled, start), count)
        var result = Text(AttributedString(text[index(start, count: count)..<index(position, count: count)]))
        for piece in arrivals.pending + [.init(end: count, time: .distantFuture)] where piece.end > position {
            let end = min(piece.end, count)
            result = result + Text(AttributedString(text[index(position, count: count)..<index(end, count: count)]))
                .customAttribute(FreshText(arrival: piece.time))
            position = end
        }
        return result
    }
}

private extension StreamingProse {
    /// Counted from the end: new text arrives there, so it is the short walk.
    func index(_ offset: Int, count: Int) -> AttributedString.Index {
        text.characters.index(text.endIndex, offsetBy: min(offset, count) - count)
    }
}

/// When a piece of streamed text arrived.
private struct FreshText: TextAttribute {
    var arrival: Date
}

@available(macOS 15, *)
private struct FadeInRenderer: TextRenderer {
    var now: Date

    func draw(layout: Text.Layout, in context: inout GraphicsContext) {
        for line in layout {
            for run in line {
                guard let fresh = run[FreshText.self] else {
                    context.draw(run)
                    continue
                }
                let reveal = StreamingArrivals.reveal(elapsed: now.timeIntervalSince(fresh.arrival), blurs: true)
                guard reveal.opacity > 0 else { continue }
                var faded = context
                // Fades in from a light blur to sharp; only the newest
                // chunks are drawn this way, never the whole reply.
                faded.opacity = reveal.opacity
                if reveal.blur > 0.05 { faded.addFilter(.blur(radius: reveal.blur)) }
                faded.draw(run)
            }
        }
    }
}

/// The find bar's text, so every message can mark where it matches.
extension EnvironmentValues {
    @Entry var findQuery = ""
}

enum FindHighlight {
    static func apply(_ text: AttributedString, query: String) -> AttributedString {
        let query = query.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !query.isEmpty else { return text }
        var text = text
        var searchStart = text.startIndex
        while searchStart < text.endIndex,
              let range = text[searchStart...].range(of: query, options: [.caseInsensitive, .diacriticInsensitive]) {
            text[range].backgroundColor = Color.attention.opacity(0.35)
            searchStart = range.upperBound
        }
        return text
    }
}

private extension MarkdownBlock {
    var isListItem: Bool {
        if case .listItem = self { return true }
        return false
    }
}

private extension View {
    func tableCell(isHeader: Bool) -> some View {
        fixedSize(horizontal: false, vertical: true)
            .frame(maxWidth: .infinity, alignment: .topLeading)
            .padding(.horizontal, 10)
            .padding(.vertical, 6)
            .overlay(alignment: .bottom) {
                if isHeader { Rectangle().fill(.separator).frame(height: 1) }
            }
    }
}

/// Lays cells out row by row in `columns` columns. Columns get the width their
/// longest cell needs when the table fits (the last one takes what is left); otherwise every column keeps up to
/// `minimumWidth` and the rest of the width goes to the columns that need more,
/// in proportion, and their cells wrap.
struct WrappingTable: Layout {
    let columns: Int
    var minimumWidth: CGFloat = 160

    private func columnWidths(_ cells: Subviews, available: CGFloat?) -> [CGFloat] {
        guard columns > 0 else { return [] }
        var ideal = Array(repeating: CGFloat(0), count: columns)
        for (index, cell) in cells.enumerated() {
            ideal[index % columns] = max(ideal[index % columns], ceil(cell.sizeThatFits(.unspecified).width))
        }
        let total = ideal.reduce(0, +)
        guard let available else { return ideal }
        // A narrow table spans the reply anyway; the last column takes the rest.
        if total <= available {
            ideal[columns - 1] += available - total
            return ideal
        }
        var floor = ideal.map { min($0, minimumWidth) }
        // Too many columns even for that: share the width by need.
        let floorTotal = floor.reduce(0, +)
        if floorTotal > available { floor = floor.map { $0 * available / floorTotal } }
        let spare = max(available - floor.reduce(0, +), 0)
        let wanted = zip(ideal, floor).map { $0 - $1 }
        let wantedTotal = max(wanted.reduce(0, +), 1)
        return zip(floor, wanted).map { $0 + spare * $1 / wantedTotal }
    }

    private func rowHeights(_ cells: Subviews, widths: [CGFloat]) -> [CGFloat] {
        guard columns > 0 else { return [] }
        var heights = Array(repeating: CGFloat(0), count: (cells.count + columns - 1) / columns)
        for (index, cell) in cells.enumerated() {
            let size = cell.sizeThatFits(ProposedViewSize(width: widths[index % columns], height: nil))
            heights[index / columns] = max(heights[index / columns], ceil(size.height))
        }
        return heights
    }

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let widths = columnWidths(subviews, available: proposal.width)
        let width = proposal.width ?? widths.reduce(0, +)
        return CGSize(width: width, height: rowHeights(subviews, widths: widths).reduce(0, +))
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        let widths = columnWidths(subviews, available: bounds.width)
        let heights = rowHeights(subviews, widths: widths)
        var y = bounds.minY
        for row in heights.indices {
            var x = bounds.minX
            for column in 0..<columns {
                let index = row * columns + column
                guard index < subviews.count else { break }
                subviews[index].place(at: CGPoint(x: x, y: y), anchor: .topLeading,
                                      proposal: ProposedViewSize(width: widths[column], height: heights[row]))
                x += widths[column]
            }
            y += heights[row]
        }
    }
}
