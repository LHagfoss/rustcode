import XCTest
@testable import RustCodeKit

final class GitPipelineTests: XCTestCase {
    private func run(_ transcript: inout Transcript, _ id: String, _ command: String, _ status: ToolStatus?,
                     exit: Int? = nil, output: String? = nil) {
        transcript.apply(.toolCall(id: id, title: "run_command", kind: nil, status: .inProgress, input: ["command": .string(command)]))
        if let status {
            transcript.apply(.toolCallUpdate(id: id, title: nil, status: status, output: output, exitCode: exit))
        }
    }

    func testAChainIsEachOfItsStepsAndQuotedTextIsNoStep() {
        XCTAssertEqual(GitPipeline.kinds(in: "git add -A && git commit -m 'fix: push it' && git push -u origin HEAD"),
                       [.commit, .push])
        XCTAssertEqual(GitPipeline.kinds(in: "cd app; gh pr create --fill"), [.pullRequest])
        XCTAssertEqual(GitPipeline.kinds(in: "gh pr checks 12 --watch || true"), [.checks])
        XCTAssertEqual(GitPipeline.kinds(in: "git status && git log"), [])
        XCTAssertEqual(GitPipeline.kinds(in: "echo \"git push\""), [])
    }

    func testOutcomesComeFromTheCommandsExitNotItsText() throws {
        var transcript = Transcript()
        run(&transcript, "a", "git commit -m x", .completed, exit: 0)
        run(&transcript, "b", "git push", .failed, exit: 1, output: "Everything up-to-date")
        run(&transcript, "c", "gh pr create --fill", .completed, exit: 0,
            output: "Creating pull request\nhttps://github.com/o/r/pull/42\n")
        run(&transcript, "d", "gh pr checks 42 --watch", .failed, exit: 8)
        let pipeline = try XCTUnwrap(GitPipeline.of(transcript.items, cancelled: false))
        XCTAssertEqual(pipeline.steps.map(\.kind), [.commit, .push, .pullRequest, .checks])
        XCTAssertEqual(pipeline.steps.map(\.outcome), [.succeeded, .failed, .succeeded, .pending])
        XCTAssertEqual(pipeline.steps[2].link?.absoluteString, "https://github.com/o/r/pull/42")
        XCTAssertEqual(pipeline.steps[1].item, transcript.items[1].id)
    }

    func testAFailedChainDoesNotSayWhichStepFailed() throws {
        var transcript = Transcript()
        run(&transcript, "a", "git commit -m x && git push", .failed, exit: 1)
        let pipeline = try XCTUnwrap(GitPipeline.of(transcript.items, cancelled: false))
        XCTAssertEqual(pipeline.steps.map(\.outcome), [.unknown, .unknown])
    }

    func testARetriedStepKeepsItsLatestRunAndARunningOneIsRunning() throws {
        var transcript = Transcript()
        run(&transcript, "a", "git push", .failed, exit: 1)
        run(&transcript, "b", "git push --force-with-lease", .completed, exit: 0)
        run(&transcript, "c", "gh pr checks --watch", nil)
        let pipeline = try XCTUnwrap(GitPipeline.of(transcript.items, cancelled: false))
        XCTAssertEqual(pipeline.steps.map(\.kind), [.push, .checks])
        XCTAssertEqual(pipeline.steps.map(\.outcome), [.succeeded, .running])
        let stopped = try XCTUnwrap(GitPipeline.of(transcript.items, cancelled: true))
        XCTAssertEqual(stopped.steps.last?.outcome, .cancelled)
        XCTAssertNil(GitPipeline.of([], cancelled: false))
    }

    func testTheTurnSummaryCarriesThePipelineAndDoesNotCountWatchingCIAsACheck() throws {
        var transcript = Transcript()
        transcript.appendUser("ship it")
        run(&transcript, "a", "git push", .completed, exit: 0)
        run(&transcript, "b", "gh pr checks --watch", .completed, exit: 0)
        run(&transcript, "c", "git commit -m y", .failed, exit: 1)
        let completion = try XCTUnwrap(TurnCompletion.of(transcript.items.dropFirst()[...], cancelled: false))
        XCTAssertEqual(completion.checks, [])
        XCTAssertEqual(completion.failedCommands, 0, "a failed pipeline step is shown there, not counted again")
        XCTAssertEqual(completion.pipeline?.steps.map(\.kind), [.push, .checks, .commit])
    }
}
