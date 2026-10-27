import Foundation

/// Plays a streamed reply out at an even pace. Chunks arrive in bursts and
/// pauses; showing each as it lands makes the text jump. The pacer holds a
/// short buffer and writes at about the rate text arrives, a little faster
/// while it is behind, so the reply reads as one steady write. Once the
/// stream ends, what is left plays out quickly instead of landing at once.
public struct StreamPacer: Equatable, Sendable {
    /// How far behind the stream the shown text aims to stay, in seconds.
    public static let lag: TimeInterval = 0.25
    /// Once the stream ended, the rest is shown within about this long.
    public static let drain: TimeInterval = 0.3
    /// Never slower than this, in characters per second.
    public static let minimumRate: Double = 40
    /// More than this many characters behind, and the text is shown at once:
    /// a pasted block or a reloaded reply is not worth watching typed out.
    public static let maximumBacklog = 4_000

    /// Characters shown so far, fractional so slow rates still advance.
    public private(set) var shown: Double
    /// The arrival rate, smoothed, in characters per second.
    public private(set) var rate: Double = 0
    private var target: Int
    private var lastArrival: Date?

    /// Text already there when the reply comes into view shows at once.
    public init(showing count: Int) {
        shown = Double(count)
        target = count
    }

    /// The whole text is shown.
    public var isCaughtUp: Bool { Int(shown) >= target }

    /// The stream is now `count` characters long.
    public mutating func received(_ count: Int, at time: Date) {
        if count < Int(shown) { shown = Double(count) }
        if let lastArrival, count > target {
            let gap = max(time.timeIntervalSince(lastArrival), 1.0 / 120)
            let instant = Double(count - target) / gap
            // Smoothed, so one burst after a pause does not set the pace.
            rate = rate == 0 ? instant : rate * 0.8 + instant * 0.2
        }
        if count != target { lastArrival = time }
        target = count
        if target - Int(shown) > Self.maximumBacklog { shown = Double(target - Self.maximumBacklog / 4) }
    }

    /// Advance by `elapsed` seconds. `streaming` is false once the stream
    /// ended, when the rest drains fast. Returns the characters to show.
    @discardableResult
    public mutating func advance(by elapsed: TimeInterval, streaming: Bool) -> Int {
        let backlog = Double(target) - shown
        guard backlog > 0 else { return target }
        let speed: Double
        if streaming {
            // At the arrival rate when `lag` behind; up to three times it
            // while further behind, and half of it when close, so a pause in
            // the stream slows the write instead of stopping it dead.
            let expected = rate * Self.lag
            speed = expected > 0 ? rate * min(max(backlog / expected, 0.5), 3) : backlog / Self.lag
        } else {
            speed = max(rate, backlog / Self.drain)
        }
        shown = min(shown + max(speed, Self.minimumRate) * elapsed, Double(target))
        return Int(shown)
    }
}

extension String {
    /// The first `count` characters, ending on a whole word when the cut
    /// would split one, so a word appears in one piece. A cut is never moved
    /// more than a few characters.
    public func pacedPrefix(_ count: Int) -> Substring {
        guard count < self.count else { return self[...] }
        var end = index(startIndex, offsetBy: max(count, 0))
        var stepped = 0
        while end < endIndex, !self[end].isWhitespace, stepped < 12 {
            end = index(after: end)
            stepped += 1
        }
        return self[..<end]
    }
}
