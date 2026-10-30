import Foundation

/// Threads that message other threads (#2637, #2638), milestone 1 of
/// docs/architecture/cross-session-orchestration.md. The engine forwards an
/// agent's `list_sessions`, `message_session` and `wait_session` calls here
/// as `_rustcode/orchestration` requests; the app asks the user, delivers
/// the message as a turn of the other thread through its own queue, and
/// answers with a bounded result. These are the rules and records; the app
/// carries them out.
public enum Orchestration {
    /// The setting that turns it on. Off by default; the engine is told at
    /// `initialize`, so a change applies once the engine restarts.
    public static let enabledKey = "threadMessagingEnabled"
    public static var isEnabled: Bool { UserDefaults.standard.bool(forKey: enabledKey) }
    public static let capabilityVersion = 1

    /// The most a message or an answer carries.
    public static let maxBytes = 8 * 1024
    /// Messages one thread may have waiting for another.
    public static let mailbox = 32
    /// Links one thread may have running or queued at once.
    public static let maxActiveLinks = 4
    public static let defaultWaitMs = 60_000
    public static let maxWaitMs = 3_600_000

    /// What the agent asked for.
    public enum Call: Equatable, Sendable {
        case list(query: String)
        case message(target: String, text: String)
        case wait(links: [String], timeoutMs: Int)

        /// The call in a request's params, or the error to answer with.
        public static func parse(_ params: JSONValue) -> Result<Call, Failure> {
            let args = params["args"] ?? [:]
            switch params["tool"]?.stringValue {
            case "list_sessions":
                return .success(.list(query: args["query"]?.stringValue ?? ""))
            case "message_session":
                let target = args["session_id"]?.stringValue ?? ""
                let text = args["message"]?.stringValue ?? ""
                guard !target.isEmpty, !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
                    return .failure(Failure("invalid", "session_id and message are required"))
                }
                guard text.utf8.count <= maxBytes else { return .failure(Failure("invalid", "the message is over 8 KiB")) }
                return .success(.message(target: target, text: text))
            case "wait_session":
                var links = args["link_ids"]?.arrayValue?.compactMap(\.stringValue) ?? []
                if let one = args["link_id"]?.stringValue, !links.contains(one) { links.insert(one, at: 0) }
                guard !links.isEmpty else { return .failure(Failure("invalid", "link_id is required")) }
                let timeout = min(max(args["timeout_ms"]?.intValue ?? defaultWaitMs, 1), maxWaitMs)
                return .success(.wait(links: links, timeoutMs: timeout))
            default:
                return .failure(Failure("invalid", "unknown tool"))
            }
        }
    }

    public struct Failure: Error, Equatable, Sendable {
        public var code: String
        public var message: String

        public init(_ code: String, _ message: String) {
            self.code = code
            self.message = message
        }

        public var json: JSONValue { ["error": ["code": .string(code), "message": .string(message)]] }
    }

    /// A thread as the rules see it.
    public struct Thread: Equatable, Sendable {
        public var id: String
        public var title: String
        public var folder: String
        public var updatedAt: Date?
        public var isRunning: Bool

        public init(id: String, title: String, folder: String, updatedAt: Date? = nil, isRunning: Bool = false) {
            self.id = id
            self.title = title
            self.folder = folder
            self.updatedAt = updatedAt
            self.isRunning = isRunning
        }
    }

    /// Two folders are one project when they are the same place.
    public static func sameProject(_ a: String, _ b: String) -> Bool {
        let clean = { (path: String) in URL(fileURLWithPath: path).standardizedFileURL.resolvingSymlinksInPath().path }
        return !a.isEmpty && clean(a) == clean(b)
    }

    /// `list_sessions`: the other threads of the caller's project, metadata
    /// only, newest first.
    public static func list(_ threads: [Thread], from caller: Thread, query: String) -> JSONValue {
        let words = query.lowercased().split(separator: " ")
        let shown = threads
            .filter { $0.id != caller.id && sameProject($0.folder, caller.folder) }
            .filter { thread in words.allSatisfy { thread.title.lowercased().contains($0) } }
            .sorted { ($0.updatedAt ?? .distantPast) > ($1.updatedAt ?? .distantPast) }
            .prefix(50)
        return ["sessions": .array(shown.map { thread in
            var entry: [String: JSONValue] = ["session_id": .string(thread.id), "title": .string(thread.title),
                                              "working": .bool(thread.isRunning)]
            if let updated = thread.updatedAt { entry["updated_at"] = .string(ISO8601DateFormatter().string(from: updated)) }
            return .object(entry)
        })]
    }

    /// Whether `caller` may message `target`, before the user is asked.
    public static func check(caller: Thread, target: Thread?, book: LinkBook) -> Failure? {
        guard let target else { return Failure("not_found", "no thread has that id; use list_sessions") }
        if target.id == caller.id { return Failure("not_allowed", "a thread cannot message itself") }
        guard sameProject(target.folder, caller.folder) else {
            return Failure("not_allowed", "that thread is in another project; only this project's threads can be messaged")
        }
        // No chains: a thread working for another one cannot pass work on.
        if book.links.contains(where: { $0.target == caller.id && $0.state.isActive }) {
            return Failure("not_allowed", "this thread is answering another thread and cannot message others")
        }
        if book.links.contains(where: { $0.target == target.id && $0.orchestrator != caller.id && $0.state.isActive }) {
            return Failure("target_linked", "another thread is already working with that one")
        }
        if book.links.filter({ $0.orchestrator == caller.id && $0.state.isActive }).count >= maxActiveLinks {
            return Failure("busy", "this thread already has \(maxActiveLinks) messages under way; wait for one first")
        }
        if book.links.filter({ $0.target == target.id && $0.state == .queued }).count >= mailbox {
            return Failure("mailbox_full", "that thread has \(mailbox) messages waiting")
        }
        return nil
    }

    /// The message as the other thread receives it: who it is from, then
    /// the text. A user message, so it changes none of that thread's rules.
    public static func delivered(_ text: String, from title: String) -> String {
        "From thread “\(title)” (agent):\n\n\(text)"
    }

    /// `text` cut to `maxBytes` on a character boundary, and whether it was.
    public static func capped(_ text: String) -> (text: String, truncated: Bool) {
        guard text.utf8.count > maxBytes else { return (text, false) }
        var end = text.startIndex
        var bytes = 0
        for index in text.indices {
            let size = text[index].utf8.count
            if bytes + size > maxBytes { break }
            bytes += size
            end = text.index(after: index)
        }
        return (String(text[..<end]), true)
    }
}

/// One thread messaging another: from the message to the answer.
public struct OrchestrationLink: Codable, Equatable, Sendable, Identifiable {
    public enum State: String, Codable, Sendable {
        case queued, working, needsUser = "needs_user", done, failed, cancelled, interrupted

        public var isActive: Bool { self == .queued || self == .working || self == .needsUser }

        public var label: String {
            switch self {
            case .queued: "Queued"
            case .working: "Working"
            case .needsUser: "Needs you"
            case .done: "Done"
            case .failed: "Failed"
            case .cancelled: "Stopped"
            case .interrupted: "Interrupted"
            }
        }
    }

    /// `<orchestrator>/<n>`.
    public var id: String
    public var orchestrator: String
    public var target: String
    public var orchestratorTitle: String
    public var targetTitle: String
    public var message: String
    public var state: State
    public var answer: String?
    public var truncated = false
    public var createdAt: Date

    public init(id: String, orchestrator: String, target: String, orchestratorTitle: String, targetTitle: String,
                message: String, state: State, createdAt: Date = Date()) {
        self.id = id
        self.orchestrator = orchestrator
        self.target = target
        self.orchestratorTitle = orchestratorTitle
        self.targetTitle = targetTitle
        self.message = message
        self.state = state
        self.createdAt = createdAt
    }

    /// `wait_session`'s answer for this link.
    public var result: JSONValue {
        var entry: [String: JSONValue] = ["link_id": .string(id), "status": .string(state.rawValue),
                                          "session_id": .string(target), "title": .string(targetTitle)]
        if let answer { entry["answer"] = .string(answer) }
        if truncated { entry["truncated"] = true }
        if state == .needsUser { entry["note"] = "That thread is waiting for the user to answer it there." }
        return .object(entry)
    }
}

/// Every link and grant, kept on disk.
public struct LinkBook: Codable, Equatable, Sendable {
    public var links: [OrchestrationLink] = []
    /// Targets each thread may message without asking again.
    public var grants: [String: Set<String>] = [:]
    private var counters: [String: Int] = [:]

    public init() {}

    public mutating func nextId(for orchestrator: String) -> String {
        let next = (counters[orchestrator] ?? 0) + 1
        counters[orchestrator] = next
        return "\(orchestrator)/\(next)"
    }

    public func link(_ id: String) -> OrchestrationLink? { links.first { $0.id == id } }

    public mutating func update(_ id: String, _ change: (inout OrchestrationLink) -> Void) {
        guard let index = links.firstIndex(where: { $0.id == id }) else { return }
        change(&links[index])
    }

    /// After a restart: work that was queued or running did not finish
    /// here, and nothing restarts by itself.
    public mutating func interruptActive() {
        for index in links.indices where links[index].state.isActive {
            links[index].state = .interrupted
        }
    }

    /// Links kept: every active one, and the latest 50 finished.
    public mutating func prune() {
        let finished = links.filter { !$0.state.isActive }.sorted { $0.createdAt > $1.createdAt }.prefix(50).map(\.id)
        links.removeAll { !$0.state.isActive && !finished.contains($0.id) }
    }

    /// A thread deleted: links from it are forgotten, links to it break.
    public mutating func forget(thread id: String) {
        links.removeAll { $0.orchestrator == id }
        for index in links.indices where links[index].target == id && links[index].state.isActive {
            links[index].state = .failed
            links[index].answer = "That thread was deleted."
        }
        grants[id] = nil
        for key in grants.keys { grants[key]?.remove(id) }
    }

    public static func load(from url: URL) -> LinkBook {
        guard let data = try? Data(contentsOf: url), var book = try? JSONDecoder().decode(LinkBook.self, from: data) else {
            return LinkBook()
        }
        book.interruptActive()
        return book
    }

    public func save(to url: URL) throws {
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try JSONEncoder().encode(self).write(to: url, options: .atomic)
    }
}
