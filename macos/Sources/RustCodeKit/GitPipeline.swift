import Foundation

/// The commit, push, pull request and CI steps a turn took, as its commands
/// show them (#2644). Only what was observed: a step's outcome comes from
/// its command's exit, never from what the agent wrote; a command that ran
/// several steps and failed does not say which one failed, so each is
/// `unknown`; a check watch that exited 8 is still pending, not failed.
public struct GitPipeline: Equatable, Sendable {
    public struct Step: Equatable, Sendable {
        public enum Kind: String, Equatable, Sendable {
            case commit, push, pullRequest, checks, merge

            public var title: String {
                switch self {
                case .commit: "Commit"
                case .push: "Push"
                case .pullRequest: "Pull request"
                case .checks: "Checks"
                case .merge: "Merge"
                }
            }
        }

        public enum Outcome: Equatable, Sendable {
            case running, succeeded, failed
            /// CI still running when the command exited (`gh pr checks` 8).
            case pending
            /// The turn was stopped first.
            case cancelled
            /// No outcome can be told: the command never reported, or it ran
            /// several steps and failed at one of them.
            case unknown
        }

        public var kind: Kind
        public var outcome: Outcome
        /// The row of the command that ran it.
        public var item: Int
        /// A pull request's URL, when the command printed one.
        public var link: URL?
        /// What the step named: a commit's message, the branch a push went to.
        public var detail: String? = nil
    }

    public var steps: [Step]

    /// The steps in `work`, a turn's items, in the order they ran. A step
    /// taken again (a second push) replaces its earlier run. Nil when the
    /// turn took none.
    public static func of(_ work: some Collection<TranscriptItem>, cancelled: Bool) -> GitPipeline? {
        var steps: [Step] = []
        for item in work {
            guard case .tool(let tool) = item.content, tool.title == "run_command", let command = tool.inputDetail else {
                continue
            }
            let kinds = kinds(in: command)
            guard !kinds.isEmpty else { continue }
            let terminal = tool.status?.isTerminal ?? false
            for kind in kinds {
                var outcome: Step.Outcome
                if tool.settledByTurnEnd || (!terminal && cancelled) {
                    outcome = cancelled ? .cancelled : .unknown
                } else if !terminal {
                    outcome = .running
                } else if tool.status == .completed {
                    outcome = .succeeded
                } else if kind == .checks, tool.exitCode == 8 {
                    outcome = .pending
                } else {
                    outcome = kinds.count == 1 ? .failed : .unknown
                }
                let link = kind == .pullRequest ? pullRequestURL(in: tool.output) : nil
                let detail: String? = switch kind {
                case .commit: CommandSummary.commitMessage(in: command)
                case .push: CommandSummary.pushTarget(in: command)
                default: nil
                }
                steps.removeAll { $0.kind == kind }
                steps.append(Step(kind: kind, outcome: outcome, item: item.id, link: link, detail: detail))
            }
        }
        return steps.isEmpty ? nil : GitPipeline(steps: steps)
    }

    /// The steps a command takes, in order: `git add -A && git commit -m x
    /// && git push` is a commit and a push.
    public static func kinds(in command: String) -> [Step.Kind] {
        var kinds: [Step.Kind] = []
        // Each part of a chain on its own, so `git commit -m "push"` is no push.
        for part in segments(command) {
            let words = part.split(whereSeparator: \.isWhitespace).map(String.init)
            guard words.count >= 2 else { continue }
            let kind: Step.Kind? = switch (words[0], words[1], words.count > 2 ? words[2] : "") {
            case ("git", "commit", _): .commit
            case ("git", "push", _): .push
            case ("gh", "pr", "create"): .pullRequest
            case ("gh", "pr", "checks"), ("gh", "run", "watch"): .checks
            case ("gh", "pr", "merge"): .merge
            default: nil
            }
            if let kind, !kinds.contains(kind) { kinds.append(kind) }
        }
        return kinds
    }

    /// A chain's commands, quoted text left whole.
    static func segments(_ command: String) -> [String] {
        var parts: [String] = []
        var current = ""
        var quote: Character?
        var previous: Character?
        for character in command {
            if let open = quote {
                if character == open { quote = nil }
                current.append(character)
            } else if character == "\"" || character == "'" {
                quote = character
                current.append(character)
            } else if character == ";" || character == "\n" || (character == "&" && previous == "&")
                        || (character == "|" && previous == "|") {
                if character == "&" || character == "|" { current.removeLast() }
                parts.append(current)
                current = ""
            } else {
                current.append(character)
            }
            previous = character
        }
        parts.append(current)
        return parts.map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
    }

    /// The pull request URL `gh pr create` printed.
    static func pullRequestURL(in output: String?) -> URL? {
        guard let output else { return nil }
        let range = output.range(of: #"https://[^\s]+/pull/\d+"#, options: .regularExpression)
        return range.flatMap { URL(string: String(output[$0])) }
    }
}
