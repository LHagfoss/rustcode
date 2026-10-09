import Foundation

/// Optional timing metadata can grow without losing unrecognised values.
public struct WireTurnTiming: Codable, Equatable, Sendable {
    public var turnID: String
    public var startedAt: String?
    public var endedAt: String?
    public var elapsedWorkMilliseconds: UInt64?
    public var outcome: String?
    public var metadata: [String: JSONValue]

    public init(from decoder: any Decoder) throws {
        var fields = try decoder.singleValueContainer().decode([String: JSONValue].self)
        let container = try decoder.container(keyedBy: WireTag.Key.self)
        turnID = try container.decode(String.self, forKey: .init("turn_id"))
        startedAt = try container.decodeIfPresent(String.self, forKey: .init("started_at"))
        endedAt = try container.decodeIfPresent(String.self, forKey: .init("ended_at"))
        elapsedWorkMilliseconds = try container.decodeIfPresent(UInt64.self, forKey: .init("elapsed_work_ms"))
        outcome = try container.decodeIfPresent(String.self, forKey: .init("outcome"))
        for key in ["turn_id", "started_at", "ended_at", "elapsed_work_ms", "outcome"] { fields.removeValue(forKey: key) }
        metadata = fields
    }

    public func encode(to encoder: any Encoder) throws {
        var container = encoder.container(keyedBy: WireTag.Key.self)
        for (key, value) in metadata { try container.encode(value, forKey: .init(key)) }
        try container.encode(turnID, forKey: .init("turn_id"))
        try container.encodeIfPresent(startedAt, forKey: .init("started_at"))
        try container.encodeIfPresent(endedAt, forKey: .init("ended_at"))
        try container.encodeIfPresent(elapsedWorkMilliseconds, forKey: .init("elapsed_work_ms"))
        try container.encodeIfPresent(outcome, forKey: .init("outcome"))
    }
}
