import Foundation

/// The prompt turns in flight, one entry per turn, so each turn clears only
/// itself. A turn that ends cannot leave its chat marked running, a second
/// turn in the same chat keeps it running until that one ends too, and a turn
/// from an engine that was reset cannot touch the new engine's turns.
public struct TurnActivity: Equatable, Sendable {
    private var turns: [UUID: String] = [:]

    public init() {}

    /// Chats with at least one turn running.
    public var sessions: Set<String> { Set(turns.values) }

    public func contains(_ session: String) -> Bool { turns.values.contains(session) }

    /// Mark a turn in `session` as started; pass the result to `end`.
    public mutating func begin(_ session: String) -> UUID {
        let turn = UUID()
        turns[turn] = session
        return turn
    }

    /// The turn is over, however it ended. A turn already forgotten by
    /// `reset` is ignored.
    public mutating func end(_ turn: UUID) {
        turns[turn] = nil
    }

    /// The engine is gone, and every turn with it.
    public mutating func reset() {
        turns = [:]
    }
}
