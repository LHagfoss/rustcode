import XCTest
@testable import RustCodeKit

final class PresenceTests: XCTestCase {
    func testInitializeAsksForPresence() {
        let params = ACPRequests.initialize(appVersion: "1")
        XCTAssertEqual(params["clientCapabilities"]?["_meta"]?["rustcode"]?["presence"]?.intValue,
                       rustcodePresenceCapabilityVersion)
    }

    func testRequestsCarryTheProject() {
        let project = URL(fileURLWithPath: "/Users/me/code/app")
        XCTAssertEqual(PresenceRequests.status(cwd: project), ["cwd": "/Users/me/code/app"])
        XCTAssertEqual(PresenceRequests.set(enabled: false, cwd: project), ["enabled": false, "cwd": "/Users/me/code/app"])
    }

    func testStatusReadsTheEngineAnswer() {
        XCTAssertEqual(PresenceStatus(result: ["enabled": true, "discordDetected": false]),
                       PresenceStatus(enabled: true, discordDetected: false))
        XCTAssertEqual(PresenceStatus(result: ["enabled": false]), PresenceStatus(enabled: false, discordDetected: false))
        XCTAssertNil(PresenceStatus(result: [:]))
    }
}
