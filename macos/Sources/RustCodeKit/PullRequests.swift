import Foundation

/// A project's pull requests as the engine reads them (`_rustcode/pulls`,
/// #2645): read-only, from the hosting provider through its CLI. Nothing
/// here checks out, reviews, comments, approves or merges.
public struct PullRequest: Identifiable, Equatable, Sendable {
    public enum State: String, Sendable, CaseIterable {
        case open, closed, merged
    }

    public struct Checks: Equatable, Sendable {
        public var passed = 0, failed = 0, pending = 0, skipped = 0
        public var total: Int { passed + failed + pending + skipped }

        /// "2 failing", "1 running", "5 passed", or nil with no checks.
        public var label: String? {
            if failed > 0 { return "\(failed) failing" }
            if pending > 0 { return "\(pending) running" }
            if passed > 0 { return "\(passed) passed" }
            return nil
        }
    }

    public var number: Int
    public var title: String
    public var author: String
    public var state: State
    public var draft: Bool
    /// `approved`, `changes_requested` or `review_required`.
    public var review: String?
    public var checks: Checks
    public var headRef: String
    public var baseRef: String
    /// The exact commit the pull request is at.
    public var headSHA: String
    public var updatedAt: Date?
    public var url: URL?
    public var additions: Int
    public var deletions: Int
    public var changedFiles: Int

    public var id: Int { number }

    public init?(json: JSONValue) {
        guard let number = json["number"]?.intValue else { return nil }
        self.number = number
        title = json["title"]?.stringValue ?? ""
        author = json["author"]?.stringValue ?? ""
        state = json["state"]?.stringValue.flatMap(State.init(rawValue:)) ?? .open
        draft = json["draft"]?.boolValue ?? false
        review = json["review"]?.stringValue
        let checks = json["checks"]
        self.checks = Checks(passed: checks?["passed"]?.intValue ?? 0, failed: checks?["failed"]?.intValue ?? 0,
                             pending: checks?["pending"]?.intValue ?? 0, skipped: checks?["skipped"]?.intValue ?? 0)
        headRef = json["headRef"]?.stringValue ?? ""
        baseRef = json["baseRef"]?.stringValue ?? ""
        headSHA = json["headSha"]?.stringValue ?? ""
        updatedAt = json["updatedAt"]?.stringValue.flatMap(TurnTiming.date)
        url = json["url"]?.stringValue.flatMap(URL.init(string:))
        additions = json["additions"]?.intValue ?? 0
        deletions = json["deletions"]?.intValue ?? 0
        changedFiles = json["changedFiles"]?.intValue ?? 0
    }

    /// "Approved", "Changes requested", "Review required", or nil.
    public var reviewLabel: String? {
        switch review {
        case "approved": "Approved"
        case "changes_requested": "Changes requested"
        case "review_required": "Review required"
        default: nil
        }
    }

    /// The short commit, as people quote it.
    public var shortSHA: String { String(headSHA.prefix(7)) }

    /// A draft for a new thread about this pull request, naming the exact
    /// commit so the agent looks at what the reader saw. Never sent by itself.
    public var threadDraft: String {
        var lines = ["Pull request #\(number): \(title)"]
        if let url { lines.append(url.absoluteString) }
        lines.append("Branch \(headRef) into \(baseRef), at commit \(shortSHA).")
        lines.append("")
        lines.append("Read it with `gh pr view \(number)` and `gh pr diff \(number)`, without checking it out or posting anything. ")
        return lines.joined(separator: "\n")
    }
}

/// One page of pull requests, or why there is none.
public enum PullRequestList: Equatable, Sendable {
    case loaded(pulls: [PullRequest], hasMore: Bool)
    case failed(PullRequestError)

    public init(result: JSONValue) {
        if let error = PullRequestError(json: result["error"]) {
            self = .failed(error)
            return
        }
        let list = result["list"]
        self = .loaded(pulls: list?["pulls"]?.arrayValue?.compactMap(PullRequest.init(json:)) ?? [],
                       hasMore: list?["hasMore"]?.boolValue ?? false)
    }
}

public struct PullRequestError: Error, Equatable, Sendable {
    /// `gh_missing`, `not_connected`, `not_a_repository`, `timed_out`, `failed`.
    public var code: String
    public var message: String

    public init(code: String, message: String) {
        self.code = code
        self.message = message
    }

    init?(json: JSONValue?) {
        guard let code = json?["code"]?.stringValue else { return nil }
        self.init(code: code, message: json?["message"]?.stringValue ?? "")
    }

    /// What the reader can do about it.
    public var title: String {
        switch code {
        case "gh_missing": "The GitHub CLI is not installed"
        case "not_connected": "GitHub is not connected"
        case "not_a_repository": "This folder has no GitHub repository"
        case "timed_out": "GitHub did not answer in time"
        default: "Pull requests could not be read"
        }
    }

    /// Whether Settings ▸ Connections is the way to fix it.
    public var opensConnections: Bool { code == "gh_missing" || code == "not_connected" }
}

/// A pull request in full: description, checks, files and activity.
public struct PullRequestDetail: Equatable, Sendable {
    public struct Check: Equatable, Sendable, Identifiable {
        public var name: String
        /// `passed`, `failed`, `pending` or `skipped`.
        public var status: String
        public var url: URL?
        public var id: String { name + status }
    }

    public struct File: Equatable, Sendable, Identifiable {
        public var path: String
        public var additions: Int
        public var deletions: Int
        public var id: String { path }
    }

    public struct Activity: Equatable, Sendable, Identifiable {
        public var author: String
        /// `commented`, or a review state such as `approved`.
        public var kind: String
        public var at: Date?
        public var body: String
        public var id: String { "\(author) \(kind) \(at?.timeIntervalSince1970 ?? 0)" }
    }

    public var pull: PullRequest
    public var body: String
    public var checks: [Check]
    public var files: [File]
    public var activity: [Activity]

    public init?(json: JSONValue) {
        guard let pull = PullRequest(json: json) else { return nil }
        self.pull = pull
        body = json["body"]?.stringValue ?? ""
        checks = json["checkRuns"]?.arrayValue?.map {
            Check(name: $0["name"]?.stringValue ?? "", status: $0["status"]?.stringValue ?? "pending",
                  url: $0["url"]?.stringValue.flatMap(URL.init(string:)))
        } ?? []
        files = json["files"]?.arrayValue?.map {
            File(path: $0["path"]?.stringValue ?? "", additions: $0["additions"]?.intValue ?? 0,
                 deletions: $0["deletions"]?.intValue ?? 0)
        } ?? []
        activity = json["activity"]?.arrayValue?.map {
            Activity(author: $0["author"]?.stringValue ?? "", kind: $0["kind"]?.stringValue ?? "",
                     at: $0["at"]?.stringValue.flatMap(TurnTiming.date), body: $0["body"]?.stringValue ?? "")
        } ?? []
    }

    /// The answer to `_rustcode/pulls/view`.
    public static func from(result: JSONValue) -> Result<PullRequestDetail, PullRequestError> {
        if let error = PullRequestError(json: result["error"]) { return .failure(error) }
        guard let pull = result["pull"], let detail = PullRequestDetail(json: pull) else {
            return .failure(PullRequestError(code: "failed", message: "The engine answered without a pull request."))
        }
        return .success(detail)
    }
}

extension ACPMethod {
    public static let pulls = "_rustcode/pulls"
    public static let pullView = "_rustcode/pulls/view"
}

extension ACPRequests {
    public static func pulls(cwd: URL, state: String, search: String, limit: Int) -> JSONValue {
        var params: [String: JSONValue] = ["cwd": .string(cwd.path), "state": .string(state),
                                           "limit": .number(Double(limit))]
        let search = search.trimmingCharacters(in: .whitespaces)
        if !search.isEmpty { params["search"] = .string(search) }
        return .object(params)
    }

    public static func pullView(cwd: URL, number: Int) -> JSONValue {
        ["cwd": .string(cwd.path), "number": .number(Double(number))]
    }
}
