import Foundation

/// Which part of a streaming paragraph is still fading in (#2181). Each
/// chunk keeps the time it arrived, so text already shown never fades again
/// when later chunks land, and a chunk that finished fading joins the
/// settled text the renderer draws plainly.
public struct StreamingArrivals: Equatable, Sendable {
    public struct Arrival: Equatable, Sendable {
        /// Where the chunk ends, in characters.
        public var end: Int
        public var time: Date

        public init(end: Int, time: Date) {
            self.end = end
            self.time = time
        }
    }

    /// How long a chunk takes to fade in.
    public static let fade: TimeInterval = 0.3

    /// Characters before this are fully shown.
    public private(set) var settled = 0
    /// Chunks still fading, oldest first.
    public private(set) var pending: [Arrival] = []

    public init() {}

    /// A paragraph first drawn mid-stream fades in whole.
    public init(appearingWith count: Int, at time: Date) {
        if count > 0 { pending = [Arrival(end: count, time: time)] }
    }

    public var isAnimating: Bool { !pending.isEmpty }

    /// The paragraph is now `count` characters long.
    public mutating func textChanged(to count: Int, at time: Date) {
        settle(at: time)
        // Styling can shorten the text, as `**bold**` loses its stars.
        if count < (pending.last?.end ?? settled) {
            pending.removeAll { $0.end > count }
            settled = min(settled, count)
        }
        if count > (pending.last?.end ?? settled) { pending.append(Arrival(end: count, time: time)) }
    }

    /// Chunks that finished fading by `time` join the settled text.
    public mutating func settle(at time: Date) {
        while let first = pending.first, time.timeIntervalSince(first.time) >= Self.fade {
            settled = first.end
            pending.removeFirst()
        }
    }

    /// Everything is shown; nothing needs redrawing.
    public mutating func settleAll() {
        if let last = pending.last { settled = last.end }
        pending = []
    }

    /// Opacity and blur for a chunk `elapsed` seconds after it arrived: quick
    /// to show, gentle to settle, and only ever a light blur.
    public static func reveal(elapsed: TimeInterval, blurs: Bool) -> (opacity: Double, blur: Double) {
        let progress = min(max(elapsed / fade, 0), 1)
        let eased = progress * (2 - progress)
        return (eased, blurs ? (1 - eased) * maxBlur : 0)
    }

    /// The blur a chunk starts at, in points: soft, never unreadable.
    public static let maxBlur = 2.0
}
