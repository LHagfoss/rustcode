import Foundation

/// How the Settings page fits the window (#2593): its section sidebar on the
/// left and the chosen section in a centered column on the right.
///
/// - Roomy (`width >= 880`): the full sidebar of section names.
/// - Compact (below 880, which includes the window's minimum width): the
///   sidebar keeps only the sections' icons, so the section has the room.
///
/// The column is centered in the room right of the sidebar, never wider than
/// `maxColumn`, with margins that grow with the room (5% of it, 24 to 64 pt)
/// so the content neither hugs an edge nor stretches across a wide window.
public struct SettingsLayout: Equatable, Sendable {
    public static let compactWidth: Double = 880
    public static let sidebarWidth: Double = 230
    public static let compactSidebarWidth: Double = 64
    public static let maxColumn: Double = 720

    public var compact: Bool
    public var sidebar: Double
    /// The least room either side of the column.
    public var margin: Double
    public var column: Double

    public init(width: Double) {
        let width = max(width, 0)
        compact = width < Self.compactWidth
        sidebar = compact ? Self.compactSidebarWidth : Self.sidebarWidth
        let room = max(width - sidebar, 0)
        margin = min(max(room * 0.05, 24), 64)
        column = max(min(Self.maxColumn, room - 2 * margin), 0)
    }

    /// The column's leading edge from the content area's leading edge.
    public func columnMinX(in width: Double) -> Double {
        (max(width - sidebar, 0) - column) / 2
    }
}
