import XCTest
@testable import RustCodeKit

final class TaskPlanTests: XCTestCase {
    private let answer: JSONValue = ["steps": [
        ["id": "t1", "content": "Read the parser", "status": "completed", "priority": "high"],
        ["id": "t2", "content": "Fix nesting", "status": "in_progress", "priority": "high"],
        ["id": "t3", "content": "Add a fixture", "status": "blocked", "priority": "medium", "blockedReason": "needs a sample file"],
        ["id": "t4", "content": "Ship it", "status": "pending"],
        ["id": "t5", "content": "Future", "status": "someday"],
        ["content": "no id is skipped"],
    ]]

    func testThePlanReadsEachStepAsTheAgentMarkedIt() {
        let plan = TaskPlan(result: answer)
        XCTAssertEqual(plan.steps.map(\.id), ["t1", "t2", "t3", "t4", "t5"])
        XCTAssertEqual(plan.steps.map(\.status), [.completed, .inProgress, .blocked, .pending, .unknown])
        XCTAssertEqual(plan.steps[2].blockedReason, "needs a sample file")
        XCTAssertEqual(plan.current?.id, "t2")
        XCTAssertEqual(plan.progressLabel, "1/5")
        XCTAssertTrue(plan.isShown)
    }

    func testAnEmptyOrOneStepPlanShowsNoButton() {
        XCTAssertTrue(TaskPlan(result: ["steps": []]).isEmpty)
        XCTAssertFalse(TaskPlan(result: ["steps": []]).isShown)
        XCTAssertFalse(TaskPlan(result: ["steps": [["id": "t1", "content": "one", "status": "pending"]]]).isShown)
        XCTAssertTrue(TaskPlan(result: [:]).isEmpty, "an engine without a plan answers nothing")
    }

    func testAStepInProgressAfterTheTurnEndedWasInterrupted() {
        let step = TaskPlan.Step(id: "t1", content: "x", status: .inProgress)
        XCTAssertTrue(TaskPlan.isInterrupted(step, turnRunning: false))
        XCTAssertFalse(TaskPlan.isInterrupted(step, turnRunning: true))
        XCTAssertFalse(TaskPlan.isInterrupted(TaskPlan.Step(id: "t2", content: "y", status: .pending), turnRunning: false))
    }

    func testEachStepLinksToTheCallThatLastWroteIt() {
        var transcript = Transcript()
        transcript.apply(.toolCall(id: "a", title: "todo_write", kind: nil, status: .completed,
                                   input: ["todos": [["id": "t1", "content": "Read the parser"], ["id": "t2", "content": "Fix nesting"]]]))
        transcript.apply(.toolCall(id: "b", title: "run_command", kind: nil, status: .completed, input: ["command": "ls"]))
        transcript.apply(.toolCall(id: "c", title: "todo_write", kind: nil, status: .completed,
                                   input: ["merge": true, "todos": [["id": "t2", "status": "completed"]]]))
        transcript.apply(.toolCall(id: "d", title: "todo_write", kind: nil, status: .inProgress, input: nil))
        let ids = transcript.items.map(\.id)
        let plan = TaskPlan(result: answer)
        let updates = plan.lastUpdates(in: transcript.items)
        XCTAssertEqual(updates["t1"], ids[0])
        XCTAssertEqual(updates["t2"], ids[2], "the later call that named it")
        XCTAssertNil(updates["t4"], "never written in what loaded")
        XCTAssertEqual(TaskPlan.latestUpdate(in: transcript.items), ids[3])
        XCTAssertEqual(TaskPlan.finishedUpdates(in: transcript.items), 2)
    }
}
