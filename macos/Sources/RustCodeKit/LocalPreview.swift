import Foundation

/// Why an address typed into the localhost preview was not loaded.
public enum PreviewURLError: Error, Equatable, Sendable {
    case empty
    case invalid
    case unsupportedScheme(String)
    /// A valid web address on another machine. The preview shows only local
    /// servers, but the URL can still be opened in the browser.
    case notLocal(URL)

    public var message: String {
        switch self {
        case .empty: "Enter a local URL, such as http://localhost:3000."
        case .invalid: "That is not a valid URL."
        case .unsupportedScheme(let scheme): "The preview opens http and https pages, not \(scheme):."
        case .notLocal(let url):
            "The preview opens only local servers (localhost, 127.0.0.1, [::1]), not \(PreviewURL.displayHost(url))."
        }
    }

    /// The address to offer in the browser instead, when there is one.
    public var browserURL: URL? {
        if case .notLocal(let url) = self { return url }
        return nil
    }
}

/// What the preview does with a navigation the page starts.
public enum PreviewNavigationPolicy: Equatable, Sendable {
    case allow
    /// Leave the preview: another site, or a link such as `mailto:`.
    case openExternally
    case cancel
}

/// Addresses the localhost preview accepts: http(s) on a loopback host.
public enum PreviewURL {
    /// Read what was typed as a local web address. A bare port (`3000`,
    /// `:3000`) means localhost, and a missing scheme means http.
    public static func normalize(_ input: String) -> Result<URL, PreviewURLError> {
        var text = input.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return .failure(.empty) }
        if text.hasPrefix(":") {
            text = "localhost" + text
        } else if text.allSatisfy({ ("0"..."9").contains($0) }) {
            text = "localhost:" + text
        }
        if !text.contains("://") { text = "http://" + text }
        guard var components = URLComponents(string: text), let scheme = components.scheme?.lowercased() else {
            return .failure(.invalid)
        }
        guard scheme == "http" || scheme == "https" else { return .failure(.unsupportedScheme(scheme)) }
        guard let host = components.host, !host.isEmpty else { return .failure(.invalid) }
        components.scheme = scheme
        if components.path.isEmpty { components.path = "/" }
        guard let url = components.url else { return .failure(.invalid) }
        guard isLocal(url) else { return .failure(.notLocal(url)) }
        return .success(url)
    }

    /// An http(s) URL whose host is this Mac: `localhost` and its subdomains,
    /// 127.0.0.0/8 or `::1`.
    public static func isLocal(_ url: URL) -> Bool {
        guard let scheme = url.scheme?.lowercased(), scheme == "http" || scheme == "https",
              let host = bareHost(url) else { return false }
        if host == "localhost" || host.hasSuffix(".localhost") { return true }
        if host == "::1" || host == "0:0:0:0:0:0:0:1" { return true }
        let octets = host.split(separator: ".", omittingEmptySubsequences: false)
        guard octets.count == 4, octets[0] == "127" else { return false }
        return octets.allSatisfy { octet in
            guard !octet.isEmpty, octet.count <= 3, let value = Int(octet) else { return false }
            return value <= 255
        }
    }

    /// The host and port, as `localhost:3000` or `[::1]:8080`.
    public static func displayHost(_ url: URL) -> String {
        guard let host = bareHost(url) else { return url.absoluteString }
        let shown = host.contains(":") ? "[\(host)]" : host
        let port = URLComponents(url: url, resolvingAgainstBaseURL: false)?.port
        return port.map { "\(shown):\($0)" } ?? shown
    }

    /// Where a navigation the page starts may go. Subframes and assets may
    /// load from anywhere, as in a browser; the page itself stays local.
    public static func policy(for url: URL, isMainFrame: Bool) -> PreviewNavigationPolicy {
        guard let scheme = url.scheme?.lowercased() else { return .allow }
        switch scheme {
        case "http", "https": return !isMainFrame || isLocal(url) ? .allow : .openExternally
        case "about", "blob", "data": return .allow
        case "file": return .cancel
        default: return .openExternally
        }
    }

    /// The host in lower case, without IPv6 brackets or a trailing dot.
    private static func bareHost(_ url: URL) -> String? {
        guard var host = URLComponents(url: url, resolvingAgainstBaseURL: false)?.host?.lowercased(),
              !host.isEmpty else { return nil }
        if host.hasPrefix("["), host.hasSuffix("]") { host = String(host.dropFirst().dropLast()) }
        if host.hasSuffix(".") { host.removeLast() }
        return host
    }
}

/// A page load that failed in a way worth showing.
public struct PreviewFailure: Equatable, Sendable {
    public enum Kind: Equatable, Sendable {
        /// Nothing answered: the dev server is not running on that port.
        case serverUnavailable
        case timedOut
        case other
    }

    public var kind: Kind
    public var message: String

    public init(kind: Kind, message: String) {
        self.kind = kind
        self.message = message
    }

    /// Classify a WebKit or URL loading error. Nil for the errors a
    /// navigation reports when it is merely replaced or stopped.
    public init?(domain: String, code: Int, description: String) {
        if domain == NSURLErrorDomain {
            switch code {
            case URLError.Code.cancelled.rawValue:
                return nil
            case URLError.Code.cannotConnectToHost.rawValue, URLError.Code.cannotFindHost.rawValue,
                 URLError.Code.networkConnectionLost.rawValue, URLError.Code.dnsLookupFailed.rawValue,
                 URLError.Code.notConnectedToInternet.rawValue:
                self.init(kind: .serverUnavailable, message: description)
            case URLError.Code.timedOut.rawValue:
                self.init(kind: .timedOut, message: description)
            default:
                self.init(kind: .other, message: description)
            }
        } else if domain == "WebKitErrorDomain", code == 102 {
            // Frame load interrupted: the response became a download or was
            // replaced by a policy decision, not a failure of the page.
            return nil
        } else {
            self.init(kind: .other, message: description)
        }
    }

    public func title(for url: URL?) -> String {
        switch kind {
        case .serverUnavailable:
            return "Nothing is running at \(url.map(PreviewURL.displayHost) ?? "this address")"
        case .timedOut:
            return url.map { "\(PreviewURL.displayHost($0)) did not answer in time" } ?? "The server did not answer in time"
        case .other:
            return "The page could not load"
        }
    }

    public var detail: String {
        switch kind {
        case .serverUnavailable: "Start the project's dev server, then retry."
        case .timedOut: "The server may be busy or stuck. Check its output, then retry."
        case .other: message
        }
    }
}

/// Where the preview's page load is.
public enum PreviewPhase: Equatable, Sendable {
    /// No page yet: the preview waits for a URL.
    case idle
    case loading
    case loaded
    case failed(PreviewFailure)
}

/// The preview's page state, driven by the web view's navigation events.
public struct PreviewState: Equatable, Sendable {
    public private(set) var phase: PreviewPhase = .idle
    /// The page shown or being loaded; after a failure, the one that failed.
    public private(set) var url: URL?

    public init() {}

    public var isLoading: Bool { phase == .loading }

    public var failure: PreviewFailure? {
        if case .failed(let failure) = phase { return failure }
        return nil
    }

    /// A navigation started, to `url` when it is known.
    public mutating func begin(_ url: URL?) {
        if let url { self.url = url }
        phase = .loading
    }

    public mutating func finish(_ url: URL?) {
        if let url { self.url = url }
        phase = .loaded
    }

    public mutating func fail(_ failure: PreviewFailure, url: URL?) {
        if let url { self.url = url }
        phase = .failed(failure)
    }

    /// The loaded page changed its URL itself, as client-side routing does.
    /// Ignored while loading or failed, so a failed URL stays the one to retry.
    public mutating func navigated(to url: URL) {
        guard phase == .loaded else { return }
        self.url = url
    }

    /// Loading was stopped: back to the page shown before, or to waiting.
    public mutating func cancel() {
        guard phase == .loading else { return }
        phase = url == nil ? .idle : .loaded
    }

    public mutating func reset() {
        self = PreviewState()
    }
}

/// The last preview URL of each project, kept on this Mac.
public struct PreviewURLStore {
    public static let key = "previewURLs"
    private let defaults: UserDefaults

    public init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
    }

    public func url(for workspace: String) -> URL? {
        guard let string = all[workspace], let url = URL(string: string), PreviewURL.isLocal(url) else { return nil }
        return url
    }

    public func remember(_ url: URL, for workspace: String) {
        guard PreviewURL.isLocal(url) else { return }
        var urls = all
        guard urls[workspace] != url.absoluteString else { return }
        urls[workspace] = url.absoluteString
        defaults.set(urls, forKey: Self.key)
    }

    private var all: [String: String] {
        defaults.dictionary(forKey: Self.key) as? [String: String] ?? [:]
    }
}
