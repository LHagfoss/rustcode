import XCTest
@testable import RustCodeKit

final class SubagentTests: XCTestCase {
    func testSidebarNestsWorkingAgentsAndTheOneOnScreen() {
        let agents = SubagentInfo.list(from: [
            "agents": [
                ["id": 1, "name": "running", "status": "running"],
                ["id": 2, "name": "queued", "status": "queued"],
                ["id": 3, "name": "done", "status": "completed"],
                ["id": 4, "name": "failed", "status": "failed"],
                ["id": 5, "name": "follow-up", "status": "completed", "activeTurn": true],
                ["id": 6, "name": "cancelled", "status": "cancelled"],
            ],
        ])
        XCTAssertEqual(SubagentInfo.sidebarChildren(agents, viewing: nil).map(\.id), [1, 2, 5])
        // A finished agent stays while its conversation is open, then leaves.
        XCTAssertEqual(SubagentInfo.sidebarChildren(agents, viewing: 4).map(\.id), [1, 2, 4, 5])
        XCTAssertEqual(SubagentInfo.sidebarChildren([], viewing: 4).map(\.id), [])
    }

    /// #2596: an agent seen working keeps its row under its chat once it
    /// ends, so its result can still be opened from there.
    func testAnAgentSeenWorkingKeepsItsRowOnceItEnds() {
        let agents = SubagentInfo.list(from: ["agents": [
            ["id": 1, "name": "running", "status": "running"],
            ["id": 3, "name": "done", "status": "completed"],
            ["id": 4, "name": "earlier", "status": "completed"],
        ]])
        XCTAssertEqual(SubagentInfo.sidebarChildren(agents, viewing: nil, seen: [3]).map(\.id), [1, 3])
        XCTAssertEqual(SubagentInfo.sidebarChildren(agents, viewing: 4, seen: [3]).map(\.id), [1, 3, 4])
    }

    /// #2596: the open agent gives the screen back to its chat only when it
    /// goes from working to done, failed or cancelled; one opened after it
    /// ended, to read its result, stays open.
    func testOnlyAnAgentEndingWhileReadReturnsToItsChat() {
        func agent(_ id: Int, _ status: String, activeTurn: Bool = false, completion: String? = nil) -> SubagentInfo {
            let json: JSONValue = ["id": .number(Double(id)), "name": .string("agent-\(id)"), "status": .string(status),
                                   "activeTurn": .bool(activeTurn), "completion": completion.map(JSONValue.string) ?? .null]
            return SubagentInfo(json: json)!
        }
        let running = agent(1, "running")
        XCTAssertTrue(SubagentInfo.endsViewing(was: running, now: agent(1, "completed")))
        XCTAssertTrue(SubagentInfo.endsViewing(was: running, now: agent(1, "failed")))
        XCTAssertTrue(SubagentInfo.endsViewing(was: agent(1, "cancelling"), now: agent(1, "cancelled")))
        // A follow-up turn on a finished child that ends.
        XCTAssertTrue(SubagentInfo.endsViewing(was: agent(1, "completed", activeTurn: true), now: agent(1, "completed")))
        XCTAssertFalse(SubagentInfo.endsViewing(was: running, now: running))
        XCTAssertFalse(SubagentInfo.endsViewing(was: running, now: agent(1, "interrupted")))
        XCTAssertFalse(SubagentInfo.endsViewing(was: agent(1, "completed"), now: agent(1, "completed")),
                       "opened to read its result")
        XCTAssertFalse(SubagentInfo.endsViewing(was: running, now: agent(2, "completed")), "another agent")

        XCTAssertEqual(agent(1, "completed", completion: "All 214 tests pass.\nMore").endedMessage,
                       "agent-1 finished: All 214 tests pass.")
        XCTAssertEqual(agent(1, "failed").endedMessage, "agent-1 failed")
        XCTAssertEqual(agent(1, "cancelled").endedMessage, "agent-1 was cancelled")
    }

    /// #2596: the chat's row and its agent's are never selected together.
    func testTheChatAndItsAgentAreNotBothSelected() {
        XCTAssertTrue(SubagentInfo.chatRowSelected("a", selected: "a", viewingAgent: nil))
        XCTAssertFalse(SubagentInfo.chatRowSelected("a", selected: "a", viewingAgent: 2))
        XCTAssertFalse(SubagentInfo.chatRowSelected("b", selected: "a", viewingAgent: nil))
    }

    /// A status a newer engine sends is shown as unknown, not as queued,
    /// so the chat does not keep polling a child that may have ended.
    func testUnknownStatusIsNotActive() {
        let agents = SubagentInfo.list(from: ["agents": [["id": 1, "name": "new", "status": "paused"]]])
        XCTAssertEqual(agents.first?.status, .unknown)
        XCTAssertEqual(agents.first?.isActive, false)
    }

    /// #2556: a chat's working agents stay listed after another chat is
    /// opened, take their final state, and go back once it is reopened.
    func testBackgroundAgentsOutliveTheChatSwitch() {
        func list(_ agents: [JSONValue]) -> [SubagentInfo] { SubagentInfo.list(from: ["agents": .array(agents)]) }
        var background = BackgroundAgents()
        background.leave("a", agents: list([["id": 1, "name": "working", "status": "running"],
                                            ["id": 2, "name": "done", "status": "completed"]]))
        background.leave("b", agents: list([["id": 1, "name": "done", "status": "completed"]]))
        XCTAssertEqual(background.agents(of: "a").map(\.id), [1])
        XCTAssertEqual(background.agents(of: "b").map(\.id), [])
        XCTAssertEqual(background.working, ["a"])

        // A new child joins; the listed one finishes and keeps its row.
        background.update("a", agents: list([["id": 1, "name": "working", "status": "completed"],
                                             ["id": 2, "name": "done", "status": "completed"],
                                             ["id": 3, "name": "new", "status": "queued"]]))
        XCTAssertEqual(background.agents(of: "a").map(\.id), [1, 3])
        XCTAssertEqual(background.agents(of: "a").first?.status, .completed)
        XCTAssertEqual(background.working, ["a"])
        background.update("a", agents: list([["id": 1, "name": "working", "status": "completed"],
                                             ["id": 3, "name": "new", "status": "failed"]]))
        XCTAssertEqual(background.agents(of: "a").map(\.status), [.completed, .failed])
        XCTAssertEqual(background.working, [])

        XCTAssertEqual(background.take("a").map(\.id), [1, 3])
        XCTAssertEqual(background.agents(of: "a").map(\.id), [])
        XCTAssertEqual(background, BackgroundAgents())
    }

    func testParsesTheAgentsList() {
        let agents = SubagentInfo.list(from: [
            "agents": [
                ["id": 1, "name": "agent-1", "task": "find the bug", "model": "small", "agentType": "explorer",
                 "status": "running", "activeTurn": true, "parentId": .null, "depth": 1, "writeAccess": false,
                 "messageCount": 4, "startedAtMs": 1_000, "finishedAtMs": .null, "completion": .null],
                ["id": 2, "name": "agent-2", "status": "completed", "startedAtMs": 1_000, "finishedAtMs": 66_000,
                 "completion": "Fixed parse()."],
                ["name": "no id"],
            ],
        ])
        XCTAssertEqual(agents.map(\.id), [1, 2])
        XCTAssertEqual(agents[0].agentType, "explorer")
        XCTAssertEqual(agents[0].status, .running)
        XCTAssertTrue(agents[0].isActive)
        XCTAssertEqual(agents[0].messageCount, 4)
        XCTAssertNil(agents[0].parentId)
        XCTAssertFalse(agents[1].isActive)
        XCTAssertEqual(agents[1].status.label, "Done")
        XCTAssertEqual(agents[1].elapsed(), "1m 05s")
        XCTAssertEqual(agents[1].completion, "Fixed parse().")
    }

    func testLifecycleStatesAndQuietAgents() {
        let agents = SubagentInfo.list(from: [
            "agents": [
                ["id": 1, "name": "stopping", "status": "cancelling", "startedAtMs": 1_000, "lastActivityMs": 2_000],
                ["id": 2, "name": "stuck", "status": "running", "startedAtMs": 1_000, "lastActivityMs": 2_000,
                 "toolCalls": 0],
                ["id": 3, "name": "busy", "status": "running", "startedAtMs": 1_000, "lastActivityMs": 890_000,
                 "toolCalls": 7],
                ["id": 4, "name": "stopped", "status": "cancelled", "startedAtMs": 1_000, "lastActivityMs": 2_000],
            ],
        ])
        let now = Date(timeIntervalSince1970: 902)
        XCTAssertEqual(agents[0].status, .cancelling)
        XCTAssertTrue(agents[0].isActive, "a cancelling agent is still polled until it ends")
        XCTAssertEqual(agents[0].status.label, "Cancelling…")
        XCTAssertEqual(agents[1].quietLabel(now: now), "No activity for 15m")
        XCTAssertEqual(agents[1].toolCalls, 0)
        XCTAssertNil(agents[2].quietLabel(now: now), "recent progress is not called quiet")
        XCTAssertEqual(agents[2].toolCalls, 7)
        XCTAssertNil(agents[3].quietLabel(now: now), "an ended agent is not quiet, it is done")
        XCTAssertFalse(agents[3].isActive)
    }

    func testCancelAnswerSaysWhetherTheAgentEnded() {
        XCTAssertTrue(SubagentInfo.cancelSettled(["message": "subagent 1 cancelled", "settled": true]))
        XCTAssertFalse(SubagentInfo.cancelSettled(["message": "subagent 1 stopping", "settled": false]))
        XCTAssertTrue(SubagentInfo.cancelSettled(["message": "subagent 1 cancelled"]))
        XCTAssertFalse(SubagentInfo.cancelSettled([
            "message": "cancellation requested for subagent 1; it is still cancelling after 3s. Use wait_agent for its terminal result",
        ]))
    }

    func testHistoryBecomesATranscript() {
        let transcript = SubagentInfo.transcript(from: [
            "updates": [
                ["sessionUpdate": "user_message_chunk", "content": ["type": "text", "text": "find the bug"]],
                ["sessionUpdate": "agent_message_chunk", "content": ["type": "text", "text": "It is in parse()."]],
            ],
        ])
        XCTAssertEqual(transcript.items.map(\.content), [.user("find the bug"), .assistant("It is in parse().")])
    }

    func testRequestsNameTheirAction() {
        XCTAssertEqual(ACPRequests.agents(sessionId: "s")["action"], "list")
        let send = ACPRequests.agentSend(sessionId: "s", id: 3, text: "go on")
        XCTAssertEqual(send["action"], "send")
        XCTAssertEqual(send["id"]?.intValue, 3)
        XCTAssertEqual(send["text"], "go on")
        XCTAssertEqual(ACPRequests.agentCancel(sessionId: "s", id: 3)["action"], "cancel")
        XCTAssertEqual(ACPRequests.agentHistory(sessionId: "s", id: 3)["action"], "history")
    }

    func testASpawnNamesItsChild() {
        XCTAssertEqual(SubagentInfo.spawnedId(in: #"{"agent_id":3,"nickname":"agent-3","status":"queued"}"#), 3)
        XCTAssertEqual(SubagentInfo.spawnedId(in: "{\"agent_id\": 12}\n\n__BUILD_UNVERIFIED__: note"), 12)
        XCTAssertEqual(SubagentInfo.spawnedId(in: "subagent 6 started; use wait_agent to receive its terminal result"), 6)
        // A session replay sends the receipt as history stored it.
        XCTAssertEqual(SubagentInfo.spawnedId(in: #"spawn_agent: {"agent_id":3,"nickname":"agent-3"}"#), 3)
        XCTAssertNil(SubagentInfo.spawnedId(in: #"read_file: {"agent_id":3}"#))
        XCTAssertNil(SubagentInfo.spawnedId(in: "error: unknown model 'x'"))
        XCTAssertNil(SubagentInfo.spawnedId(in: nil))
    }

    func testThePillCountsOnlyWorkingAgents() {
        let agents = SubagentInfo.list(from: ["agents": [
            ["id": 1, "name": "a", "status": "running"],
            ["id": 2, "name": "b", "status": "queued"],
            ["id": 3, "name": "c", "status": "completed"],
            ["id": 4, "name": "d", "status": "completed", "activeTurn": true],
        ]])
        XCTAssertEqual(SubagentInfo.workingLabel(agents), "3 agents working")
        XCTAssertEqual(SubagentInfo.workingLabel(Array(agents.prefix(1))), "1 agent working")
        XCTAssertNil(SubagentInfo.workingLabel(Array(agents[2...2])), "no pill once all are done")
        XCTAssertNil(SubagentInfo.workingLabel([]))
    }

    /// A replayed or live spawn card becomes the row of the child it started.
    func testSpawnRowsFollowTheirAgents() {
        var transcript = Transcript()
        transcript.apply(.toolCall(id: "c1", title: "spawn_agent", kind: nil, status: .pending,
                                   input: ["task": "Check the latest PRs\nand report"]))
        transcript.apply(.toolCall(id: "c2", title: "spawn_agent", kind: nil, status: .inProgress, input: nil,
                                   detail: "Read the docs"))
        transcript.apply(.toolCall(id: "c3", title: "spawn_agent", kind: nil, status: .pending, input: ["task": "x"]))
        transcript.apply(.toolCallUpdate(id: "c1", title: nil, status: .completed,
                                         output: #"{"agent_id":1,"nickname":"agent-1","status":"queued"}"#))
        transcript.apply(.toolCallUpdate(id: "c3", title: nil, status: .failed, output: "error: unknown model 'x'"))
        let agents = SubagentInfo.list(from: ["agents": [
            ["id": 1, "name": "agent-1", "task": "Check the latest PRs\nand report", "agentType": "explorer",
             "status": "running", "startedAtMs": 1_000],
        ]])
        let rows = AgentSpawn.rows(transcript.items.map(\.id), in: transcript.items, agents: agents)
        XCTAssertEqual(rows.map(\.agentId), [1, nil, nil])
        XCTAssertEqual(rows.map(\.name), ["agent-1", "Starting agent…", "Agent did not start"])
        XCTAssertEqual(rows.map(\.task), ["Check the latest PRs", "Read the docs", "x"])
        XCTAssertEqual(rows[0].info?.status, .running)
        XCTAssertEqual(rows[2].failure, "error: unknown model 'x'")
        XCTAssertEqual(rows[0].accessibilityLabel(now: Date(timeIntervalSince1970: 13)),
                       "agent-1, explorer, working, 12s, Check the latest PRs")
        // Before the agents list has it, the row names the child by its id.
        XCTAssertEqual(AgentSpawn.rows([transcript.items[0].id], in: transcript.items, agents: [])[0].name, "agent-1")
    }
}
