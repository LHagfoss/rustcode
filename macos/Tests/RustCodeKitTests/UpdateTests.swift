import XCTest
@testable import RustCodeKit

final class UpdateTests: XCTestCase {
    func testVersionsCompareNumerically() {
        XCTAssertLessThan(AppVersion("0.9.0")!, AppVersion("v0.10.0")!)
        XCTAssertEqual(AppVersion("1.2")!, AppVersion("1.2.0")!)
        XCTAssertNil(AppVersion("development"))
    }

    func testReleaseNeedsTheMacAppAsset() throws {
        let json = """
        {"tag_name":"v0.70.0","body":"Notes","html_url":"https://github.com/LHagfoss/rustcode/releases/tag/v0.70.0",
         "assets":[{"name":"rustcode-macos-aarch64-swift-app.zip","browser_download_url":"https://example.com/app.zip"},
                   {"name":"SHA256SUMS","browser_download_url":"https://example.com/sums"}]}
        """
        let release = try XCTUnwrap(AppRelease(githubJSON: Data(json.utf8)))
        XCTAssertEqual(release.version, AppVersion("0.70.0"))
        XCTAssertEqual(release.checksumsURL?.absoluteString, "https://example.com/sums")
        XCTAssertNil(AppRelease(githubJSON: Data(#"{"tag_name":"v1.0.0","assets":[]}"#.utf8)))
    }

    func testChecksumIsFoundByAssetName() {
        let sums = "abc123  rustcode-linux-x86_64.tar.gz\nDEF456 *artifacts/rustcode-macos-aarch64-swift-app.zip\n"
        XCTAssertEqual(AppRelease.checksum(for: AppRelease.appAssetName, in: sums), "def456")
        XCTAssertNil(AppRelease.checksum(for: "missing.zip", in: sums))
    }
}
