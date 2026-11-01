import Foundation

/// A shell command in words, for the turn summary: `git add -A && git commit
/// -m 'fix: x'` is `Committed “fix: x”`, `cd app && cargo test --workspace`
/// is `Ran tests` tagged `cargo`. The chain's most telling command names the
/// row, the other telling ones are a count, and the whole command stays for
/// the tooltip and the opened row. Only the text is read; nothing runs.
public struct CommandSummary: Equatable, Sendable {
    public enum Kind: String, Equatable, Sendable {
        case commit, push, pullRequest, merge, ciChecks
        case branch, stage, sync, diffCheck, diff, history, status
        case test, build, typecheck, lint, format, install, other

        /// The SF Symbol its row starts with.
        public var symbol: String {
            switch self {
            case .commit: "smallcircle.filled.circle"
            case .push: "arrow.up.circle"
            case .pullRequest: "arrow.triangle.pull"
            case .merge: "arrow.triangle.merge"
            case .ciChecks: "checklist"
            case .branch: "arrow.triangle.branch"
            case .stage: "plus.square.on.square"
            case .sync: "arrow.down.circle"
            case .diffCheck: "text.magnifyingglass"
            case .diff: "plusminus"
            case .history: "clock.arrow.circlepath"
            case .status: "info.circle"
            case .test: "testtube.2"
            case .build: "hammer"
            case .typecheck: "checkmark.shield"
            case .lint: "sparkles"
            case .format: "text.alignleft"
            case .install: "shippingbox"
            case .other: "terminal"
            }
        }
    }

    public var kind: Kind
    /// What it did: `Committed`, `Ran tests`, `Pushed`.
    public var label: String
    /// What it did it to, read after the label: `“fix: x”`, `to main`.
    public var detail: String?
    /// The tool that did it, drawn as a tag: `cargo`, `pytest`.
    public var tag: String?
    /// Other telling commands in the same chain.
    public var others: Int = 0

    public var symbol: String { kind.symbol }

    /// The steps `GitPipeline` draws on their own rows; a check's row leaves
    /// them out so a commit is not told twice.
    public static let pipelineKinds: Set<Kind> = [.commit, .push, .pullRequest, .merge, .ciChecks]

    /// The command in words. Commands of an `omitting` kind are passed over,
    /// unless nothing else is left.
    public static func of(_ command: String, omitting: Set<Kind> = []) -> CommandSummary {
        let parts = commands(in: command).map(summary)
        let kept = parts.filter { !omitting.contains($0.summary.kind) }
        let pool = kept.isEmpty ? parts : kept
        guard let best = pool.enumerated().max(by: { a, b in
            // The first of equal rank wins: `max` keeps the later one, so
            // compare as if the earlier were larger.
            a.element.rank != b.element.rank ? a.element.rank < b.element.rank : a.offset > b.offset
        }) else {
            return CommandSummary(kind: .other, label: "Ran a command")
        }
        var result = best.element.summary
        result.others = pool.enumerated().filter { $0.offset != best.offset && $0.element.rank >= 2 }.count
        return result
    }

    /// The message `git commit` was given, its first line. A heredoc
    /// (`-m "$(cat <<'EOF' … EOF)"`) gives its first line of text.
    public static func commitMessage(in command: String) -> String? {
        for words in commands(in: command) {
            guard let git = gitArguments(words), git.first == "commit" else { continue }
            if let message = commitMessage(arguments: Array(git.dropFirst())) { return message }
        }
        return nil
    }

    /// The branch `git push` named, when it named one.
    public static func pushTarget(in command: String) -> String? {
        for words in commands(in: command) {
            guard let git = gitArguments(words), git.first == "push" else { continue }
            if let target = pushTarget(arguments: Array(git.dropFirst())) { return target }
        }
        return nil
    }

    // MARK: - Reading a command

    /// Each command of a chain as its words, quotes removed, with `bash -lc
    /// '…'` opened to the commands it runs and leading `FOO=1`, `env`,
    /// `sudo`, `time` and `timeout 60` set aside. Recursion stops at a depth
    /// that a real command never reaches.
    static func commands(in command: String, depth: Int = 0) -> [[String]] {
        var result: [[String]] = []
        for segment in GitPipeline.segments(command) {
            var words = Self.words(segment)
            words = withoutPrefixes(words)
            guard let first = words.first else { continue }
            let program = programName(first)
            if depth < 3, ["sh", "bash", "zsh", "dash", "ksh", "fish"].contains(program),
               let flag = words.dropFirst().firstIndex(where: { $0.hasPrefix("-") && !$0.hasPrefix("--") && $0.contains("c") }),
               flag + 1 < words.count {
                result += commands(in: words[flag + 1], depth: depth + 1)
                continue
            }
            result.append(words)
        }
        return result
    }

    /// A command's words: split at blanks outside quotes, with the quotes
    /// and backslash escapes removed. A pipe ends the words: `cargo test |
    /// tail` is cargo's command.
    static func words(_ text: String) -> [String] {
        var words: [String] = []
        var current = ""
        var started = false
        var quote: Character?
        var escaped = false
        var depth = 0
        for character in text {
            if escaped {
                current.append(character)
                escaped = false
                continue
            }
            if let open = quote {
                if character == open && depth == 0 {
                    quote = nil
                } else {
                    if open == "\"", character == "\\" { escaped = true; continue }
                    // `"$(cat <<'EOF' … )"` keeps its inner quotes whole.
                    if open == "\"", character == "(", current.hasSuffix("$") { depth += 1 }
                    if open == "\"", character == ")", depth > 0 { depth -= 1 }
                    current.append(character)
                }
                continue
            }
            switch character {
            case "\\": escaped = true; started = true
            case "\"", "'": quote = character; started = true
            case "|":
                if started { words.append(current) }
                return words
            case _ where character.isWhitespace:
                if started { words.append(current) }
                current = ""
                started = false
            default:
                current.append(character)
                started = true
            }
        }
        if started { words.append(current) }
        return words
    }

    /// The words with what only sets up the command set aside.
    static func withoutPrefixes(_ words: [String]) -> [String] {
        var index = 0
        while index < words.count {
            let word = words[index]
            if isAssignment(word) { index += 1; continue }
            switch programName(word) {
            case "env", "sudo", "nice", "nohup", "command", "exec", "time", "xcrun", "npx", "bunx":
                index += 1
                while index < words.count, words[index].hasPrefix("-") || isAssignment(words[index]) { index += 1 }
            case "timeout":
                index += 1
                while index < words.count, words[index].hasPrefix("-") { index += 1 }
                index += 1
            default:
                return Array(words[index...])
            }
        }
        return []
    }

    static func isAssignment(_ word: String) -> Bool {
        guard let equals = word.firstIndex(of: "="), equals != word.startIndex else { return false }
        return word[..<equals].allSatisfy { $0.isLetter || $0.isNumber || $0 == "_" }
    }

    /// `/usr/bin/git` is `git`.
    static func programName(_ word: String) -> String {
        String(word.split(separator: "/").last ?? Substring(word))
    }

    /// `git -C dir commit -m x` is `["commit", "-m", "x"]`; nil for a
    /// command that is not git.
    static func gitArguments(_ words: [String]) -> [String]? {
        guard let first = words.first, programName(first) == "git" else { return nil }
        var index = 1
        while index < words.count, words[index].hasPrefix("-") {
            index += ["-C", "-c", "--git-dir", "--work-tree"].contains(words[index]) ? 2 : 1
        }
        return index < words.count ? Array(words[index...]) : []
    }

    /// The words that are not flags, `--` set aside.
    static func positional(_ arguments: [String]) -> [String] {
        arguments.filter { !$0.hasPrefix("-") }
    }

    static func commitMessage(arguments: [String]) -> String? {
        var index = 0
        while index < arguments.count {
            let word = arguments[index]
            var message: String?
            // `-m x`, and `-am x` where the flags run together.
            let cluster = word.hasPrefix("-") && !word.hasPrefix("--") && word.hasSuffix("m")
            if word == "--message" || cluster, index + 1 < arguments.count {
                message = arguments[index + 1]
            } else if word.hasPrefix("--message=") {
                message = String(word.dropFirst("--message=".count))
            } else if word.hasPrefix("-m"), word.count > 2, !word.hasPrefix("--") {
                message = String(word.dropFirst(2))
            }
            if let message, let line = firstLine(of: message) { return line }
            index += 1
        }
        return nil
    }

    /// The first line of text in a message, past a heredoc's opening line.
    static func firstLine(of message: String) -> String? {
        var lines = message.split(separator: "\n", omittingEmptySubsequences: false).map {
            $0.trimmingCharacters(in: .whitespaces)
        }
        if let first = lines.first, first.hasPrefix("$(cat <<") || first.hasPrefix("$(cat<<") {
            lines.removeFirst()
        }
        return lines.first { !$0.isEmpty && $0 != ")" }
    }

    static func pushTarget(arguments: [String]) -> String? {
        let names = positional(arguments)
        guard names.count >= 2 else { return nil }
        let refspec = names[1]
        let target = refspec.split(separator: ":").last.map(String.init) ?? refspec
        return target == "HEAD" || target.isEmpty ? nil : target
    }

    // MARK: - Naming a command

    /// A command's words in words, with how telling it is: a commit says more
    /// than a test run, a test run more than `git status`, and `cd` nothing.
    static func summary(_ words: [String]) -> (summary: CommandSummary, rank: Int) {
        guard let first = words.first else { return (CommandSummary(kind: .other, label: "Ran a command"), 0) }
        let program = programName(first)
        let arguments = Array(words.dropFirst())
        if let git = gitArguments(words) { return gitSummary(git) }
        if program == "gh" { return ghSummary(arguments) }
        if let check = checkSummary(program: program, arguments: arguments) { return check }
        let quiet: Set<String> = ["cd", "pushd", "popd", "echo", "printf", "true", "false", ":", "pwd", "export",
                                  "source", ".", "set", "unset", "sleep", "exit", "wait", "test", "["]
        let reads: Set<String> = ["ls", "cat", "head", "tail", "sed", "awk", "rg", "grep", "find", "fd", "wc", "which",
                                  "stat", "file", "tree", "diff", "sort", "uniq", "jq", "less", "nl"]
        let rank = quiet.contains(program) ? 0 : reads.contains(program) ? 1 : 3
        return (CommandSummary(kind: .other, label: "Ran \(program)"), rank)
    }

    static func gitSummary(_ git: [String]) -> (summary: CommandSummary, rank: Int) {
        let sub = git.first ?? ""
        let arguments = Array(git.dropFirst())
        let names = positional(arguments)
        func make(_ kind: Kind, _ label: String, _ detail: String? = nil, _ rank: Int) -> (CommandSummary, Int) {
            (CommandSummary(kind: kind, label: label, detail: detail), rank)
        }
        switch sub {
        case "commit":
            if let message = commitMessage(arguments: arguments) { return make(.commit, "Committed", "“\(message)”", 10) }
            return make(.commit, arguments.contains("--amend") ? "Amended the commit" : "Committed", nil, 10)
        case "push":
            return make(.push, "Pushed", pushTarget(arguments: arguments).map { "to \($0)" }, 8)
        case "switch", "checkout":
            if let flag = arguments.firstIndex(where: { ["-c", "-b", "-C", "-B", "--create"].contains($0) }),
               flag + 1 < arguments.count {
                return make(.branch, "Created branch", arguments[flag + 1], 4)
            }
            if arguments.contains("--") || sub == "checkout" && names.count > 1 { return make(.other, "Restored files", nil, 3) }
            return make(.branch, "Switched to", names.first, 4)
        case "branch":
            if arguments.contains(where: { ["-d", "-D", "--delete"].contains($0) }) {
                return make(.branch, "Deleted branch", names.first, 4)
            }
            if arguments.contains(where: { ["-m", "-M", "--move"].contains($0) }) {
                return make(.branch, "Renamed branch", names.count > 1 ? "\(names[0]) to \(names[1])" : names.first.map { "to \($0)" }, 4)
            }
            if let name = names.first, !arguments.contains(where: { $0.hasPrefix("--") || $0 == "-a" || $0 == "-r" }) {
                return make(.branch, "Created branch", name, 4)
            }
            return make(.status, "Listed branches", nil, 1)
        case "merge": return make(.branch, "Merged", names.first, 4)
        case "rebase": return make(.sync, "Rebased", names.first.map { "onto \($0)" }, 4)
        case "pull": return make(.sync, "Pulled", nil, 4)
        case "fetch": return make(.sync, "Fetched", nil, 3)
        case "add":
            if arguments.contains(where: { ["-A", "--all", "-u", "--update"].contains($0) }) || names == ["."] {
                return make(.stage, "Staged", "all changes", 3)
            }
            if names.count == 1, let name = names.first { return make(.stage, "Staged", programName(name), 3) }
            return make(.stage, "Staged", names.isEmpty ? nil : "\(names.count) files", 3)
        case "diff":
            if arguments.contains("--check") { return make(.diffCheck, "Checked the diff", "for whitespace errors", 5) }
            return make(.diff, "Read the diff", nil, 2)
        case "status": return make(.status, "Checked git status", nil, 1)
        case "log", "show", "blame", "reflog", "shortlog": return make(.history, "Read git history", nil, 2)
        case "stash": return make(.other, names.first == "pop" ? "Restored stashed changes" : "Stashed changes", nil, 3)
        case "restore": return make(.other, "Restored files", nil, 3)
        case "reset": return make(.other, "Reset changes", nil, 3)
        case "tag": return make(.other, "Tagged", names.first, 3)
        case "": return make(.other, "Ran git", nil, 1)
        default: return make(.other, "Ran git \(sub)", nil, 2)
        }
    }

    static func ghSummary(_ arguments: [String]) -> (summary: CommandSummary, rank: Int) {
        let names = positional(arguments)
        let pair = (names.first ?? "", names.count > 1 ? names[1] : "")
        let number = names.count > 2 && names[2].allSatisfy(\.isNumber) ? "#\(names[2])" : nil
        switch pair {
        case ("pr", "create"): return (CommandSummary(kind: .pullRequest, label: "Opened a pull request"), 9)
        case ("pr", "checks"), ("run", "watch"): return (CommandSummary(kind: .ciChecks, label: "Watched CI", detail: number), 8)
        case ("pr", "merge"): return (CommandSummary(kind: .merge, label: "Merged pull request", detail: number), 9)
        case ("pr", _): return (CommandSummary(kind: .status, label: "Read pull request", detail: number), 2)
        case ("issue", "create"): return (CommandSummary(kind: .other, label: "Opened an issue"), 3)
        default: return (CommandSummary(kind: .other, label: "Ran gh \(pair.0)"), 2)
        }
    }

    /// Tests, builds, type checks, lints, formatting and installs, with the
    /// tool that ran them; nil for anything else.
    static func checkSummary(program: String, arguments: [String]) -> (summary: CommandSummary, rank: Int)? {
        let names = positional(arguments)
        let sub = names.first ?? ""
        func make(_ kind: Kind, _ tag: String) -> (CommandSummary, Int) {
            let label = switch kind {
            case .test: "Ran tests"
            case .build: "Built"
            case .typecheck: "Type-checked"
            case .lint: "Linted"
            case .format: "Formatted"
            case .install: "Installed dependencies"
            default: "Ran \(tag)"
            }
            let rank = switch kind {
            case .test: 7
            case .build, .typecheck, .lint: 6
            default: 5
            }
            return (CommandSummary(kind: kind, label: label, tag: tag), rank)
        }
        /// A script or target by its name: `test:unit`, `lint-all`.
        func named(_ name: String, tag: String) -> (CommandSummary, Int)? {
            let name = name.lowercased()
            if name.contains("test") { return make(.test, tag) }
            if name.contains("lint") { return make(.lint, tag) }
            if name.contains("typecheck") || name.contains("tsc") { return make(.typecheck, tag) }
            if name.contains("build") { return make(.build, tag) }
            if name.contains("format") || name == "fmt" { return make(.format, tag) }
            if name.contains("check") { return (CommandSummary(kind: .typecheck, label: "Ran checks", tag: tag), 6) }
            return nil
        }
        let checking = arguments.contains("--check")
        switch program {
        case "cargo":
            switch sub {
            case "test", "nextest": return make(.test, "cargo")
            case "build": return make(.build, "cargo")
            case "check": return make(.typecheck, "cargo")
            case "clippy": return make(.lint, "clippy")
            case "fmt":
                return checking ? (CommandSummary(kind: .lint, label: "Checked formatting", tag: "rustfmt"), 6)
                    : make(.format, "rustfmt")
            case "install", "add", "fetch": return make(.install, "cargo")
            default: return nil
            }
        case "swift":
            switch sub {
            case "test": return make(.test, "swift")
            case "build": return make(.build, "swift")
            case "package" where names.contains("resolve"): return make(.install, "swift")
            default: return nil
            }
        case "xcodebuild":
            return names.contains("test") ? make(.test, "xcodebuild") : make(.build, "xcodebuild")
        case "go":
            switch sub {
            case "test": return make(.test, "go")
            case "build": return make(.build, "go")
            case "vet": return make(.lint, "go vet")
            default: return nil
            }
        case "npm", "pnpm", "yarn", "bun", "deno":
            if ["install", "i", "ci", "add"].contains(sub) || sub.isEmpty && program != "deno" { return make(.install, program) }
            let script = sub == "run" && names.count > 1 ? names[1] : sub
            return named(script, tag: program)
        case "make", "just", "rake", "gradle", "gradlew", "mvn":
            if sub.isEmpty { return make(.build, program) }
            return named(sub, tag: program)
        case "python", "python3":
            if let module = arguments.firstIndex(of: "-m"), module + 1 < arguments.count {
                let name = arguments[module + 1]
                if ["pytest", "unittest"].contains(name) { return make(.test, name) }
                if name == "mypy" { return make(.typecheck, name) }
                if name == "pip" { return make(.install, "pip") }
            }
            return nil
        case "pytest", "jest", "vitest", "mocha", "rspec", "phpunit", "ctest": return make(.test, program)
        case "tsc", "mypy", "pyright": return make(.typecheck, program)
        case "eslint", "ruff", "swiftlint", "shellcheck", "rubocop", "golangci-lint", "biome":
            return make(.lint, program)
        case "prettier", "black", "swift-format", "gofmt", "rustfmt":
            return checking ? (CommandSummary(kind: .lint, label: "Checked formatting", tag: program), 6)
                : make(.format, program)
        case "pip", "pip3", "brew", "uv", "poetry":
            return ["install", "add", "sync", "update", "upgrade"].contains(sub) ? make(.install, program) : nil
        default:
            // A project script: `scripts/swift-test.sh` is a test run.
            guard program != "test", program != "[" else { return nil }
            let stem = program.split(separator: ".").first.map(String.init) ?? program
            return named(stem, tag: stem)
        }
    }
}
