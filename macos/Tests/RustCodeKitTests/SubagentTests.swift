import XCTest
@testable import RustCodeKit

final class SubagentTests: XCTestCase {
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
}
