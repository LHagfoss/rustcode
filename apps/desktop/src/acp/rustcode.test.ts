import { describe, expect, test } from "bun:test";
import {
  agentActive,
  parseAgentList,
  parsePlanUsage,
  parsePullList,
  repositoryOf,
  shownWindow,
  spawnedAgentId,
  windowTitle,
} from "./rustcode";

describe("rustcode extensions", () => {
  test("agents and the spawn receipt", () => {
    const [agent] = parseAgentList({ agents: [{ id: 3, name: "agent-3", status: "running", depth: 1 }] });
    expect(agent?.status).toBe("running");
    expect(agentActive(agent!)).toBe(true);
    expect(parseAgentList({ agents: [{ id: 4, name: "x", status: "weird" }] })[0]?.status).toBe("unknown");
    expect(spawnedAgentId('{"agent_id":7,"name":"agent-7"}')).toBe(7);
    expect(spawnedAgentId("spawn_agent: subagent 2 started")).toBe(2);
    expect(spawnedAgentId("nope")).toBeUndefined();
  });

  test("plan usage shows its shortest window", () => {
    const usage = parsePlanUsage({
      source: "codex",
      primary: { usedPercent: 42, windowMinutes: 300 },
      secondary: { usedPercent: 71, windowMinutes: 10080, resetsAt: 1 },
    })!;
    expect(windowTitle(shownWindow(usage)!)).toBe("5-hour");
    expect(usage.windows[1]?.resetsAt).toBe(1000);
    expect(parsePlanUsage({})).toBeNull();
  });

  test("pull requests", () => {
    const list = parsePullList({
      list: { pulls: [{ number: 1, title: "t", checks: { failed: 1 } }], hasMore: true },
    });
    expect(list.pulls[0]?.checks.failed).toBe(1);
    expect(list.hasMore).toBe(true);
    expect(repositoryOf("https://github.com/a/b/pull/3")).toBe("a/b");
    expect(repositoryOf("https://example.com/a/b/pull/3")).toBeUndefined();
  });
});
