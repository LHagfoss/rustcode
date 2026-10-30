import Foundation

/// What a finished turn changed and which checks it ran, as observed in its
/// tool calls (#2649). Only what was seen: a check's outcome comes from its
/// call's status, never from what the agent wrote about it, and a call that
/// never reported back is `unknown`, not passed.
public struct TurnCompletion: Equatable, Sendable {
    public struct Check: Equatable, Sendable {
        public enum Outcome: Equatable, Sendable {
            case passed, failed
            /// The turn was stopped before the check reported.
            case cancelled
            /// The check never reported an outcome.
            case unknown
        }

        public var command: String
        public var outcome: Outcome
    }

    /// The first item of the turn's work: the turn row that opens its steps.
    public var anchor: Int
    /// Files the turn's edits changed, each once, in the order first edited.
    public var edited: [String]
    public var added: Int
    public var removed: Int
    public var checks: [Check]
    /// Other commands (not checks) that failed.
    public var failedCommands: Int
    public var cancelled: Bool
    /// The commit, push, pull request and CI steps the turn took (#2644).
    public var pipeline: GitPipeline? = nil

    public var failedChecks: Int { checks.filter { $0.outcome == .failed }.count }

    /// The finished turns' summaries, by the id of each turn's last reply
    /// (the item carrying its timing). A turn that neither edited files nor
    /// ran a check has none.
    public static func byLastReply(_ items: [TranscriptItem]) -> [Int: TurnCompletion] {
        var result: [Int: TurnCompletion] = [:]
        var start = 0
        for (index, item) in items.enumerated() {
            if item.startsTurn {
                start = index + 1
                continue
            }
            guard let timing = item.turn else { continue }
            if let completion = of(items[start..<index], cancelled: timing.outcome == .cancelled) {
                result[item.id] = completion
            }
        }
        return result
    }

    static func of(_ work: ArraySlice<TranscriptItem>, cancelled: Bool) -> TurnCompletion? {
        guard let anchor = work.first?.id else { return nil }
        var edited: [String] = []
        var added = 0
        var removed = 0
        var checks: [Check] = []
        var failedCommands = 0
        for item in work {
            guard case .tool(let tool) = item.content else { continue }
            if tool.status == .completed {
                for change in tool.changes {
                    if !edited.contains(change.path) { edited.append(change.path) }
                    added += change.added
                    removed += change.removed
                }
            }
            guard tool.title == "run_command", let command = tool.inputDetail else { continue }
            // Watching CI is the pipeline's checks step, not a check run here.
            let watchesCI = GitPipeline.kinds(in: command).contains(.checks)
            if isCheck(command), !watchesCI {
                let outcome: Check.Outcome = if tool.settledByTurnEnd || !(tool.status?.isTerminal ?? false) {
                    cancelled ? .cancelled : .unknown
                } else {
                    tool.status == .failed ? .failed : .passed
                }
                checks.append(Check(command: command, outcome: outcome))
            } else if tool.status == .failed, !tool.settledByTurnEnd, GitPipeline.kinds(in: command).isEmpty {
                failedCommands += 1
            }
        }
        let pipeline = GitPipeline.of(work, cancelled: cancelled)
        guard !edited.isEmpty || !checks.isEmpty || pipeline != nil else { return nil }
        return TurnCompletion(anchor: anchor, edited: edited, added: added, removed: removed, checks: checks,
                              failedCommands: failedCommands, cancelled: cancelled, pipeline: pipeline)
    }

    /// A command that verifies work: tests, type checks, lints, builds.
    public static func isCheck(_ command: String) -> Bool {
        let words = command.lowercased()
            .split(whereSeparator: { $0.isWhitespace || $0 == "&" || $0 == ";" || $0 == "|" || $0 == "(" || $0 == ")" })
            .map(String.init)
        let checkWords: Set<String> = ["test", "tests", "check", "clippy", "lint", "pytest", "jest", "vitest", "mocha",
                                       "tsc", "mypy", "ruff", "eslint", "rspec", "phpunit", "xcodebuild", "build", "vet"]
        if words.contains(where: checkWords.contains) { return true }
        // `scripts/swift-test.sh`, `npm run test:unit`, `make lint-all`.
        return words.contains { word in
            ["test", "check", "lint"].contains { word.contains($0) } && (word.contains("/") || word.contains(":") || word.contains("-"))
        }
    }
}

