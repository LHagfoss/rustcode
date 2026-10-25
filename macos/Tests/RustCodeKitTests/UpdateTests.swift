import XCTest
@testable import RustCodeKit

final class UpdateTests: XCTestCase {
    func testVersionsCompareNumerically() {
        XCTAssertLessThan(AppVersion("0.9.0")!, AppVersion("v0.10.0")!)
        XCTAssertEqual(AppVersion("1.2")!, AppVersion("1.2.0")!)
        XCTAssertNil(AppVersion("development"))
    }

    func testTheLatestReleasePageNamesItsAssets() throws {
        let page = try XCTUnwrap(URL(string: "https://github.com/LHagfoss/rustcode/releases/tag/v0.95.0"))
        let release = try XCTUnwrap(AppRelease(releasePage: page))
        XCTAssertEqual(release.version, AppVersion("0.95.0"))
        XCTAssertEqual(release.appURL.absoluteString,
                       "https://github.com/LHagfoss/rustcode/releases/download/v0.95.0/rustcode-macos-aarch64-swift-app.zip")
        XCTAssertEqual(release.checksumsURL?.absoluteString,
                       "https://github.com/LHagfoss/rustcode/releases/download/v0.95.0/SHA256SUMS")
        XCTAssertNil(AppRelease(releasePage: AppRelease.latestURL), "a page that did not redirect")
    }

    func testChecksumIsFoundByAssetName() {
        let sums = "abc123  rustcode-linux-x86_64.tar.gz\nDEF456 *artifacts/rustcode-macos-aarch64-swift-app.zip\n"
        XCTAssertEqual(AppRelease.checksum(for: AppRelease.appAssetName, in: sums), "def456")
        XCTAssertNil(AppRelease.checksum(for: "missing.zip", in: sums))
    }
}
