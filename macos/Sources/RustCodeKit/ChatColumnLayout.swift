import Foundation

/// How the chat's column, its padding and the timeline rail fit the width
/// the chat has (#2552). The transcript, the composer and the panels over it
/// share the column; the rail sits in the gutter left of it.
///
/// - Wide (`width >= 1072`): the column at its 980 pt cap, 28 pt padding,
///   and the full rail in the wide gutter.
/// - Medium (`600..<1072`): the column fills the width inside 28 pt padding,
///   and a compact rail fits in that padding instead of disappearing (it
///   used to go below 848 pt and, sized for a 760 pt column, crossed the text
///   between 848 and 1036).
/// - Narrow (`360..<600`): 20 pt padding, so the text and the composer keep
///   their room, a narrower rail in it, and a compact composer.
/// - Below 360: 16 pt padding and no rail; Page Up/Down, Home/End and the
///   sticky turn bar still move between turns.
public struct ChatColumnLayout: Equatable, Sendable {
    public enum Size: Equatable, Sendable { case narrow, medium, wide }

    /// Where the rail sits, from the chat's leading edge, and how wide it is.
    public struct Rail: Equatable, Sendable {
        public var x: Double
        public var width: Double
        /// The longest tick, the current turn's.
        public var tick: Double { width - 6 }
    }

    public static let maxColumn: Double = 980
    public static let wideWidth: Double = 1072
    public static let mediumWidth: Double = 600
    public static let railWidth: Double = 360

    public var size: Size
    /// The room either side of the column at its narrowest.
    public var padding: Double
    public var column: Double
    /// The timeline rail, when it fits; drawn only for two turns or more.
    public var rail: Rail?

    public init(width: Double) {
        let width = max(width, 0)
        if width >= Self.wideWidth {
            size = .wide
            padding = 28
            column = Self.maxColumn
            let gutter = (width - column) / 2
            rail = Rail(x: min(10, (gutter - 28) / 2), width: 28)
        } else if width >= Self.mediumWidth {
            size = .medium
            padding = 28
            column = width - 2 * padding
            rail = Rail(x: 5, width: 18)
        } else {
            size = .narrow
            padding = width >= Self.railWidth ? 20 : 16
            column = max(width - 2 * padding, 0)
            rail = width >= Self.railWidth ? Rail(x: 3, width: 14) : nil
        }
    }

    /// The composer drops labels it can do without: effort shows only its
    /// icon, and the agents pill only its count.
    public var compactComposer: Bool { size == .narrow }

    /// Whether the rail shows for `turns` turns. One turn has nowhere to go.
    public func showsRail(turns: Int) -> Bool { rail != nil && turns >= 2 }

    /// The column's leading edge from the chat's leading edge.
    public func columnMinX(in width: Double) -> Double { (width - column) / 2 }
}
