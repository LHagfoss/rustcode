import Foundation

/// The host's logical-turn clock. Keep timestamps and unknown outcomes intact;
/// work time is authoritative and excludes waits and gaps between runs.
public struct TurnTiming: Hashable, Sendable {
    public var turnID: String
    public var startedAt: String?
    public var endedAt: String?
    public var elapsedWorkMilliseconds: UInt64?
    public var outcome: String?
    public var metadata: [String: JSONValue]

    public init(turnID: String, startedAt: String? = nil, endedAt: String? = nil,
                elapsedWorkMilliseconds: UInt64? = nil, outcome: String? = nil,
                metadata: [String: JSONValue] = [:]) {
        self.turnID = turnID
        self.startedAt = startedAt
        self.endedAt = endedAt
        self.elapsedWorkMilliseconds = elapsedWorkMilliseconds
        self.outcome = outcome
        self.metadata = metadata
    }
}
