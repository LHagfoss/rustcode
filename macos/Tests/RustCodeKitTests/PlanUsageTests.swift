import XCTest
@testable import RustCodeKit

final class PlanUsageTests: XCTestCase {
    func testReadsBothWindowsAndNamesThem() throws {
        let usage = try XCTUnwrap(PlanUsage(json: [
            "source": "codex",
            "primary": ["usedPercent": 42, "windowMinutes": 300, "resetsAt": 1_800_000_000],
            "secondary": ["usedPercent": 1, "windowMinutes": 10080, "resetsAt": nil],
        ]))
        XCTAssertEqual(usage.windows.map(\.title), ["5-hour", "Weekly"])
        XCTAssertEqual(usage.windows.map(\.leftPercent), [58, 99])
        XCTAssertEqual(usage.windows[1].detail(), "99% left")
        XCTAssertTrue(usage.summary().contains("Codex CLI"))
    }

    func testResetSameDayShowsTheTimeAndLaterTheWeekday() throws {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(identifier: "UTC")!
        let locale = Locale(identifier: "en_GB")
        let now = Date(timeIntervalSince1970: 1_800_000_000)  // Fri 15 Jan 2027, 08:00 UTC
        let soon = PlanUsage.Window(usedPercent: 10, minutes: 300, resetsAt: now.addingTimeInterval(3 * 3600))
        XCTAssertEqual(soon.detail(now: now, calendar: calendar, locale: locale), "90% left · resets 11:00")
        let later = PlanUsage.Window(usedPercent: 10, minutes: 10080, resetsAt: now.addingTimeInterval(3 * 86400))
        XCTAssertEqual(later.detail(now: now, calendar: calendar, locale: locale), "90% left · resets Mon 08:00")
    }

    func testNoWindowsIsNoUsage() {
        XCTAssertNil(PlanUsage(json: nil))
        XCTAssertNil(PlanUsage(json: .null))
        XCTAssertNil(PlanUsage(json: ["source": "codex", "primary": nil, "secondary": nil]))
    }
}
