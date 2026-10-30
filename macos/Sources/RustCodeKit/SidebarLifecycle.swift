import Foundation

/// Where a chat stands for the person reading the sidebar, from what the app
/// has observed of it, highest precedence first (#2633). An idle chat is
/// `.idle` however long it has been open: only an unanswered request, a
/// running turn or a reply not yet seen asks for attention.
public enum SidebarAttention: Int, Comparable, Sendable {
    /// An approval or question waits for an answer.
    case needsYou
    /// A turn is running.
    case working
    /// A turn finished with a reply the reader has not opened.
    case readyForReview
    case idle

    public static func < (lhs: Self, rhs: Self) -> Bool { lhs.rawValue < rhs.rawValue }

    public static func of(needsAnswer: Bool, isRunning: Bool, hasUnseenReply: Bool) -> SidebarAttention {
        if needsAnswer { return .needsYou }
        if isRunning { return .working }
        if hasUnseenReply { return .readyForReview }
        return .idle
    }
}

/// What the reader decided about a chat's place in the sidebar, kept apart
/// from what the chat is doing (#2634, #2635). Marking a chat done or
/// snoozing it only moves its row: its turn, its questions, its permissions
/// and its folder are untouched. Archive and Trash stay separate.
public struct SidebarLifecycle: Codable, Equatable, Sendable {
    /// Chats marked done, with when. A done chat sits on the Done shelf until
    /// it is reopened or something new happens in it.
    public private(set) var done: [String: Date] = [:]
    /// Snoozed chats and when each wakes. A wake time is an absolute instant,
    /// so a time zone change does not move it.
    public private(set) var snoozed: [String: Date] = [:]

    public init(done: [String: Date] = [:], snoozed: [String: Date] = [:]) {
        self.done = done
        self.snoozed = snoozed
    }

    /// Why a chat cannot be marked done or snoozed now: hiding it would hide
    /// a question, an approval or a running turn.
    public static func refusal(_ attention: SidebarAttention) -> String? {
        switch attention {
        case .needsYou: "It is waiting for your answer."
        case .working: "Its turn is still running."
        case .readyForReview, .idle: nil
        }
    }

    public func isDone(_ id: String) -> Bool { done[id] != nil }

    public func wake(of id: String) -> Date? { snoozed[id] }

    public func isSnoozed(_ id: String, now: Date) -> Bool {
        snoozed[id].map { $0 > now } ?? false
    }

    public mutating func markDone(_ id: String, at now: Date) {
        snoozed[id] = nil
        done[id] = now
    }

    public mutating func reopen(_ id: String) {
        done[id] = nil
    }

    public mutating func snooze(_ id: String, until wake: Date) {
        done[id] = nil
        snoozed[id] = wake
    }

    public mutating func wakeNow(_ id: String) {
        snoozed[id] = nil
    }

    /// Drop a chat that is gone, archived or trashed.
    public mutating func forget(_ id: String) {
        done[id] = nil
        snoozed[id] = nil
    }

    /// Bring back what should be seen again: snoozes whose time has come
    /// (overdue ones too, after a restart), and done or snoozed chats where
    /// something new happened, such as a question or a new turn. Returns the
    /// chats that came back, each once.
    @discardableResult
    public mutating func settle(now: Date, attention: (String) -> SidebarAttention) -> Set<String> {
        var back = Set<String>()
        for (id, wake) in snoozed where wake <= now || attention(id) < .readyForReview {
            snoozed[id] = nil
            back.insert(id)
        }
        for id in done.keys where attention(id) < .idle {
            done[id] = nil
            back.insert(id)
        }
        return back
    }

    /// The next instant a snooze ends, for a timer.
    public func nextWake(after now: Date) -> Date? {
        snoozed.values.filter { $0 > now }.min()
    }
}

/// The sidebar's groups, in the order they are shown. Each chat appears in
/// exactly one.
public struct SidebarGroups: Equatable, Sendable {
    public enum Kind: String, CaseIterable, Sendable {
        case needsYou, readyForReview, working, pinned, chats, snoozed, done

        public var title: String {
            switch self {
            case .needsYou: "Needs You"
            case .readyForReview: "Ready for Review"
            case .working: "Working"
            case .pinned: "Pinned"
            case .chats: "Chats"
            case .snoozed: "Snoozed"
            case .done: "Done"
            }
        }

        public var systemImage: String {
            switch self {
            case .needsYou: "exclamationmark.bubble"
            case .readyForReview: "eye"
            case .working: "circle.dotted"
            case .pinned: "pin.fill"
            case .chats: "bubble.left.and.bubble.right"
            case .snoozed: "moon.zzz"
            case .done: "checkmark.circle"
            }
        }

        /// A shelf starts folded: what is on it was put away on purpose.
        public var isShelf: Bool { self == .snoozed || self == .done }
    }

    public struct Group: Equatable, Sendable {
        public let kind: Kind
        public let ids: [String]
    }

    public let groups: [Group]

    /// `ids` in the sidebar's order (newest first). With `byAttention`, chats
    /// that need you, are ready for review or are working come first in
    /// their own groups; without it they stay in place, as the list was
    /// ordered before (#2633). Pinned chats keep their own group either way,
    /// unless they ask for attention.
    public init(ids: [String], pinned: Set<String>, lifecycle: SidebarLifecycle, now: Date,
                byAttention: Bool, attention: (String) -> SidebarAttention) {
        var buckets: [Kind: [String]] = [:]
        for id in ids {
            let kind: Kind
            let state = attention(id)
            if lifecycle.isDone(id), state == .idle {
                kind = .done
            } else if lifecycle.isSnoozed(id, now: now), state >= .readyForReview {
                kind = .snoozed
            } else if byAttention, state == .needsYou {
                kind = .needsYou
            } else if byAttention, state == .working {
                kind = .working
            } else if byAttention, state == .readyForReview {
                kind = .readyForReview
            } else if pinned.contains(id) {
                kind = .pinned
            } else {
                kind = .chats
            }
            buckets[kind, default: []].append(id)
        }
        groups = Kind.allCases.compactMap { kind in
            buckets[kind].map { Group(kind: kind, ids: $0) }
        }
    }

    public var ids: [String] { groups.flatMap(\.ids) }
}
