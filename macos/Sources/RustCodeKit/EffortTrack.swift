import Foundation

/// The Model & Effort picker's effort slider: evenly spaced stops on a
/// track, Off first. Positions are fractions of the knob's travel, 0...1.
public enum EffortTrack {
    /// Where stop `index` of `count` sits.
    public static func fraction(of index: Int, count: Int) -> Double {
        guard count > 1 else { return 0 }
        return Double(min(max(index, 0), count - 1)) / Double(count - 1)
    }

    /// The stop nearest `fraction`, which may lie past either end while a
    /// drag overshoots.
    public static func index(nearest fraction: Double, count: Int) -> Int {
        guard count > 1 else { return 0 }
        let clamped = min(max(fraction, 0), 1)
        return Int((clamped * Double(count - 1)).rounded())
    }

    /// The knob's travel as a fraction for a pointer at `x` on a track
    /// `width` wide whose knob is `knob` wide.
    public static func fraction(atX x: Double, width: Double, knob: Double) -> Double {
        let travel = width - knob
        guard travel > 0 else { return 0 }
        return min(max((x - knob / 2) / travel, 0), 1)
    }
}
