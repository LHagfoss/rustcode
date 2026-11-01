import XCTest
@testable import RustCodeKit

final class SmartUITests: XCTestCase {
    private let page = """
        <title>Slowest tests</title>
        <meta name="description" content="Filter by crate">
        <input id="q"><ul id="list"></ul>
        <script>document.getElementById('list').textContent = 'ok';</script>
        """

    private func fenced(_ body: String, info: String = "rustcode-ui v1") -> String {
        "Here you go:\n\n```\(info)\n\(body)\n```\n\nDone."
    }

    private func block(_ text: String) -> SmartUIBlock? {
        for case .smartUI(let block) in MarkdownBlock.parse(text, visualizations: true, smartUI: true) {
            return block
        }
        return nil
    }

    func testAVersionedFenceIsReadyWithItsTitleAndSummary() throws {
        let block = try XCTUnwrap(block(fenced(page)))
        XCTAssertEqual(block.state, .ready(title: "Slowest tests", summary: "Filter by crate"))
        XCTAssertEqual(block.body, page)
        XCTAssertEqual(block.source, "```rustcode-ui v1\n\(page)\n```")
    }

    func testOnlyRunsWhenTheRendererAsksForIt() {
        // User text, tool output and Smart UI turned off parse it as code.
        let blocks = MarkdownBlock.parse(fenced(page), visualizations: true)
        XCTAssertEqual(blocks[1], .code(language: "rustcode-ui v1", text: page))
        // Ordinary HTML fences never run.
        XCTAssertNil(block(fenced(page, info: "html")))
    }

    func testUnversionedFutureOversizedAndEmptyBlocksShowSource() throws {
        func reason(_ text: String) throws -> String {
            let state = try XCTUnwrap(block(text)).state
            guard case .invalid(let reason) = state else { XCTFail("\(state)"); return "" }
            return reason
        }
        XCTAssertTrue(try reason(fenced(page, info: "rustcode-ui")).contains("no version"))
        XCTAssertTrue(try reason(fenced(page, info: "rustcode-ui v2")).contains("unsupported version"))
        let huge = String(repeating: "x", count: SmartUI.maxSourceBytes + 1)
        XCTAssertTrue(try reason(fenced(huge)).contains("larger than"))
        XCTAssertEqual(try reason(fenced("  ")), "empty block")
    }

    func testAnOpenFenceIsIncomplete() throws {
        let partial = "Building:\n\n```rustcode-ui v1\n<title>Half"
        XCTAssertEqual(try XCTUnwrap(block(partial)).state, .incomplete)
    }

    func testMissingTitleFallsBackAndDescriptionIsOptional() throws {
        let block = try XCTUnwrap(block(fenced("<button>Go</button>")))
        XCTAssertEqual(block.state, .ready(title: "Interactive view", summary: nil))
    }

    func testTheDocumentLeadsWithTheContentPolicy() {
        let document = SmartUI.document(for: page)
        let policy = document.range(of: "Content-Security-Policy")
        let fragment = document.range(of: "<title>Slowest tests</title>")
        XCTAssertNotNil(policy)
        XCTAssertLessThan(policy!.lowerBound, fragment!.lowerBound)
        for directive in ["default-src 'none'", "connect-src 'none'", "form-action 'none'", "frame-src 'none'"] {
            XCTAssertTrue(SmartUI.contentSecurityPolicy.contains(directive), directive)
        }
        XCTAssertFalse(SmartUI.contentSecurityPolicy.contains("http"))
    }

    func testInitializeAdvertisesSmartUIVersionZeroWhileOff() {
        let off = ACPRequests.initialize(appVersion: "1", smartUI: false)
        let on = ACPRequests.initialize(appVersion: "1", smartUI: true)
        XCTAssertEqual(off["clientCapabilities"]?["_meta"]?["rustcode"]?["smartUi"], .number(0))
        XCTAssertEqual(on["clientCapabilities"]?["_meta"]?["rustcode"]?["smartUi"], .number(2))
        XCTAssertEqual(on["clientCapabilities"]?["_meta"]?["rustcode"]?["visualizations"],
                       .number(Double(Visualization.capabilityLevel)))
    }

    func testSmartUIIsOnUntilTheUserTurnsItOff() {
        let defaults = UserDefaults.standard
        let was = defaults.object(forKey: SmartUI.enabledKey)
        defer { defaults.set(was, forKey: SmartUI.enabledKey) }
        defaults.removeObject(forKey: SmartUI.enabledKey)
        XCTAssertTrue(SmartUI.isEnabled)
        defaults.set(false, forKey: SmartUI.enabledKey)
        XCTAssertFalse(SmartUI.isEnabled)
        defaults.set(true, forKey: SmartUI.enabledKey)
        XCTAssertTrue(SmartUI.isEnabled)
    }

    // MARK: - smart_ui_preview requests

    func testPreviewRequestReadsHtmlAndSessionAndTheCardTitle() throws {
        let request = try SmartUIPreviewRequest(params: ["sessionId": "s1", "html": .string(page)])
        XCTAssertEqual(request.sessionId, "s1")
        XCTAssertEqual(request.html, page)
        XCTAssertEqual(request.title, "Slowest tests")
        XCTAssertEqual(try SmartUIPreviewRequest(params: ["html": "<p>x</p>"]).title, "Interactive view")
    }

    func testPreviewRequestRejectsMissingEmptyAndOversizedHtml() {
        let oversized = String(repeating: "x", count: SmartUI.maxSourceBytes + 1)
        for params: JSONValue in [[:], ["html": 3], ["html": "  \n"], ["html": .string(oversized)]] {
            XCTAssertThrowsError(try SmartUIPreviewRequest(params: params))
        }
    }

    func testPreviewFailureCarriesTheReasonAndPageErrors() {
        let failure = SmartUIPreviewRequest.failure("busy loop", errors: ["TypeError: x"])
        XCTAssertEqual(failure["ok"], .bool(false))
        XCTAssertEqual(failure["error"]?["message"], .string("busy loop"))
        XCTAssertEqual(failure["errors"], .array([.string("TypeError: x")]))
        XCTAssertNil(SmartUIPreviewRequest.failure("x")["errors"])
    }
}
