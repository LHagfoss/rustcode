/// Report commands sent beside a running turn, per chat, oldest first. Each
/// answer goes to the oldest report of the chat it came from, so it can never
/// be shown for another chat after a switch (#2164).
public struct PendingReports: Equatable, Sendable {
    public struct Report: Equatable, Sendable {
        public let id: Int
        public let input: String
        /// Its answer arrived, so its prompt ending needs no notice.
        public var answered = false
    }

    private var bySession: [String: [Report]] = [:]
    private var nextID = 0

    public init() {}

    /// Starts a report for `session` and returns its id.
    public mutating func begin(_ input: String, in session: String) -> Int {
        nextID += 1
        bySession[session, default: []].append(Report(id: nextID, input: input))
        return nextID
    }

    /// The input of the report an answer for `session` belongs to, marking it
    /// answered; nil when the chat has none waiting.
    public mutating func answer(in session: String) -> String? {
        guard let index = bySession[session]?.firstIndex(where: { !$0.answered }) else { return nil }
        bySession[session]![index].answered = true
        return bySession[session]![index].input
    }

    /// Ends report `id`, returning it, answered or not.
    public mutating func finish(_ id: Int, in session: String) -> Report? {
        guard let index = bySession[session]?.firstIndex(where: { $0.id == id }) else { return nil }
        let report = bySession[session]!.remove(at: index)
        if bySession[session]!.isEmpty { bySession[session] = nil }
        return report
    }

    public func isWaiting(in session: String) -> Bool {
        bySession[session]?.contains { !$0.answered } ?? false
    }
}
