import XCTest
@testable import RustCodeKit

final class LocalPreviewTests: XCTestCase {
    private func normalized(_ input: String) -> String? {
        try? PreviewURL.normalize(input).get().absoluteString
    }

    func testNormalizeFillsInSchemeHostAndPath() {
        XCTAssertEqual(normalized("http://localhost:3000"), "http://localhost:3000/")
        XCTAssertEqual(normalized("  localhost:5173/app?x=1 "), "http://localhost:5173/app?x=1")
        XCTAssertEqual(normalized("3000"), "http://localhost:3000/")
        XCTAssertEqual(normalized(":8080/docs"), "http://localhost:8080/docs")
        XCTAssertEqual(normalized("127.0.0.1:4000"), "http://127.0.0.1:4000/")
        XCTAssertEqual(normalized("HTTPS://localhost:8443/#/route"), "https://localhost:8443/#/route")
        XCTAssertEqual(normalized("app.localhost:3000"), "http://app.localhost:3000/")
    }

    func testNormalizeAcceptsIPv6Loopback() throws {
        let url = try PreviewURL.normalize("[::1]:8080").get()
        XCTAssertTrue(PreviewURL.isLocal(url))
        XCTAssertEqual(PreviewURL.displayHost(url), "[::1]:8080")
    }

    func testNormalizeRejectsEmptyInvalidAndOtherSchemes() {
        XCTAssertEqual(PreviewURL.normalize("   "), .failure(.empty))
        XCTAssertEqual(PreviewURL.normalize("http://"), .failure(.invalid))
        XCTAssertEqual(PreviewURL.normalize("file:///etc/hosts"), .failure(.unsupportedScheme("file")))
        XCTAssertEqual(PreviewURL.normalize("ftp://localhost/"), .failure(.unsupportedScheme("ftp")))
    }

    func testNormalizeRejectsRemoteHostsButOffersTheBrowser() {
        for input in ["example.com", "https://localhost.evil.com", "http://localhost@evil.com", "192.168.1.5:3000", "128.0.0.1"] {
            guard case .failure(.notLocal(let url)) = PreviewURL.normalize(input) else {
                return XCTFail("\(input) should be rejected as not local")
            }
            XCTAssertEqual(PreviewURLError.notLocal(url).browserURL, url)
        }
        XCTAssertNil(PreviewURLError.invalid.browserURL)
    }

    func testIsLocalCoversTheLoopbackRange() throws {
        XCTAssertTrue(PreviewURL.isLocal(try XCTUnwrap(URL(string: "http://127.1.2.3:9000/"))))
        XCTAssertTrue(PreviewURL.isLocal(try XCTUnwrap(URL(string: "http://LOCALHOST/"))))
        XCTAssertFalse(PreviewURL.isLocal(try XCTUnwrap(URL(string: "http://127.0.0.256/"))))
        XCTAssertFalse(PreviewURL.isLocal(try XCTUnwrap(URL(string: "ws://localhost:3000/"))))
        XCTAssertEqual(PreviewURL.displayHost(try XCTUnwrap(URL(string: "http://localhost:3000/a"))), "localhost:3000")
    }

    func testNavigationPolicyKeepsThePageLocal() throws {
        let local = try XCTUnwrap(URL(string: "http://localhost:3000/next"))
        let remote = try XCTUnwrap(URL(string: "https://example.com/docs"))
        XCTAssertEqual(PreviewURL.policy(for: local, isMainFrame: true), .allow)
        XCTAssertEqual(PreviewURL.policy(for: remote, isMainFrame: true), .openExternally)
        XCTAssertEqual(PreviewURL.policy(for: remote, isMainFrame: false), .allow)
        XCTAssertEqual(PreviewURL.policy(for: try XCTUnwrap(URL(string: "about:blank")), isMainFrame: true), .allow)
        XCTAssertEqual(PreviewURL.policy(for: try XCTUnwrap(URL(string: "mailto:a@b.c")), isMainFrame: true), .openExternally)
        XCTAssertEqual(PreviewURL.policy(for: try XCTUnwrap(URL(string: "file:///etc/hosts")), isMainFrame: true), .cancel)
    }

    func testFailureClassification() {
        let refused = PreviewFailure(domain: NSURLErrorDomain, code: URLError.Code.cannotConnectToHost.rawValue,
                                     description: "Could not connect to the server.")
        XCTAssertEqual(refused?.kind, .serverUnavailable)
        XCTAssertEqual(refused?.title(for: URL(string: "http://localhost:3000/")), "Nothing is running at localhost:3000")
        XCTAssertEqual(PreviewFailure(domain: NSURLErrorDomain, code: URLError.Code.timedOut.rawValue, description: "")?.kind, .timedOut)
        XCTAssertNil(PreviewFailure(domain: NSURLErrorDomain, code: URLError.Code.cancelled.rawValue, description: ""))
        XCTAssertNil(PreviewFailure(domain: "WebKitErrorDomain", code: 102, description: ""))
        let other = PreviewFailure(domain: "WebKitErrorDomain", code: 101, description: "Cannot show this URL")
        XCTAssertEqual(other?.kind, .other)
        XCTAssertEqual(other?.detail, "Cannot show this URL")
    }

    func testStateTransitions() throws {
        let home = try XCTUnwrap(URL(string: "http://localhost:3000/"))
        let route = try XCTUnwrap(URL(string: "http://localhost:3000/settings"))
        let failure = PreviewFailure(kind: .serverUnavailable, message: "refused")
        var state = PreviewState()
        XCTAssertEqual(state.phase, .idle)

        state.begin(home)
        XCTAssertTrue(state.isLoading)
        XCTAssertEqual(state.url, home)

        state.fail(failure, url: nil)
        XCTAssertEqual(state.failure, failure)
        XCTAssertEqual(state.url, home)
        // A failed page keeps its URL to retry, whatever the web view reports.
        state.navigated(to: route)
        XCTAssertEqual(state.url, home)

        state.begin(nil)
        XCTAssertNil(state.failure)
        state.finish(home)
        XCTAssertEqual(state.phase, .loaded)
        state.navigated(to: route)
        XCTAssertEqual(state.url, route)

        state.begin(home)
        state.cancel()
        XCTAssertEqual(state.phase, .loaded)

        state.reset()
        XCTAssertEqual(state, PreviewState())
        state.begin(nil)
        state.cancel()
        XCTAssertEqual(state.phase, .idle)
    }

    func testStoreRemembersTheLastLocalURLPerProject() throws {
        let suite = "LocalPreviewTests.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let store = PreviewURLStore(defaults: defaults)
        let web = try XCTUnwrap(URL(string: "http://localhost:3000/"))
        let api = try XCTUnwrap(URL(string: "http://127.0.0.1:8000/docs"))

        XCTAssertNil(store.url(for: "/work/web"))
        store.remember(web, for: "/work/web")
        store.remember(api, for: "/work/api")
        store.remember(try XCTUnwrap(URL(string: "https://example.com/")), for: "/work/web")
        XCTAssertEqual(store.url(for: "/work/web"), web)
        XCTAssertEqual(PreviewURLStore(defaults: defaults).url(for: "/work/api"), api)
    }
}
