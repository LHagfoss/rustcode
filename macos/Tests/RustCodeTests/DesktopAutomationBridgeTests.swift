import XCTest
@testable import RustCode

final class DesktopAutomationBridgeTests: XCTestCase {
    func testSubprocessDeadlineBoundsOpenOutput() {
        let start = Date()
        let result = DesktopAutomationBridge.invoke("/bin/sleep", ["5"], environment: ProcessInfo.processInfo.environment, timeout: 0.2)
        XCTAssertNil(result)
        XCTAssertLessThan(Date().timeIntervalSince(start), 2)
    }

    func testSubprocessReadsOutput() {
        let result = DesktopAutomationBridge.invoke("/bin/echo", ["hello"], environment: ProcessInfo.processInfo.environment, timeout: 2)
        XCTAssertEqual(result.flatMap { String(data: $0, encoding: .utf8) }, "hello\n")
    }
}
