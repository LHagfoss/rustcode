import XCTest
@testable import RustCodeKit

final class WhatsNewTests: XCTestCase {
    func testAnnouncesOncePerReleaseAfterAnUpdate() {
        let v103 = AppVersion("0.103.0")!
        let v104 = AppVersion("0.104.0")!
        XCTAssertEqual(WhatsNewGate.decide(current: v104, announced: "0.103.0"), .announce(v104))
        XCTAssertEqual(WhatsNewGate.decide(current: v104, announced: "0.104.0"), .nothing, "shown once")
        XCTAssertEqual(WhatsNewGate.decide(current: v103, announced: "0.104.0"), .nothing, "a downgrade")
        XCTAssertEqual(WhatsNewGate.decide(current: v104, announced: nil), .record(v104), "a fresh install")
        XCTAssertEqual(WhatsNewGate.decide(current: v104, announced: "garbage"), .record(v104))
        XCTAssertEqual(WhatsNewGate.decide(current: nil, announced: "0.103.0"), .nothing, "a development build")
    }

    func testURLsArePinnedToTheTag() {
        let notes = ReleaseNotes(tag: "v0.104.0", body: "")
        XCTAssertEqual(notes.pageURL.absoluteString, "https://github.com/LHagfoss/rustcode/releases/tag/v0.104.0")
        XCTAssertEqual(ReleaseNotes.apiURL(tag: "v0.104.0").absoluteString,
                       "https://api.github.com/repos/LHagfoss/rustcode/releases/tags/v0.104.0")
        XCTAssertEqual(ReleaseNotes.tag(for: AppVersion("0.104.0")!), "v0.104.0")
    }

    func testReadsOnlyTheExpectedRelease() throws {
        let json = Data(###"{"tag_name":"v0.104.0","body":"## What's Changed\n* feat(macos): add a thing"}"###.utf8)
        let notes = try XCTUnwrap(ReleaseNotes(apiJSON: json, expectedTag: "v0.104.0"))
        XCTAssertEqual(notes.tag, "v0.104.0")
        XCTAssertNil(ReleaseNotes(apiJSON: json, expectedTag: "v0.103.0"))
        XCTAssertNil(ReleaseNotes(apiJSON: Data("not json".utf8), expectedTag: "v0.104.0"))
        let missingBody = Data(#"{"tag_name":"v0.104.0"}"#.utf8)
        XCTAssertEqual(ReleaseNotes(apiJSON: missingBody, expectedTag: "v0.104.0")?.body, "")
    }

    func testHighlightsFromGeneratedNotesSkipHousekeeping() {
        let body = """
        <!-- Release notes generated using configuration in .github/release.yml at v0.103.0 -->

        ## What's Changed
        ### Features
        * feat(macos): archive idle chats and move expired ones to the Trash by @LHagfoss in https://github.com/LHagfoss/rustcode/pull/2493
        ### Fixes
        * fix(engine): count pending background starts as pending, not failed by @LHagfoss in https://github.com/LHagfoss/rustcode/pull/2509
        * feat(macos): show working subagents as nested chats in the sidebar by @LHagfoss in https://github.com/LHagfoss/rustcode/pull/2494
        ### Tests
        * test(engine): add a corpus by @LHagfoss in https://github.com/LHagfoss/rustcode/pull/2499
        * chore(ci): release v0.103.0 by @LHagfoss in https://github.com/LHagfoss/rustcode/pull/2505

        **Full Changelog**: https://github.com/LHagfoss/rustcode/compare/v0.102.0...v0.103.0
        """
        let notes = ReleaseNotes(tag: "v0.103.0", body: body)
        XCTAssertEqual(notes.highlights(), [
            "Archive idle chats and move expired ones to the Trash.",
            "Show working subagents as nested chats in the sidebar.",
            "Count pending background starts as pending, not failed.",
        ])
        XCTAssertEqual(notes.highlights(limit: 1).count, 1)
        XCTAssertFalse(notes.readableBody.contains("<!--"))
        XCTAssertFalse(notes.readableBody.contains("Full Changelog"))
        XCTAssertTrue(notes.readableBody.contains("### Features"))
        XCTAssertFalse(notes.readableBody.contains("What's Changed"))
        XCTAssertTrue(notes.readableBody.contains(
            "- Archive idle chats and move expired ones to the Trash ([#2493](https://github.com/LHagfoss/rustcode/pull/2493))"))
        XCTAssertEqual(ReleaseNotes.readableEntry("Curated line (#12)"), "Curated line (#12)")
    }

    func testHighlightsFromCuratedNotes() {
        let body = """
        - Project memory now matches whole words. (#2513)
        - **Faster** startup for chats with MCP servers (#2515)
        - none (#2510)
        """
        XCTAssertEqual(ReleaseNotes(tag: "v1.0.0", body: body).highlights(), [
            "Project memory now matches whole words.",
            "Faster startup for chats with MCP servers.",
        ])
        XCTAssertEqual(ReleaseNotes(tag: "v1.0.0", body: "").highlights(), [])
    }
}
