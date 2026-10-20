import Combine
import Foundation

/// ⌘F in the chat: the query, the transcript items it matches and which match
/// is current. Kept out of the app model so a keystroke redraws the find bar,
/// not every view watching the model (#2166). Matching runs off the main actor
/// after a short pause in typing; newer work cancels older, and results for a
/// query or chat that is no longer current are dropped.
@MainActor
public final class ChatFind: ObservableObject {
    /// What the find bar holds. Not published: the field keeps its own text,
    /// so typing redraws nothing until the matches for it arrive.
    public private(set) var query = ""
    /// The query `matches` and the highlights are for.
    @Published public private(set) var appliedQuery = ""
    /// Ids of the matching items, in transcript order.
    @Published public private(set) var matches: [Int] = []
    @Published public private(set) var index = 0

    /// The pause in typing before a search starts.
    public let debounce: Duration
    private var items: [TranscriptItem] = []
    private var task: Task<Void, Never>?
    /// Bumped by every new search, so a result that finishes late is ignored.
    private var generation = 0

    public init(debounce: Duration = .milliseconds(90)) {
        self.debounce = debounce
    }

    /// The transcript item the find bar is on, if any matched.
    public var currentMatch: Int? {
        matches.isEmpty ? nil : matches[min(index, matches.count - 1)]
    }

    /// True while matches for the latest query or transcript are outstanding.
    public var isSearching: Bool { task != nil }

    /// The find bar's text changed. An empty query clears at once; anything
    /// else searches after `debounce`, replacing a search still waiting or running.
    public func setQuery(_ query: String) {
        guard query != self.query else { return }
        self.query = query
        schedule(delay: Self.normalized(query).isEmpty ? nil : debounce, resetIndex: true)
    }

    /// The chat's transcript changed. Streaming output searches again at most
    /// once per `debounce`; a switch to another chat drops the old chat's
    /// matches and any search still running for it.
    public func transcriptChanged(_ transcript: Transcript, sessionChanged: Bool = false) {
        items = transcript.items
        if sessionChanged {
            apply(query: appliedQuery, matches: [], resetIndex: true)
            schedule(delay: nil, resetIndex: true)
        } else if task == nil, !Self.normalized(query).isEmpty {
            // A pending search reads the items when it starts, so it already
            // covers this change.
            schedule(delay: debounce, resetIndex: false)
        }
    }

    /// Step through the matches, wrapping at either end.
    public func step(_ step: Int = 1) {
        let count = matches.count
        guard count > 0 else { return }
        index = ((min(index, count - 1) + step) % count + count) % count
    }

    /// Ids of the items containing `query`, ignoring case and diacritics, in
    /// order. Stops early, returning nothing, once its task is cancelled.
    public nonisolated static func matches(in items: [TranscriptItem], query: String) -> [Int] {
        let query = normalized(query)
        guard !query.isEmpty else { return [] }
        var found: [Int] = []
        for (offset, item) in items.enumerated() {
            if offset % 64 == 0, Task.isCancelled { return [] }
            if item.searchableText.range(of: query, options: [.caseInsensitive, .diacriticInsensitive]) != nil {
                found.append(item.id)
            }
        }
        return found
    }

    public nonisolated static func normalized(_ query: String) -> String {
        query.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    /// Runs on the global executor, not the main actor; a cancelled caller cancels it.
    private nonisolated static func search(_ items: [TranscriptItem], query: String) async -> [Int] {
        matches(in: items, query: query)
    }

    private func schedule(delay: Duration?, resetIndex: Bool) {
        task?.cancel()
        task = nil
        generation += 1
        let generation = generation
        let query = Self.normalized(query)
        guard !query.isEmpty else {
            apply(query: "", matches: [], resetIndex: true)
            return
        }
        task = Task { [weak self] in
            if let delay { try? await Task.sleep(for: delay) }
            guard let self, !Task.isCancelled, generation == self.generation else { return }
            let found = await Self.search(self.items, query: query)
            guard !Task.isCancelled, generation == self.generation else { return }
            self.task = nil
            self.apply(query: query, matches: found, resetIndex: resetIndex)
        }
    }

    /// Publishes only what changed. When the same query is searched again over
    /// a longer transcript, the current match stays on the same item.
    private func apply(query: String, matches found: [Int], resetIndex: Bool) {
        let current = currentMatch
        var newIndex = 0
        if !resetIndex, let current, let kept = found.firstIndex(of: current) {
            newIndex = kept
        } else if !resetIndex {
            newIndex = min(index, max(found.count - 1, 0))
        }
        if appliedQuery != query { appliedQuery = query }
        if matches != found { matches = found }
        if index != newIndex { index = newIndex }
    }
}
