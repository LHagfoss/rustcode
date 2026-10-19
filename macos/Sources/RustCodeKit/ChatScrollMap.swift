import Foundation

/// The chat laid out from the heights its rows really had, for the chat's
/// own scroller.
///
/// The chat's older rows are in a lazy stack, which guesses the height of
/// every row it has not laid out and forgets a row's height again once it
/// scrolls away. Its content size, and with it the system scroller, changed
/// on every pass: the knob jumped where the guesses met real rows and grew
/// and shrank while scrolling. This map keeps each row's measured height for
/// as long as the chat is open, so the scroller sizes and places its knob
/// from real heights, and rows not yet seen are guessed from the average of
/// those that were.
public struct ChatScrollMap: Equatable, Sendable {
    /// The gap between rows.
    public var spacing: Double
    /// Above the first row.
    public var topPadding: Double
    /// Below the last row, to the end of the content.
    public var bottomPadding: Double

    public private(set) var rows: [Int] = []
    /// Bumped whenever a row or height changes the layout.
    public private(set) var revision = 0
    private var heights: [Int: Double] = [:]
    /// The top of each row, and the content height last; nil when stale.
    private var tops: [Double]?

    /// A row's guess before any row has been measured.
    public static let defaultEstimate: Double = 60

    public init(spacing: Double, topPadding: Double, bottomPadding: Double) {
        self.spacing = spacing
        self.topPadding = topPadding
        self.bottomPadding = bottomPadding
    }

    /// The rows of the chat, top to bottom.
    public mutating func setRows(_ rows: [Int]) {
        guard rows != self.rows else { return }
        self.rows = rows
        tops = nil
        revision += 1
    }

    /// A row was laid out at this height.
    public mutating func measured(_ row: Int, height: Double) {
        guard height.isFinite, height >= 0 else { return }
        if let known = heights[row], abs(known - height) < 0.5 { return }
        heights[row] = height
        tops = nil
        revision += 1
    }

    public func height(of row: Int) -> Double? { heights[row] }

    /// The guess for a row not yet laid out: the average measured row.
    public var estimate: Double {
        let known = rows.compactMap { heights[$0] }
        return known.isEmpty ? Self.defaultEstimate : known.reduce(0, +) / Double(known.count)
    }

    public mutating func contentHeight() -> Double {
        layout().last ?? topPadding + bottomPadding
    }

    /// Where a row starts, from the top of the content.
    public mutating func top(of row: Int) -> Double? {
        guard let index = rows.firstIndex(of: row) else { return nil }
        return layout()[index]
    }

    /// The row at `y` from the top of the content, and how far into it `y`
    /// is. Above the first row is the first row; below the last, the last.
    public mutating func row(at y: Double) -> (row: Int, offset: Double)? {
        guard !rows.isEmpty else { return nil }
        let tops = layout()
        // The last row starting at or above `y`.
        var low = 0, high = rows.count - 1
        while low < high {
            let middle = (low + high + 1) / 2
            if tops[middle] <= y { low = middle } else { high = middle - 1 }
        }
        return (rows[low], y - tops[low])
    }

    private mutating func layout() -> [Double] {
        if let tops { return tops }
        let guess = estimate
        var result: [Double] = []
        result.reserveCapacity(rows.count + 1)
        var y = topPadding
        for (index, row) in rows.enumerated() {
            result.append(y)
            y += heights[row] ?? guess
            if index < rows.count - 1 { y += spacing }
        }
        result.append(y + bottomPadding)
        tops = result
        return result
    }
}

/// The chat scroller's knob, from where the chat is in its `ChatScrollMap`.
public struct ChatScrollerKnob: Equatable, Sendable {
    /// From the top of the track.
    public var top: Double
    public var height: Double

    public static let minimumHeight: Double = 28

    /// `offset` is how far the top of the visible area is below the top of
    /// the content, `visible` that area's height between the insets.
    public init?(offset: Double, contentHeight: Double, visible: Double, track: Double) {
        let range = contentHeight - visible
        guard visible > 0, track > 0, range > 1 else { return nil }
        let height = min(track, max(Self.minimumHeight, track * visible / contentHeight))
        self.top = (track - height) * min(max(offset / range, 0), 1)
        self.height = height
    }

    /// The offset that puts the knob's top at `top` on the track.
    public static func offset(forKnobTop top: Double, knobHeight: Double, contentHeight: Double,
                              visible: Double, track: Double) -> Double {
        let travel = track - knobHeight
        let fraction = travel > 0 ? min(max(top / travel, 0), 1) : 0
        return fraction * max(contentHeight - visible, 0)
    }

    /// The knob after the chat scrolled `moved` points (positive down) to
    /// `offset`, while the map puts it at `target`.
    ///
    /// A measured row correcting a guessed height moves the map, not the
    /// chat, so the knob does not jump for it: it keeps moving with the
    /// scroll, and its distance from `target` shrinks as the chat scrolls,
    /// by the share of a page scrolled and by the share of the way to the
    /// end it heads for, so it is at `target` again within about a page and
    /// always by that end. It never moves against the scroll.
    public func following(_ target: ChatScrollerKnob, moved: Double, offset: Double, contentHeight: Double,
                          visible: Double, track: Double) -> ChatScrollerKnob {
        let range = contentHeight - visible
        let travel = track - target.height
        if offset <= 0.5 || offset >= range - 0.5 || range <= 1 || travel <= 0 { return target }
        let start = min(self.top, travel)
        var error = start + moved * travel / range - target.top
        if moved != 0 {
            error *= max(0, 1 - abs(moved) / visible)
            let before = moved < 0 ? offset - moved : range - offset + moved
            let after = moved < 0 ? offset : range - offset
            if before > 0 { error *= after / before }
        }
        var top = target.top + error
        if moved < 0 { top = min(top, start) } else if moved > 0 { top = max(top, start) } else { top = start }
        return ChatScrollerKnob(top: min(max(top, 0), travel), height: target.height)
    }

    public init(top: Double, height: Double) {
        self.top = top
        self.height = height
    }
}
