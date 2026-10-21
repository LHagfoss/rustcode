import XCTest
@testable import RustCodeKit

final class RemoteShareTests: XCTestCase {
    func testParsesASharedSessionWithAnOffer() {
        let status = RemoteShareStatus(result: [
            "available": true,
            "enabled": true,
            "shared": true,
            "sessionId": "session-1",
            "registrationEpoch": 42,
            "gateway": [
                "connected": true,
                "advertisedAddress": "192.168.1.4:8787",
                "loopbackOnly": false,
                "attachedDevices": ["phone"],
                "connectedDevices": ["phone"],
            ],
            "offer": [
                "qrPayload": "rustcode://pair?code=1234",
                "code": "1234-5678",
                "address": "192.168.1.4:8787",
                "expiresInSecs": 240,
                "loopbackOnly": false,
                "gatewayId": "gateway-1",
                "challenge": 7,
            ],
            "pairingPending": false,
            "pairingExpired": false,
            "pairedDevices": 1,
            "devices": [["id": "device-1", "name": "Phone"]],
        ])

        XCTAssertTrue(status.available)
        XCTAssertTrue(status.enabled)
        XCTAssertTrue(status.shared)
        XCTAssertEqual(status.sessionId, "session-1")
        XCTAssertEqual(status.registrationEpoch, 42)
        XCTAssertEqual(status.gateway?.connected, true)
        XCTAssertEqual(status.offer?.code, "1234-5678")
        XCTAssertEqual(status.offer?.expiresInSecs, 240)
        XCTAssertEqual(status.address, "192.168.1.4:8787")
        XCTAssertEqual(status.connectedDevices, ["phone"])
        XCTAssertEqual(status.devices, [RemoteShareStatus.Device(id: "device-1", name: "Phone")])
        XCTAssertEqual(status.pairedDevices, 1)
    }

    func testIdleSessionHasNoOfferOrGateway() {
        let status = RemoteShareStatus(result: [
            "available": true,
            "enabled": false,
            "shared": false,
            "sessionId": "session-1",
            "pairedDevices": 0,
            "devices": [],
        ])

        XCTAssertFalse(status.enabled)
        XCTAssertFalse(status.shared)
        XCTAssertNil(status.gateway)
        XCTAssertNil(status.offer)
        XCTAssertNil(status.address)
        XCTAssertFalse(status.pairingPending)
        XCTAssertFalse(status.pairingExpired)
        XCTAssertEqual(status.pairedDevices, 0)
    }

    func testExpiredOfferIsKeptForTheExpiryNoticeButNotAsAUsableCode() {
        let status = RemoteShareStatus(result: [
            "shared": true,
            "pairingExpired": true,
            "offer": [
                "code": "1234-5678",
                "address": "192.168.1.4:8787",
                "expiresInSecs": 0,
            ],
        ])

        XCTAssertTrue(status.pairingExpired)
        XCTAssertEqual(status.offer?.code, "1234-5678")
        XCTAssertEqual(status.offer?.expiresInSecs, 0)
    }

    func testErrorStatusDoesNotPretendToBeShared() {
        let status = RemoteShareStatus(error: "gateway unavailable")
        XCTAssertFalse(status.shared)
        XCTAssertFalse(status.enabled)
        XCTAssertEqual(status.error, "gateway unavailable")
        XCTAssertNil(status.offer)
    }
}
