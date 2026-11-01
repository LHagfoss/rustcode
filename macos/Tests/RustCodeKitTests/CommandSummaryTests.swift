import XCTest
@testable import RustCodeKit

final class CommandSummaryTests: XCTestCase {
    private func line(_ command: String, omitting: Set<CommandSummary.Kind> = []) -> String {
        let summary = CommandSummary.of(command, omitting: omitting)
        return [summary.label, summary.detail, summary.tag.map { "[\($0)]" }, summary.others > 0 ? "+\(summary.others)" : nil]
            .compactMap { $0 }.joined(separator: " ")
    }

    func testACommitChainIsItsCommitAndMessage() {
        XCTAssertEqual(line("git add -- CHANGELOG.md && git commit -m 'docs(changelog): fyll inn hash' && git status --short"),
                       "Committed “docs(changelog): fyll inn hash” +1")
        XCTAssertEqual(CommandSummary.of("git commit -m x").kind, .commit)
        XCTAssertEqual(line("git commit -am \"fix: it\""), "Committed “fix: it”")
        XCTAssertEqual(line("git commit --message=wip"), "Committed “wip”")
        XCTAssertEqual(line("git commit --amend --no-edit"), "Amended the commit")
    }

    func testAHeredocMessageGivesItsFirstLine() {
        let command = "git commit -m \"$(cat <<'EOF'\nfeat(macos): add it\n\nBody line.\nEOF\n)\""
        XCTAssertEqual(CommandSummary.commitMessage(in: command), "feat(macos): add it")
    }

    func testGitStepsHaveTheirOwnWords() {
        XCTAssertEqual(line("git push -u origin feat/x"), "Pushed to feat/x")
        XCTAssertEqual(line("git push origin HEAD"), "Pushed")
        XCTAssertEqual(line("git switch -c fix/tui-thing"), "Created branch fix/tui-thing")
        XCTAssertEqual(line("git branch -m old new"), "Renamed branch old to new")
        XCTAssertEqual(line("git branch -M main"), "Renamed branch to main")
        XCTAssertEqual(line("git checkout main"), "Switched to main")
        XCTAssertEqual(line("git status --short --branch"), "Checked git status")
        XCTAssertEqual(line("git -C repo log --oneline -5"), "Read git history")
        XCTAssertEqual(line("git add -A"), "Staged all changes")
        XCTAssertEqual(line("gh pr create --fill"), "Opened a pull request")
        XCTAssertEqual(line("gh pr merge 12 --squash"), "Merged pull request #12")
        XCTAssertEqual(CommandSummary.pushTarget(in: "git push origin HEAD:main"), "main")
    }

    func testChecksNameTheirRunner() {
        XCTAssertEqual(line("cargo test --workspace"), "Ran tests [cargo]")
        XCTAssertEqual(line("cd app && npm run test:unit 2>&1 | tail -20"), "Ran tests [npm]")
        XCTAssertEqual(line("scripts/swift-test.sh CommandSummaryTests"), "Ran tests [swift-test]")
        XCTAssertEqual(line("python3 -m pytest -q"), "Ran tests [pytest]")
        XCTAssertEqual(line("swift build --package-path macos"), "Built [swift]")
        XCTAssertEqual(line("cargo fmt --all -- --check"), "Checked formatting [rustfmt]")
        XCTAssertEqual(line("cargo clippy --workspace"), "Linted [clippy]")
        XCTAssertEqual(line("npx tsc --noEmit"), "Type-checked [tsc]")
        XCTAssertEqual(line("make lint-all"), "Linted [make]")
    }

    func testWrappersAndSetupAreSetAside() {
        XCTAssertEqual(line("/bin/zsh -lc 'cd app && cargo test'"), "Ran tests [cargo]")
        XCTAssertEqual(line("RUST_LOG=debug timeout 60 cargo test"), "Ran tests [cargo]")
        XCTAssertEqual(line("sudo -E env CI=1 pytest"), "Ran tests [pytest]")
        XCTAssertEqual(line("echo \"git push\""), "Ran echo", "quoted text is no step")
        XCTAssertEqual(line("test -f Cargo.toml"), "Ran test", "the shell's test is no test run")
    }

    func testTheMostTellingCommandNamesTheChain() {
        XCTAssertEqual(line("git status --short && cargo test && git diff --stat"), "Ran tests [cargo] +1")
        XCTAssertEqual(line("git status && git diff --check && git diff -- a.md && git add -- a.md"),
                       "Checked the diff for whitespace errors +2")
    }

    func testOmittedStepsLeaveTheRestUnlessNothingIsLeft() {
        let command = "git diff --check && git add -- a.md && git commit -m 'docs: x' && git status"
        XCTAssertEqual(line(command, omitting: CommandSummary.pipelineKinds), "Checked the diff for whitespace errors +1")
        XCTAssertEqual(line("git commit -m x", omitting: CommandSummary.pipelineKinds), "Committed “x”")
    }

    func testAPipelineStepCarriesWhatItNamed() throws {
        var transcript = Transcript()
        transcript.apply(.toolCall(id: "a", title: "run_command", kind: nil, status: .inProgress,
                                   input: ["command": .string("git add -A && git commit -m 'fix: x' && git push origin feat/y")]))
        transcript.apply(.toolCallUpdate(id: "a", title: nil, status: .completed, output: nil, exitCode: 0))
        let pipeline = try XCTUnwrap(GitPipeline.of(transcript.items, cancelled: false))
        XCTAssertEqual(pipeline.steps.map(\.detail), ["fix: x", "feat/y"])
    }
}
