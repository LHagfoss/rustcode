import Foundation

/// Version of remote protocol v1 and of the gateway handshake frames this build speaks.
public let remoteProtocolVersion: UInt32 = 1

/// Any JSON value. Used to carry a frame, operation, result or event that this version does
/// not know, so that it decodes, can be inspected and encodes back unchanged.
public enum JSONValue: Codable, Equatable, Sendable {
    case null
    case bool(Bool)
    case number(Double)
    case string(String)
    case array([JSONValue])
    case object([String: JSONValue])

    public init(from decoder: any Decoder) throws {
        let container = try decoder.singleValueContainer()
        if container.decodeNil() {
            self = .null
        } else if let value = try? container.decode(Bool.self) {
            self = .bool(value)
        } else if let value = try? container.decode(Double.self) {
            self = .number(value)
        } else if let value = try? container.decode(String.self) {
            self = .string(value)
        } else if let value = try? container.decode([JSONValue].self) {
            self = .array(value)
        } else {
            self = .object(try container.decode([String: JSONValue].self))
        }
    }

    public func encode(to encoder: any Encoder) throws {
        var container = encoder.singleValueContainer()
        switch self {
        case .null: try container.encodeNil()
        case .bool(let value): try container.encode(value)
        case .number(let value):
            if let integer = Int64(exactly: value) {
                try container.encode(integer)
            } else {
                try container.encode(value)
            }
        case .string(let value): try container.encode(value)
        case .array(let value): try container.encode(value)
        case .object(let value): try container.encode(value)
        }
    }
}

/// A string enum that may grow on the host. An unknown value still decodes and encodes back
/// as it came; `known` is nil for it.
public struct WireOpen<Known: RawRepresentable & Sendable & Equatable>: Codable, Equatable, Sendable
where Known.RawValue == String {
    public var rawValue: String

    public init(_ known: Known) {
        rawValue = known.rawValue
    }

    public init(rawValue: String) {
        self.rawValue = rawValue
    }

    public var known: Known? { Known(rawValue: rawValue) }

    public init(from decoder: any Decoder) throws {
        rawValue = try decoder.singleValueContainer().decode(String.self)
    }

    public func encode(to encoder: any Encoder) throws {
        var container = encoder.singleValueContainer()
        try container.encode(rawValue)
    }
}

/// Helpers for flat objects tagged by one string field (`type` or `kind`).
enum WireTag {
    struct Key: CodingKey {
        var stringValue: String
        var intValue: Int? { nil }

        init(_ name: String) {
            stringValue = name
        }

        init?(stringValue: String) {
            self.stringValue = stringValue
        }

        init?(intValue: Int) {
            nil
        }
    }

    static func read(_ field: String, from decoder: any Decoder) throws -> String {
        try decoder.container(keyedBy: Key.self).decode(String.self, forKey: Key(field))
    }

    /// Writes the tag, then the payload's own fields into the same object.
    static func write(_ tag: String, as field: String, payload: (any Encodable)?, to encoder: any Encoder) throws {
        var container = encoder.container(keyedBy: Key.self)
        try container.encode(tag, forKey: Key(field))
        try payload?.encode(to: encoder)
    }
}
