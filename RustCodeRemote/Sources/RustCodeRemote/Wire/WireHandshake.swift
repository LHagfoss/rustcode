import Foundation

// DTOs for `handshake-request.schema.json`, `handshake-response.schema.json` and
// `pairing-qr.schema.json`. Handshake frames are tagged by `type` and carry no `kind`, which
// is how they are told apart from protocol frames. After the handshake a frame of this
// family can only be an `error`, and the gateway closes the connection behind it.

public enum WireHandshakeRequest: Codable, Equatable, Sendable {
    case pair(Pair)
    case authenticate(Authenticate)

    public struct Pair: Codable, Equatable, Sendable {
        public var protocolVersion: UInt32
        public var method: WireOpen<WirePairingMethod>
        public var secret: String
        public var deviceName: String

        enum CodingKeys: String, CodingKey {
            case protocolVersion = "protocol_version"
            case method
            case secret
            case deviceName = "device_name"
        }
    }

    public struct Authenticate: Codable, Equatable, Sendable {
        public var protocolVersion: UInt32
        public var deviceID: String
        public var token: String

        enum CodingKeys: String, CodingKey {
            case protocolVersion = "protocol_version"
            case deviceID = "device_id"
            case token
        }
    }

    public init(from decoder: any Decoder) throws {
        let type = try WireTag.read("type", from: decoder)
        switch type {
        case "pair": self = .pair(try Pair(from: decoder))
        case "authenticate": self = .authenticate(try Authenticate(from: decoder))
        default:
            throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "unknown handshake request"))
        }
    }

    public func encode(to encoder: any Encoder) throws {
        switch self {
        case .pair(let payload): try WireTag.write("pair", as: "type", payload: payload, to: encoder)
        case .authenticate(let payload): try WireTag.write("authenticate", as: "type", payload: payload, to: encoder)
        }
    }
}

public enum WirePairingMethod: String, Sendable, Equatable, CaseIterable {
    case credential
    case code
}

/// Frames the gateway sends during the handshake, and the error frame it uses for the rest
/// of the connection.
public enum WireGatewayFrame: Codable, Equatable, Sendable {
    case paired(Paired)
    case authenticated(Authenticated)
    case error(Failure)
    case unknown(type: String, JSONValue)

    public struct Paired: Codable, Equatable, Sendable {
        public var protocolVersion: UInt32
        public var gatewayID: String
        public var instanceID: String
        public var deviceID: String
        public var deviceName: String
        public var token: String
        /// The host's own name, for display.
        public var hostName: String?

        enum CodingKeys: String, CodingKey {
            case protocolVersion = "protocol_version"
            case gatewayID = "gateway_id"
            case instanceID = "instance_id"
            case deviceID = "device_id"
            case deviceName = "device_name"
            case token
            case hostName = "host_name"
        }
    }

    public struct Authenticated: Codable, Equatable, Sendable {
        public var protocolVersion: UInt32
        public var gatewayID: String
        public var instanceID: String
        public var deviceID: String
        public var deviceName: String
        public var hostName: String?

        enum CodingKeys: String, CodingKey {
            case protocolVersion = "protocol_version"
            case gatewayID = "gateway_id"
            case instanceID = "instance_id"
            case deviceID = "device_id"
            case deviceName = "device_name"
            case hostName = "host_name"
        }
    }

    public struct Failure: Codable, Equatable, Sendable {
        public var code: WireOpen<WireGatewayErrorCode>
        public var message: String
        public var retryAfterSeconds: UInt64?

        enum CodingKeys: String, CodingKey {
            case code
            case message
            case retryAfterSeconds = "retry_after_secs"
        }
    }

    public init(from decoder: any Decoder) throws {
        let type = try WireTag.read("type", from: decoder)
        switch type {
        case "paired": self = .paired(try Paired(from: decoder))
        case "authenticated": self = .authenticated(try Authenticated(from: decoder))
        case "error": self = .error(try Failure(from: decoder))
        default: self = .unknown(type: type, try JSONValue(from: decoder))
        }
    }

    public func encode(to encoder: any Encoder) throws {
        switch self {
        case .paired(let payload): try WireTag.write("paired", as: "type", payload: payload, to: encoder)
        case .authenticated(let payload): try WireTag.write("authenticated", as: "type", payload: payload, to: encoder)
        case .error(let payload): try WireTag.write("error", as: "type", payload: payload, to: encoder)
        case .unknown(_, let raw): try raw.encode(to: encoder)
        }
    }
}

public enum WireGatewayErrorCode: String, Sendable, Equatable, CaseIterable {
    case invalidFrame = "invalid_frame"
    case frameTooLarge = "frame_too_large"
    case unsupportedVersion = "unsupported_version"
    case handshakeTimeout = "handshake_timeout"
    case pairingFailed = "pairing_failed"
    case rateLimited = "rate_limited"
    case unauthorized
    case busy
    case revoked
    case slowConsumer = "slow_consumer"
    case idleTimeout = "idle_timeout"
    case shuttingDown = "shutting_down"
    case notImplemented = "not_implemented"
    case `internal`
}

/// What a pairing QR code encodes: compact JSON.
public struct WirePairingPayload: Codable, Equatable, Sendable {
    public var protocolVersion: UInt32
    /// `host:port` to dial.
    public var address: String
    public var gatewayID: String
    public var credential: String
    public var hostName: String?

    enum CodingKeys: String, CodingKey {
        case protocolVersion = "protocol_version"
        case address
        case gatewayID = "gateway_id"
        case credential
        case hostName = "host_name"
    }
}
