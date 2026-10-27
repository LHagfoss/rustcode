import XCTest
import RustCodeKit
@testable import RustCode

@MainActor
final class WhatsNewFlowTests: XCTestCase {
    private var defaults: UserDefaults!
    private let suite = "WhatsNewFlowTests"

    override func setUp() {
        super.setUp()
        UserDefaults().removePersistentDomain(forName: suite)
        defaults = UserDefaults(suiteName: suite)
    }

    override func tearDown() {
        UserDefaults().removePersistentDomain(forName: suite)
        super.tearDown()
    }

    /// Cached notes, so no test reaches GitHub.
    private func cache(_ notes: ReleaseNotes) throws {
        defaults.set(try JSONEncoder().encode(notes), forKey: "whatsNewNotes.\(notes.tag)")
    }

    func testAnUpdateShowsTheCardOnceThenReadMoreOpensTheChangelog() async throws {
        let notes = ReleaseNotes(tag: "v0.2.0", body: "* feat(macos): add a thing by @a in https://github.com/LHagfoss/rustcode/pull/1")
        try cache(notes)
        defaults.set("0.1.0", forKey: WhatsNewGate.announcedKey)

        let launch = WhatsNew(defaults: defaults)
        await launch.checkOnLaunch(current: AppVersion("0.2.0"))
        XCTAssertEqual(launch.card, notes)
        XCTAssertEqual(defaults.string(forKey: WhatsNewGate.announcedKey), "0.2.0")
        launch.readMore()
        XCTAssertNil(launch.card)
        XCTAssertEqual(launch.full, notes)

        let next = WhatsNew(defaults: defaults)
        await next.checkOnLaunch(current: AppVersion("0.2.0"))
        XCTAssertNil(next.card, "shown once per release")
    }

    func testAFreshInstallAndADevelopmentBuildShowNothing() async {
        let fresh = WhatsNew(defaults: defaults)
        await fresh.checkOnLaunch(current: AppVersion("0.2.0"))
        XCTAssertNil(fresh.card)
        XCTAssertEqual(defaults.string(forKey: WhatsNewGate.announcedKey), "0.2.0")

        let development = WhatsNew(defaults: defaults)
        await development.checkOnLaunch(current: nil)
        XCTAssertNil(development.card)
        await development.reopen(current: nil)
        XCTAssertNil(development.full?.version, "a development build reopens to its fallback")
    }

    func testDismissingAndReopeningUseTheSameNotes() async throws {
        let notes = ReleaseNotes(tag: "v0.3.0", body: "")
        try cache(notes)
        defaults.set("0.2.0", forKey: WhatsNewGate.announcedKey)
        let whatsNew = WhatsNew(defaults: defaults)
        await whatsNew.checkOnLaunch(current: AppVersion("0.3.0"))
        whatsNew.dismissCard()
        XCTAssertNil(whatsNew.card)
        XCTAssertNil(whatsNew.full)
        await whatsNew.reopen(current: AppVersion("0.3.0"))
        XCTAssertEqual(whatsNew.full?.pageURL.absoluteString, "https://github.com/LHagfoss/rustcode/releases/tag/v0.3.0")
    }
}
