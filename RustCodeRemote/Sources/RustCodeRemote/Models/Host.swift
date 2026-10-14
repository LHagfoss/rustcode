import Foundation

/// Where a RustCode gateway listens. Parsed from manual entry or supplied by the codec
/// after it decodes a scanned pairing code.
public struct HostAddress: Hashable, Sendable, Codable, CustomStringConvertible {
    public var host: String
    public var port: Int

    public init(host: String, port: Int) {
        self.host = host
        self.port = port
    }

    public var description: String {
        host.contains(":") ? "[\(host)]:\(port)" : "\(host):\(port)"
    }

    public enum ParseError: Error, Equatable, Sendable {
        case empty
        case missingPort
        case invalidPort
        case invalidHost
    }

    /// Accepts `host:port`, `[v6]:port` and the same with a `ws://` prefix.
    /// The port is required: a pairing code alone cannot locate the host.
    public static func parse(_ text: String) throws(ParseError) -> HostAddress {
        var rest = text.trimmingCharacters(in: .whitespacesAndNewlines)
        for scheme in ["ws://", "wss://", "http://", "https://"] where rest.lowercased().hasPrefix(scheme) {
            rest.removeFirst(scheme.count)
        }
        if let slash = rest.firstIndex(of: "/") {
            rest = String(rest[..<slash])
        }
        guard !rest.isEmpty else { throw .empty }

        let host: String
        let portText: String
        if rest.hasPrefix("[") {
            guard let close = rest.firstIndex(of: "]") else { throw .invalidHost }
            host = String(rest[rest.index(after: rest.startIndex)..<close])
            let tail = rest[rest.index(after: close)...]
            guard tail.hasPrefix(":") else { throw .missingPort }
            portText = String(tail.dropFirst())
        } else {
            let parts = rest.split(separator: ":", omittingEmptySubsequences: false)
            guard parts.count == 2 else { throw parts.count == 1 ? .missingPort : .invalidHost }
            host = String(parts[0])
            portText = String(parts[1])
        }

        guard !host.isEmpty, !host.contains(" ") else { throw .invalidHost }
        guard !portText.isEmpty else { throw .missingPort }
        guard let port = Int(portText), (1...65535).contains(port) else { throw .invalidPort }
        return HostAddress(host: host, port: port)
    }

    /// The kind of network the address belongs to, judged from the literal alone.
    public enum Network: Sendable, Equatable {
        case loopback
        /// RFC 1918 or IPv6 unique-local.
        case privateLAN
        case linkLocal
        /// 100.64.0.0/10, the range NetBird assigns to peers, or a NetBird peer DNS name
        /// (`*.netbird.cloud`, `*.netbird.selfhosted`), which only resolves inside the mesh.
        case overlay
        /// `.local` or an unqualified name.
        case localName
        case other
    }

    public var network: Network {
        let lower = host.lowercased()
        if lower == "localhost" || lower == "::1" { return .loopback }
        if let v4 = Self.ipv4Octets(lower) {
            switch (v4[0], v4[1]) {
            case (127, _): return .loopback
            case (10, _), (192, 168): return .privateLAN
            case (172, 16...31): return .privateLAN
            case (169, 254): return .linkLocal
            case (100, let second) where second & 0xC0 == 64: return .overlay
            default: return .other
            }
        }
        if lower.contains(":") {
            if lower.hasPrefix("fe8") || lower.hasPrefix("fe9") || lower.hasPrefix("fea") || lower.hasPrefix("feb") {
                return .linkLocal
            }
            if lower.hasPrefix("fc") || lower.hasPrefix("fd") { return .privateLAN }
            return .other
        }
        if Self.netBirdDomains.contains(where: { lower.hasSuffix("." + $0) }) { return .overlay }
        if lower.hasSuffix(".local") || !lower.contains(".") { return .localName }
        return .other
    }

    /// DNS zones NetBird serves for peers: the cloud default and the self-hosted default.
    static let netBirdDomains = ["netbird.cloud", "netbird.selfhosted"]

    /// Whether the address reaches the Mac over the NetBird mesh.
    public var isNetBird: Bool { network == .overlay }

    /// What protects the traffic on the way to this address. Authentication is not encryption:
    /// only the overlay tunnel encrypts a plain WebSocket.
    public var protection: TransportProtection {
        switch network {
        case .loopback: .loopback
        case .overlay: .tunnelEncrypted
        case .privateLAN, .linkLocal, .localName: .unencryptedLAN
        case .other: .unencryptedOther
        }
    }

    private static func ipv4Octets(_ text: String) -> [Int]? {
        let parts = text.split(separator: ".", omittingEmptySubsequences: false)
        guard parts.count == 4 else { return nil }
        let octets = parts.compactMap { part -> Int? in
            guard !part.isEmpty, part.allSatisfy(\.isNumber), let value = Int(part), value <= 255 else { return nil }
            return value
        }
        return octets.count == 4 ? octets : nil
    }
}

public enum TransportProtection: Sendable, Equatable {
    case loopback
    case tunnelEncrypted
    case unencryptedLAN
    case unencryptedOther

    public var isEncrypted: Bool { self == .tunnelEncrypted || self == .loopback }
}

/// A host this device has paired with. Contains no secret; the device token lives in
/// `StoredCredential` and only ever in the credential store.
public struct PairedHost: Hashable, Sendable, Codable {
    /// Gateway identity reported at pairing time.
    public var gatewayID: String
    public var name: String
    public var address: HostAddress
    public var device: DeviceIdentity
    public var pairedAt: Date
    /// When the host stops accepting this pairing and asks for a code again. Unknown for a
    /// host that does not say.
    public var expiresAt: Date?

    public init(
        gatewayID: String,
        name: String,
        address: HostAddress,
        device: DeviceIdentity,
        pairedAt: Date,
        expiresAt: Date? = nil
    ) {
        self.gatewayID = gatewayID
        self.name = name
        self.address = address
        self.device = device
        self.pairedAt = pairedAt
        self.expiresAt = expiresAt
    }
}

/// The identity the host assigned to this phone. The name is a label, not an identity.
public struct DeviceIdentity: Hashable, Sendable, Codable {
    public var id: String
    public var name: String

    public init(id: String, name: String) {
        self.id = id
        self.name = name
    }
}

/// Who answered the handshake. `gatewayID` is stable for a host; `instanceID` changes every
/// time the gateway starts, which invalidates event cursors from before.
public struct ConnectionIdentity: Hashable, Sendable {
    public var gatewayID: String
    public var instanceID: String
    /// The host's own name, when it says. A label for display, never an identity.
    public var hostName: String?
    /// When the host stops accepting this device's pairing, when it says.
    public var linkExpiresAt: Date?

    public init(gatewayID: String, instanceID: String, hostName: String? = nil, linkExpiresAt: Date? = nil) {
        self.gatewayID = gatewayID
        self.instanceID = instanceID
        self.hostName = hostName
        self.linkExpiresAt = linkExpiresAt
    }
}

/// What a successful pairing hands out. The token is shown exactly once.
public struct PairingGrant: Hashable, Sendable {
    public var identity: ConnectionIdentity
    public var device: DeviceIdentity
    public var deviceToken: String

    public init(identity: ConnectionIdentity, device: DeviceIdentity, deviceToken: String) {
        self.identity = identity
        self.device = device
        self.deviceToken = deviceToken
    }
}

public struct StoredCredential: Hashable, Sendable, Codable {
    public var host: PairedHost
    public var deviceToken: String

    public init(host: PairedHost, deviceToken: String) {
        self.host = host
        self.deviceToken = deviceToken
    }
}

/// What the user supplied to pair. The scanned text is opaque here: only the wire codec
/// knows how a pairing QR is laid out.
public enum PairingRequest: Sendable, Equatable {
    case scanned(String)
    case manual(address: HostAddress, code: String)
}
