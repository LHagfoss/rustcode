import Foundation

// Agent access to the localhost preview (#2138). The engine forwards each
// `localhost_preview` tool call as a `_rustcode/preview` request; the app
// checks the user's grant, runs the action in the visible preview and
// answers `{"ok": true, …}` or `{"ok": false, "error": {code, message}}`.
// Everything here is UI-free so it can be tested without a web view.

/// The `clientCapabilities._meta.rustcode.preview` version this app speaks
/// (`engine/src/preview.rs`, `PREVIEW_CAPABILITY_VERSION`).
public let rustcodePreviewCapabilityVersion = 1

extension ACPMethod {
    public static let preview = "_rustcode/preview"
}

/// Why a preview action did not run, with the stable code the engine maps.
public enum PreviewAgentError: Error, Equatable, Sendable {
    /// The user said no to this request.
    case denied
    /// Access ended while the request waited: revoked, another project, or a
    /// new engine.
    case revoked
    /// Nobody answered the approval prompt in time.
    case approvalTimedOut
    case outOfScope(String)
    case noPage
    case notFound(String)
    case invalid(String)
    case timedOut(String)
    case busy
    case failed(String)

    public var code: String {
        switch self {
        case .denied: "denied"
        case .revoked: "revoked"
        case .approvalTimedOut: "approval_timeout"
        case .outOfScope: "out_of_scope"
        case .noPage: "no_page"
        case .notFound: "not_found"
        case .invalid: "invalid"
        case .timedOut: "timeout"
        case .busy: "busy"
        case .failed: "failed"
        }
    }

    public var message: String {
        switch self {
        case .denied: "The user denied access."
        case .revoked: "Access to the preview was revoked or reset before the action ran."
        case .approvalTimedOut: "Nobody answered the access request in time."
        case .outOfScope(let detail), .notFound(let detail), .invalid(let detail),
             .timedOut(let detail), .failed(let detail): detail
        case .noPage: "Open the app's local URL in the preview first, or navigate to a full http://localhost URL."
        case .busy: "Another preview action is still running."
        }
    }

    /// The `_rustcode/preview` answer for this failure.
    public var result: JSONValue {
        ["ok": false, "error": ["code": .string(code), "message": .string(message)]]
    }
}

/// One action the agent asked for, parsed from the request's `operation`.
public enum PreviewAgentOperation: Equatable, Sendable {
    case inspect(maxChars: Int)
    case screenshot
    case navigate(String)
    case click(selector: String)
    case fill(selector: String, value: String)
    case select(selector: String, value: String)
    case scroll(selector: String?, dy: Int?)

    static let maxArgumentLength = 2048
    static let maxScroll = 100_000

    public init(_ value: JSONValue?) throws {
        guard let object = value?.objectValue else { throw PreviewAgentError.invalid("The request has no operation.") }
        func string(_ key: String, allowEmpty: Bool = false) throws -> String? {
            guard let raw = object[key], raw != .null else { return nil }
            guard let text = raw.stringValue else { throw PreviewAgentError.invalid("`\(key)` must be a string.") }
            guard text.count <= Self.maxArgumentLength else { throw PreviewAgentError.invalid("`\(key)` is too long.") }
            if !allowEmpty, text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                throw PreviewAgentError.invalid("`\(key)` is empty.")
            }
            return text
        }
        func required(_ key: String, allowEmpty: Bool = false) throws -> String {
            guard let text = try string(key, allowEmpty: allowEmpty) else { throw PreviewAgentError.invalid("`\(key)` is required.") }
            return text
        }
        switch object["action"]?.stringValue {
        case "inspect":
            let chars = object["maxChars"]?.intValue ?? 6000
            self = .inspect(maxChars: min(max(chars, 200), 20_000))
        case "screenshot": self = .screenshot
        case "navigate": self = .navigate(try required("url"))
        case "click": self = .click(selector: try required("selector"))
        case "fill": self = .fill(selector: try required("selector"), value: try required("value", allowEmpty: true))
        case "select": self = .select(selector: try required("selector"), value: try required("value", allowEmpty: true))
        case "scroll":
            var dy: Int?
            if let raw = object["dy"], raw != .null {
                guard let value = raw.doubleValue, value == value.rounded(), abs(value) <= Double(Self.maxScroll) else {
                    throw PreviewAgentError.invalid("`dy` must be a whole number of pixels.")
                }
                dy = Int(value)
            }
            let selector = try string("selector")
            guard selector != nil || dy != nil else { throw PreviewAgentError.invalid("`scroll` needs `dy` or `selector`.") }
            self = .scroll(selector: selector, dy: dy)
        case let other:
            throw PreviewAgentError.invalid("Unknown action \(other.map { "`\($0)`" } ?? "(none)").")
        }
    }

    public var action: String {
        switch self {
        case .inspect: "inspect"
        case .screenshot: "screenshot"
        case .navigate: "navigate"
        case .click: "click"
        case .fill: "fill"
        case .select: "select"
        case .scroll: "scroll"
        }
    }

    /// What the preview shows the user while the agent acts.
    public var activity: String {
        switch self {
        case .inspect: "Reading the page"
        case .screenshot: "Taking a screenshot"
        case .navigate(let url): "Opening \(url)"
        case .click(let selector): "Clicking \(selector)"
        case .fill(let selector, _): "Typing into \(selector)"
        case .select(let selector, _): "Choosing in \(selector)"
        case .scroll(let selector, let dy):
            selector.map { "Scrolling to \($0)" } ?? "Scrolling \((dy ?? 0) < 0 ? "up" : "down")"
        }
    }
}

/// A `_rustcode/preview` request: the session it came from and its action.
public struct PreviewAgentRequest: Equatable, Sendable {
    public var sessionId: String
    public var operation: PreviewAgentOperation

    public init(params: JSONValue) throws {
        guard let session = params["sessionId"]?.stringValue, !session.isEmpty else {
            throw PreviewAgentError.invalid("The request names no session.")
        }
        sessionId = session
        operation = try PreviewAgentOperation(params["operation"])
    }
}

/// An http(s) scheme, host and port: what a grant allows the agent to reach.
public struct PreviewOrigin: Hashable, Sendable, CustomStringConvertible {
    public let scheme: String
    public let host: String
    public let port: Int

    /// The origin of a local preview URL; nil for anything else.
    public init?(_ url: URL) {
        guard PreviewURL.isLocal(url), let components = URLComponents(url: url, resolvingAgainstBaseURL: false),
              let scheme = components.scheme?.lowercased(), var host = components.host?.lowercased() else { return nil }
        if host.hasPrefix("["), host.hasSuffix("]") { host = String(host.dropFirst().dropLast()) }
        if host.hasSuffix(".") { host.removeLast() }
        self.scheme = scheme
        self.host = host
        self.port = components.port ?? (scheme == "https" ? 443 : 80)
    }

    public var description: String {
        let shown = host.contains(":") ? "[\(host)]" : host
        let isDefault = (scheme == "http" && port == 80) || (scheme == "https" && port == 443)
        return "\(scheme)://\(shown)\(isDefault ? "" : ":\(port)")"
    }

    /// Where a navigate action goes: a path, query or fragment resolved
    /// against the current page, or a full local URL. It must stay on
    /// `approved` (or, with no grant yet, be local).
    public static func navigationTarget(_ target: String, from current: URL?, approved: PreviewOrigin?) -> Result<URL, PreviewAgentError> {
        let text = target.trimmingCharacters(in: .whitespacesAndNewlines)
        let resolved: URL?
        if text.hasPrefix("//") {
            resolved = nil
        } else if text.hasPrefix("/") || text.hasPrefix("?") || text.hasPrefix("#") {
            guard let current else { return .failure(.noPage) }
            resolved = URL(string: text, relativeTo: current)?.absoluteURL
        } else {
            resolved = try? PreviewURL.normalize(text).get()
        }
        guard let url = resolved, let origin = PreviewOrigin(url) else {
            return .failure(.outOfScope("\(text) is not a local address; the agent may only open the approved localhost origin."))
        }
        if let approved, origin != approved {
            return .failure(.outOfScope("\(origin) is outside the approved origin \(approved)."))
        }
        return .success(url)
    }
}

/// Who may use the preview: one project, and the chats the user allowed,
/// each for the origin it was approved on. Kept in memory only, so quitting
/// the app, switching project or restarting the engine ends every grant.
public struct PreviewAgentAccess: Equatable, Sendable {
    public private(set) var workspace: String?
    public private(set) var grants: [String: PreviewOrigin] = [:]

    public init() {}

    public var isEmpty: Bool { grants.isEmpty }

    /// Switch project. Grants never carry over to another project.
    @discardableResult
    public mutating func setWorkspace(_ path: String) -> Bool {
        guard path != workspace else { return false }
        workspace = path
        grants = [:]
        return true
    }

    public mutating func grant(session: String, origin: PreviewOrigin) {
        grants[session] = origin
    }

    public mutating func revokeAll() {
        grants = [:]
    }

    public func origin(for session: String) -> PreviewOrigin? {
        grants[session]
    }

    /// Whether `session` may act on a page at `pageURL` in `workspace`.
    /// Nil: allowed. `.denied` with no grant means the user must be asked.
    public func check(session: String, workspace: String?, pageURL: URL?) -> PreviewAgentError? {
        guard workspace != nil, workspace == self.workspace else { return .revoked }
        guard let approved = grants[session] else { return .denied }
        guard let pageURL else { return nil }
        guard let origin = PreviewOrigin(pageURL) else {
            return .outOfScope("The preview is not showing a local page.")
        }
        return origin == approved ? nil : .outOfScope("The preview is on \(origin), outside the approved origin \(approved).")
    }
}

/// A request for access waiting on the user.
public struct PreviewAgentApproval: Identifiable, Equatable, Sendable {
    public let id: UUID
    public let sessionId: String
    public let origin: PreviewOrigin
    public let workspace: String
}

/// Holds the grants and the one approval prompt the preview shows. The
/// prompt is answered by the user, ended by revoke or a project change, or
/// times out; every path resumes the waiting request exactly once.
@MainActor
public final class PreviewAgentGate {
    public private(set) var access = PreviewAgentAccess()
    public private(set) var pending: PreviewAgentApproval?
    public var approvalTimeout: Duration
    /// Called after any change the UI shows.
    public var onChange: (() -> Void)?
    private var waiter: CheckedContinuation<PreviewAgentError?, Never>?

    nonisolated public init(approvalTimeout: Duration = .seconds(120)) {
        self.approvalTimeout = approvalTimeout
    }

    /// Ask (when needed) and decide whether `session` may use `origin`.
    /// Nil when allowed.
    public func authorize(session: String, workspace: String?, origin: PreviewOrigin) async -> PreviewAgentError? {
        guard let workspace, workspace == access.workspace else { return .revoked }
        if let approved = access.origin(for: session) {
            return approved == origin ? nil : .outOfScope("\(origin) is outside the approved origin \(approved).")
        }
        guard pending == nil else { return .busy }
        let approval = PreviewAgentApproval(id: UUID(), sessionId: session, origin: origin, workspace: workspace)
        pending = approval
        onChange?()
        let timeout = approvalTimeout
        let timer = Task { [weak self] in
            try? await Task.sleep(for: timeout)
            guard !Task.isCancelled else { return }
            self?.finish(approval.id, with: .approvalTimedOut)
        }
        let outcome = await withCheckedContinuation { continuation in
            if pending?.id == approval.id { waiter = continuation } else { continuation.resume(returning: .revoked) }
        }
        timer.cancel()
        return outcome
    }

    /// The user's answer to the prompt `id`. A stale answer does nothing.
    public func answer(_ id: UUID, allow: Bool) {
        guard let approval = pending, approval.id == id else { return }
        if allow {
            guard approval.workspace == access.workspace else { return finish(id, with: .revoked) }
            access.grant(session: approval.sessionId, origin: approval.origin)
        }
        finish(id, with: allow ? nil : .denied)
    }

    /// End every grant and any open prompt.
    public func revokeAll() {
        access.revokeAll()
        if let pending { finish(pending.id, with: .revoked) } else { onChange?() }
    }

    /// The preview moved to another project: nothing granted carries over.
    public func setWorkspace(_ path: String) {
        guard access.setWorkspace(path) else { return }
        if let pending { finish(pending.id, with: .revoked) } else { onChange?() }
    }

    private func finish(_ id: UUID, with outcome: PreviewAgentError?) {
        guard pending?.id == id else { return }
        pending = nil
        let waiter = waiter
        self.waiter = nil
        waiter?.resume(returning: outcome)
        onChange?()
    }
}

/// Run `operation`, failing with `.timedOut` after `limit`. The caller is
/// released at the limit even if the operation never returns (a stuck page
/// script cannot be interrupted); the operation is cancelled, and WebKit work
/// it started may still finish, so a timeout never claims nothing happened.
public func withPreviewTimeout<T: Sendable>(
    _ limit: Duration, action: String,
    _ operation: @escaping @Sendable () async throws -> T
) async throws -> T {
    let once = ResumeOnce<T>()
    return try await withCheckedThrowingContinuation { continuation in
        once.start(continuation)
        let work = Task {
            do { once.resume(.success(try await operation())) } catch { once.resume(.failure(error)) }
        }
        let timer = Task {
            try? await Task.sleep(for: limit)
            guard !Task.isCancelled else { return }
            once.resume(.failure(PreviewAgentError.timedOut(
                "The \(action) did not finish within \(limit.components.seconds) s; its outcome is unknown.")))
            work.cancel()
        }
        once.onFinish = { timer.cancel() }
    }
}

/// Resumes a continuation exactly once, from whichever side finishes first.
private final class ResumeOnce<T: Sendable>: @unchecked Sendable {
    private let lock = NSLock()
    private var continuation: CheckedContinuation<T, Error>?
    private var early: Result<T, Error>?
    private var done = false
    var onFinish: (() -> Void)? {
        get { lock.withLock { finishHandler } }
        set {
            let runNow = lock.withLock { () -> Bool in
                finishHandler = newValue
                return done
            }
            if runNow { newValue?() }
        }
    }
    private var finishHandler: (() -> Void)?

    func start(_ continuation: CheckedContinuation<T, Error>) {
        let pendingResult = lock.withLock { () -> Result<T, Error>? in
            if let early { return early }
            self.continuation = continuation
            return nil
        }
        if let pendingResult { continuation.resume(with: pendingResult) }
    }

    func resume(_ result: Result<T, Error>) {
        let (target, handler) = lock.withLock { () -> (CheckedContinuation<T, Error>?, (() -> Void)?) in
            guard !done else { return (nil, nil) }
            done = true
            guard let continuation else {
                early = result
                return (nil, finishHandler)
            }
            self.continuation = nil
            return (continuation, finishHandler)
        }
        target?.resume(with: result)
        handler?()
    }
}
