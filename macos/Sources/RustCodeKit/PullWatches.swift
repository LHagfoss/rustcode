import Foundation

/// A thread watching one pull request (#2648): the engine checks it on its
/// own schedule and says what changed. Nothing acts on what it reports; the
/// reader decides, and can hand it to the agent as a message they send.
public struct PullWatch: Equatable, Sendable, Identifiable {
    public struct Event: Equatable, Sendable {
        /// `checks_failed`, `checks_passed`, `activity`, `conflicts`,
        /// `closed` or `stopped`.
        public var kind: String
        public var message: String
        public var at: Date?

        public init(kind: String, message: String, at: Date?) {
            self.kind = kind
            self.message = message
            self.at = at
        }

        init?(json: JSONValue) {
            guard let kind = json["kind"]?.stringValue, let message = json["message"]?.stringValue else { return nil }
            self.init(kind: kind, message: message, at: json["at"]?.stringValue.flatMap(TurnTiming.date))
        }

        public var systemImage: String {
            switch kind {
            case "checks_failed": "xmark.circle.fill"
            case "checks_passed": "checkmark.circle.fill"
            case "activity": "text.bubble.fill"
            case "conflicts": "exclamationmark.triangle.fill"
            case "closed": "checkmark.seal.fill"
            default: "pause.circle.fill"
            }
        }

        /// A change the reader likely has to act on.
        public var needsAttention: Bool { kind == "checks_failed" || kind == "conflicts" || kind == "activity" }
    }

    public var sessionId: String
    public var cwd: URL
    public var number: Int
    public var title: String
    public var url: URL?
    public var isWatching: Bool
    public var stopReason: String?
    /// The last check failed with this; the watch tries again later.
    public var lastError: String?
    public var events: [Event]

    public var id: String { "\(sessionId)#\(number)" }

    public init?(json: JSONValue) {
        guard let sessionId = json["sessionId"]?.stringValue, let number = json["number"]?.intValue,
              let cwd = json["cwd"]?.stringValue else { return nil }
        self.sessionId = sessionId
        self.cwd = URL(fileURLWithPath: cwd)
        self.number = number
        title = json["title"]?.stringValue ?? ""
        url = json["url"]?.stringValue.flatMap { $0.isEmpty ? nil : URL(string: $0) }
        isWatching = json["status"]?.stringValue == "watching"
        stopReason = json["stopReason"]?.stringValue
        lastError = json["lastError"]?.stringValue
        events = json["events"]?.arrayValue?.compactMap(Event.init(json:)) ?? []
    }

    /// What the chip in the thread's title bar says.
    public var label: String {
        isWatching ? "Watching #\(number)" : "#\(number) \(stopReason.map { "· \($0)" } ?? "· stopped")"
    }

    /// A message for the reader to send the agent about the latest events:
    /// written into the composer, never sent by itself.
    public func draft(for events: [Event]) -> String {
        let lines = events.map { "- \($0.message)" }.joined(separator: "\n")
        return "Pull request #\(number)\(title.isEmpty ? "" : " (\(title))") changed:\n\(lines)\n\nLook into it and tell me what you would do before changing anything."
    }

    /// The watches in an answer to `_rustcode/pulls/watches`.
    public static func list(result: JSONValue) -> [PullWatch] {
        result["watches"]?.arrayValue?.compactMap(PullWatch.init(json:)) ?? []
    }
}

/// A `_rustcode/pulls/watch` notification: the watch as it now is, and the
/// events this check found, which come only once.
public struct PullWatchUpdate: Equatable, Sendable {
    public var watch: PullWatch
    public var events: [PullWatch.Event]

    public init?(params: JSONValue) {
        guard let watch = params["watch"].flatMap(PullWatch.init(json:)) else { return nil }
        self.watch = watch
        events = params["events"]?.arrayValue?.compactMap(PullWatch.Event.init(json:)) ?? []
    }
}

extension ACPMethod {
    public static let pullWatchStart = "_rustcode/pulls/watch/start"
    public static let pullWatchStop = "_rustcode/pulls/watch/stop"
    public static let pullWatches = "_rustcode/pulls/watches"
    /// The engine's notification when a watch changed.
    public static let pullWatch = "_rustcode/pulls/watch"
}

extension ACPRequests {
    public static func pullWatchStart(session: String, cwd: URL, number: Int) -> JSONValue {
        ["sessionId": .string(session), "cwd": .string(cwd.path), "number": .number(Double(number))]
    }

    public static func pullWatchStop(session: String, number: Int, remove: Bool) -> JSONValue {
        ["sessionId": .string(session), "number": .number(Double(number)), "remove": .bool(remove)]
    }
}
