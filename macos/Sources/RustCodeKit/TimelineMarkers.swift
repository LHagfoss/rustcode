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
    /// The tick's width and strength: 1 for the current turn, less with
    /// distance from it.
    public var scale: Double
    public var opacity: Double
    public var isCurrent: Bool
    /// Its message's first line is in view.
    public var isInView: Bool
}

/// What the session timeline draws: a short horizontal tick per turn,
/// stacked close together and centred on the rail. The turn being read is
/// the current one and has the widest tick; ticks narrow and fade with
/// distance from it. With more turns than fit, the rail shows the ones
/// around the current turn.
public struct TimelineLayout: Equatable, Sendable {
    public var markers: [TimelineMarker]
    /// The current turn's id.
    public var current: Int?

    public static let empty = TimelineLayout(markers: [], current: nil)

    /// The gap between ticks.
    public static let step: Double = 8
    /// Between the rail's ends and its outermost ticks.
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
        // As many ticks as fit, around the current one.
        let fits = max(1, Int((visible - 2 * Self.margin) / Self.step) + 1)
        let shown = min(turns.count, fits)
        let first = min(max(currentIndex - shown / 2, 0), turns.count - shown)
        let top = (visible - Double(shown - 1) * Self.step) / 2

        var markers: [TimelineMarker] = []
        for index in first..<(first + shown) {
            let distance = Double(abs(index - currentIndex))
            let y = ys[index] ?? -.infinity
            markers.append(TimelineMarker(
                id: turns[index].id, number: index + 1, title: turns[index].title,
                y: top + Double(index - first) * Self.step,
                // Far ticks stay legible: at least 8 points wide and a
                // third as strong as the current one.
                scale: index == currentIndex ? 1 : max(0.45, 0.65 - 0.05 * distance),
                opacity: index == currentIndex ? 1 : max(0.55, 0.9 - 0.08 * distance),
                isCurrent: index == currentIndex, isInView: y >= 0 && y <= visible))
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
