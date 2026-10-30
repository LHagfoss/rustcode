import Foundation

/// A pull request review being written (#2646): line comments and a summary
/// kept on this Mac until the reader submits them. Nothing is posted until
/// then, and a failed or uncertain submission keeps every draft.
public struct ReviewDrafts: Codable, Equatable, Sendable {
    /// One line comment, anchored where it was written.
    public struct Comment: Codable, Equatable, Sendable, Identifiable {
        /// The provider's sides: `RIGHT` is the file as the pull request has
        /// it (added and unchanged lines), `LEFT` the base (removed lines).
        public enum Side: String, Codable, Sendable { case left = "LEFT", right = "RIGHT" }

        public var id: UUID
        public var path: String
        public var line: Int
        public var side: Side
        /// The line's text when the comment was written, to show and to tell
        /// whether the line is still there.
        public var lineText: String
        public var body: String
        /// The commit the line was read at.
        public var commit: String

        public init(id: UUID = UUID(), path: String, line: Int, side: Side, lineText: String, body: String, commit: String) {
            self.id = id
            self.path = path
            self.line = line
            self.side = side
            self.lineText = lineText
            self.body = body
            self.commit = commit
        }

        /// The comment for a diff line, or nil for a line with no number.
        public init?(path: String, line: DiffLine, body: String, commit: String) {
            let side: Side = line.kind == .removed ? .left : .right
            guard let number = side == .left ? line.oldNumber : line.newNumber else { return nil }
            self.init(path: path, line: number, side: side, lineText: line.text, body: body, commit: commit)
        }
    }

    /// What the review says as a whole when submitted.
    public enum Event: String, Codable, Sendable, CaseIterable {
        case comment = "COMMENT", approve = "APPROVE", requestChanges = "REQUEST_CHANGES"

        public var label: String {
            switch self {
            case .comment: "Comment"
            case .approve: "Approve"
            case .requestChanges: "Request Changes"
            }
        }
    }

    /// `owner/name` of the repository.
    public var repository: String
    public var number: Int
    public var comments: [Comment] = []
    public var summary = ""
    public var event = Event.comment
    /// A submission that may or may not have reached the provider: the
    /// reader checks on the provider before submitting again.
    public var uncertainSince: Date?

    public init(repository: String, number: Int) {
        self.repository = repository
        self.number = number
    }

    public var isEmpty: Bool {
        comments.isEmpty && summary.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    /// Comments written at another commit than `head`: their lines may have
    /// moved or gone.
    public func stale(head: String) -> [Comment] {
        comments.filter { !head.isEmpty && $0.commit != head }
    }

    /// A review needs a summary unless it only comments on lines; the
    /// provider refuses a change request without one.
    public var problem: String? {
        if isEmpty { return "Add a line comment or a summary first." }
        if event == .requestChanges, summary.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            return "Say what to change in the summary to request changes."
        }
        if comments.contains(where: { $0.body.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }) {
            return "A line comment is empty."
        }
        return nil
    }

    /// The `_rustcode/pulls/review` request for these drafts at `head`.
    public func request(cwd: URL, head: String) -> JSONValue {
        .object([
            "cwd": .string(cwd.path),
            "repository": .string(repository),
            "number": .number(Double(number)),
            "commitId": .string(head),
            "event": .string(event.rawValue),
            "body": .string(summary),
            "comments": .array(comments.map {
                ["path": .string($0.path), "line": .number(Double($0.line)), "side": .string($0.side.rawValue),
                 "body": .string($0.body)]
            }),
        ])
    }

    /// `owner/name` from a pull request's web address.
    public static func repository(of url: URL?) -> String? {
        guard let parts = url?.pathComponents.filter({ $0 != "/" }), parts.count >= 2 else { return nil }
        return "\(parts[0])/\(parts[1])"
    }
}

/// How a submission ended.
public enum ReviewSubmission: Equatable, Sendable {
    case posted(URL?)
    /// The provider refused it; nothing was posted.
    case failed(String)
    /// The answer never came: it may have been posted.
    case uncertain(String)

    public init(result: JSONValue) {
        if let url = result["review"]?["url"]?.stringValue {
            self = .posted(URL(string: url))
        } else if result["review"] != nil {
            self = .posted(nil)
        } else if result["error"]?["code"]?.stringValue == "uncertain" || result["error"]?["code"]?.stringValue == "timed_out" {
            self = .uncertain(result["error"]?["message"]?.stringValue ?? "")
        } else {
            self = .failed(result["error"]?["message"]?.stringValue ?? "The review was not posted.")
        }
    }
}

/// Drafts on disk, one file per pull request, in Application Support.
public struct ReviewDraftStore: Sendable {
    public let directory: URL

    public init(directory: URL) {
        self.directory = directory
    }

    private func file(_ repository: String, _ number: Int) -> URL {
        let name = repository.replacingOccurrences(of: "/", with: "__")
        return directory.appendingPathComponent("\(name)__\(number).json")
    }

    public func load(repository: String, number: Int) -> ReviewDrafts {
        guard let data = try? Data(contentsOf: file(repository, number)),
              let drafts = try? JSONDecoder().decode(ReviewDrafts.self, from: data) else {
            return ReviewDrafts(repository: repository, number: number)
        }
        return drafts
    }

    /// Writes the drafts, or removes the file when there are none.
    public func save(_ drafts: ReviewDrafts) throws {
        let url = file(drafts.repository, drafts.number)
        if drafts.isEmpty && drafts.uncertainSince == nil {
            try? FileManager.default.removeItem(at: url)
            return
        }
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        try JSONEncoder().encode(drafts).write(to: url, options: .atomic)
    }
}
