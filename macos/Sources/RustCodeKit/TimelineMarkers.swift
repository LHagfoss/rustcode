import Foundation

/// A turn the session timeline marks (#2309): one per message the user sent.
public struct TimelineTurn: Equatable, Sendable, Identifiable {
    /// The user message's row.
    public var id: Int
    /// The message's first words, for the marker's tooltip and label.
    public var title: String

    public init(id: Int, title: String) {
        self.id = id
        self.title = title
    }

    /// The user turns of `items`, oldest first.
    public static func turns(in items: [TranscriptItem]) -> [TimelineTurn] {
        items.compactMap { item in
            guard case .user(let text) = item.content else { return nil }
            return TimelineTurn(id: item.id, title: title(ImageMarkers.split(text).text))
        }
    }

    /// The first words of `text`, on one line, cut at a word.
    public static func title(_ text: String, limit: Int = 60) -> String {
        let words = text.split(whereSeparator: \.isWhitespace)
        guard !words.isEmpty else { return "Image" }
        var result = ""
        for word in words {
            let next = result.isEmpty ? String(word) : result + " " + word
            if next.count > limit {
                if result.isEmpty { result = String(next.prefix(limit)) }
                return result + "…"
            }
            result = next
        }
        return result
    }
}

/// A turn's marker on the timeline, where the rail draws it.
public struct TimelineMarker: Equatable, Sendable, Identifiable {
    public var id: Int
    /// The turn's number, from 1.
    public var number: Int
    public var title: String
    /// From the top of the chat's visible area.
    public var y: Double
    /// The marker's size and strength: 1 for the current turn, less with
    /// distance from it.
    public var scale: Double
    public var opacity: Double
    public var isCurrent: Bool
    /// In view, drawn level with its message; else stacked at an edge.
    public var isInView: Bool
}

/// What the session timeline draws: a marker per turn. Turns in view sit
/// level with their message; turns above or below the view stack at the
/// rail's top or bottom, nearest first, so the rail always shows that there
/// is more and which way. The turn being read is the current one; markers
/// shrink and fade with distance from it.
public struct TimelineLayout: Equatable, Sendable {
    public var markers: [TimelineMarker]
    /// The current turn's id.
    public var current: Int?

    public static let empty = TimelineLayout(markers: [], current: nil)

    /// How many turns stack at each edge; the rest are not drawn.
    public static let stackLimit = 5
    /// The gap between stacked markers.
    public static let stackStep: Double = 7
    /// Between the rail's ends and its outermost markers.
    public static let margin: Double = 8
    /// Where in the view the reader is taken to be reading, as a share of
    /// its height: the turn starting above it is the current one.
    public static let readingLine: Double = 0.3
    /// Below a message's top, where its first line is.
    public static let firstLine: Double = 16

    public init(markers: [TimelineMarker], current: Int?) {
        self.markers = markers
        self.current = current
    }

    /// `tops` are the turns' tops in the chat's content, nil for turns not
    /// laid out yet (older than what the chat shows, so above it). The view
    /// shows `visible` points from `offset` down the content. While the chat
    /// `follows` its end, the last turn is the current one.
    public init(turns: [TimelineTurn], tops: [Double?], offset: Double, visible: Double, follows: Bool) {
        precondition(turns.count == tops.count)
        guard !turns.isEmpty, visible > 0 else {
            self = .empty
            return
        }
        // Where each turn's first line is in the view; nil is above it.
        let ys = tops.map { $0.map { $0 + Self.firstLine - offset } }
        let reading = visible * Self.readingLine
        let currentIndex: Int
        if follows {
            currentIndex = turns.count - 1
        } else {
            currentIndex = ys.lastIndex { ($0 ?? -.infinity) <= reading } ?? 0
        }
        // Turns run down the content, so those above the view come first
        // and those below it last.
        let above = ys.prefix { ($0 ?? -.infinity) < 0 }.count
        let below = ys.count - (ys.lastIndex { ($0 ?? -.infinity) <= visible }.map { $0 + 1 } ?? 0)
        let stackedAbove = min(above, Self.stackLimit)
        let stackedBelow = min(below, Self.stackLimit)
        // In-view markers keep clear of the stacks.
        let low = Self.margin + Double(stackedAbove) * Self.stackStep
        let high = max(visible - Self.margin - Double(stackedBelow) * Self.stackStep, low)

        var markers: [TimelineMarker] = []
        for index in (above - stackedAbove)..<(turns.count - below + stackedBelow) {
            let y: Double
            var inView = false
            if index < above {
                // The nearest turn above sits innermost.
                y = Self.margin + Double(stackedAbove - (above - index)) * Self.stackStep
            } else if index >= turns.count - below {
                let rank = index - (turns.count - below)
                y = visible - Self.margin - Double(stackedBelow - 1 - rank) * Self.stackStep
            } else {
                y = min(max(ys[index] ?? 0, low), high)
                inView = true
            }
            let distance = Double(abs(index - currentIndex))
            markers.append(TimelineMarker(
                id: turns[index].id, number: index + 1, title: turns[index].title, y: y,
                scale: index == currentIndex ? 1 : max(0.45, 0.85 - 0.1 * distance),
                opacity: index == currentIndex ? 1 : max(0.25, 0.8 - 0.12 * distance),
                isCurrent: index == currentIndex, isInView: inView))
        }
        self.init(markers: markers, current: turns[currentIndex].id)
    }

    /// Whether the layout differs enough from `other` to draw again.
    public func differs(from other: TimelineLayout) -> Bool {
        guard current == other.current, markers.count == other.markers.count else { return true }
        return zip(markers, other.markers).contains { $0.id != $1.id || abs($0.y - $1.y) >= 0.25 || $0.isInView != $1.isInView }
    }

    /// The width of the gutter beside the chat's column below which the
    /// rail is not drawn.
    public static let minimumGutter: Double = 44

    /// Whether the rail shows beside a chat column of `column` points
    /// centred in `width`, with at least `padding` either side, for `turns`
    /// turns. One turn has nowhere to go.
    public static func shows(width: Double, column: Double, padding: Double, turns: Int) -> Bool {
        turns >= 2 && gutter(width: width, column: column, padding: padding) >= minimumGutter
    }

    /// The empty width left of the chat's centred column.
    public static func gutter(width: Double, column: Double, padding: Double) -> Double {
        max(padding, (width - column) / 2)
    }
}
