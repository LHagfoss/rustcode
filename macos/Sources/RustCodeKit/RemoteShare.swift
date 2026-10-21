import Foundation

/// The engine's answer to `_rustcode/remote`, `.../start`, `.../pair` and
/// `.../stop`: the share's state, its gateway, and the current pairing offer.
public struct RemoteShareStatus: Equatable, Sendable {
    public struct Gateway: Equatable, Sendable {
        public var connected: Bool
        public var advertisedAddress: String
        public var loopbackOnly: Bool
        public var attachedDevices: [String]
        public var connectedDevices: [String]

        init(json: JSONValue) {
            connected = json["connected"]?.boolValue ?? false
            advertisedAddress = json["advertisedAddress"]?.stringValue ?? ""
            loopbackOnly = json["loopbackOnly"]?.boolValue ?? false
            attachedDevices = json["attachedDevices"]?.arrayValue?.compactMap(\.stringValue) ?? []
            connectedDevices = json["connectedDevices"]?.arrayValue?.compactMap(\.stringValue) ?? []
        }
    }

    public struct Offer: Equatable, Sendable {
        public var qrPayload: String
        public var code: String
        public var address: String
        public var expiresInSecs: Int
        public var loopbackOnly: Bool
        public var gatewayId: String
        public var challenge: Int?
        public var lockedForSecs: Int?

        init(json: JSONValue) {
            qrPayload = json["qrPayload"]?.stringValue ?? ""
            code = json["code"]?.stringValue ?? ""
            address = json["address"]?.stringValue ?? ""
            expiresInSecs = json["expiresInSecs"]?.intValue ?? 0
            loopbackOnly = json["loopbackOnly"]?.boolValue ?? false
            gatewayId = json["gatewayId"]?.stringValue ?? ""
            challenge = json["challenge"]?.intValue
            lockedForSecs = json["lockedForSecs"]?.intValue
        }
    }

    public struct Device: Identifiable, Equatable, Sendable {
        public var id: String
        public var name: String
    }

    public var available: Bool
    public var enabled: Bool
    public var shared: Bool
    public var sessionId: String?
    public var registrationEpoch: UInt64?
    public var gateway: Gateway?
    public var offer: Offer?
    public var pairingPending: Bool
    public var pairingExpired: Bool
    public var pairingError: String?
    public var error: String?
    public var pairedDevices: Int
    public var devices: [Device]

    /// The address a device types beside the manual code.
    public var address: String? {
        let value = offer?.address ?? gateway?.advertisedAddress
        return value?.isEmpty == false ? value : nil
    }

    public var loopbackOnly: Bool {
        offer?.loopbackOnly ?? gateway?.loopbackOnly ?? false
    }

    public var connectedDevices: [String] { gateway?.connectedDevices ?? [] }
    public var attachedDevices: [String] { gateway?.attachedDevices ?? [] }

    public init(result: JSONValue) {
        available = result["available"]?.boolValue ?? true
        enabled = result["enabled"]?.boolValue ?? false
        shared = result["shared"]?.boolValue ?? false
        sessionId = result["sessionId"]?.stringValue
        registrationEpoch = result["registrationEpoch"]?.intValue.map(UInt64.init)
        gateway = result["gateway"].flatMap { $0 == .null ? nil : Gateway(json: $0) }
        offer = result["offer"].flatMap { $0 == .null ? nil : Offer(json: $0) }
        pairingPending = result["pairingPending"]?.boolValue ?? false
        pairingExpired = result["pairingExpired"]?.boolValue ?? false
        pairingError = result["pairingError"]?.stringValue
        error = result["error"]?.stringValue
        pairedDevices = result["pairedDevices"]?.intValue ?? 0
        devices = result["devices"]?.arrayValue?.compactMap { value in
            guard let id = value["id"]?.stringValue else { return nil }
            return Device(id: id, name: value["name"]?.stringValue ?? "Device")
        } ?? []
    }

    public init(error: String) {
        available = true
        enabled = false
        shared = false
        sessionId = nil
        registrationEpoch = nil
        gateway = nil
        offer = nil
        pairingPending = false
        pairingExpired = false
        pairingError = nil
        self.error = error
        pairedDevices = 0
        devices = []
    }
}
