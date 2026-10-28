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
