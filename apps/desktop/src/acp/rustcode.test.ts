import { describe, expect, test } from "bun:test";
import {
  agentActive,
  nextPlanRefresh,
  PLAN_REFRESH_MS,
  parseAgentList,
  parsePayloadCapture,
  parsePlanUsage,
  parsePullList,
  repositoryOf,
  resetLabel,
  shownWindow,
  spawnedAgentId,
  windowTitle,
} from "./rustcode";
import { parseUsage } from "./types";

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

  test("plan usage refreshes on the interval, or just after the soonest reset", () => {
    const now = 1_000_000;
    expect(nextPlanRefresh(null, now)).toBe(PLAN_REFRESH_MS);
    const soon = { source: "codex", windows: [{ usedPercent: 90, minutes: 300, resetsAt: now + 60_000 }] };
    expect(nextPlanRefresh(soon, now)).toBe(65_000);
    const past = { source: "codex", windows: [{ usedPercent: 90, minutes: 300, resetsAt: now - 1 }] };
    expect(nextPlanRefresh(past, now)).toBe(PLAN_REFRESH_MS);
    const now2 = { source: "codex", windows: [{ usedPercent: 90, minutes: 300, resetsAt: now + 1 }] };
    expect(nextPlanRefresh(now2, now)).toBe(5_001);
  });

  test("a window's reset reads as time left", () => {
    const now = 1_000_000;
    expect(resetLabel({ usedPercent: 1 }, now)).toBeUndefined();
    expect(resetLabel({ usedPercent: 1, resetsAt: now + 30 * 60_000 }, now)).toBe("resets in 30 min");
    expect(resetLabel({ usedPercent: 1, resetsAt: now + 134 * 60_000 }, now)).toBe("resets in 2 h 14 min");
    expect(resetLabel({ usedPercent: 1, resetsAt: now + 120 * 60_000 }, now)).toBe("resets in 2 h");
    expect(resetLabel({ usedPercent: 1, resetsAt: now - 1 }, now)).toBe("resetting now");
  });

  test("the payload switches read as off unless the engine says on", () => {
    expect(parsePayloadCapture({ providerPayloads: true, harnessRpcPayloads: false })).toEqual({
      providerPayloads: true,
      harnessRpcPayloads: false,
    });
    // An engine that does not know the method yet, or answers nothing: capture off.
    expect(parsePayloadCapture(undefined)).toEqual({ providerPayloads: false, harnessRpcPayloads: false });
    expect(parsePayloadCapture({})).toEqual({ providerPayloads: false, harnessRpcPayloads: false });
  });

  test("a usage update carries the context breakdown", () => {
    const usage = parseUsage({
      used: 18_400,
      size: 272_000,
      _meta: {
        rustcode: {
          context: {
            model: "m",
            free: 253_600,
            categories: [
              {
                name: "Tool calls",
                tokens: 5_100,
                items: [{ name: "read src/ui/Tool.tsx", tokens: 2_200 }],
              },
              { name: "bad" },
            ],
            compactAt: { tokens: 199_000 },
            compaction: { count: 2, reclaimedTokens: 40_000 },
          },
        },
      },
    });
    expect(usage?.context).toEqual({
      model: "m",
      categories: [
        {
          name: "Tool calls",
          tokens: 5_100,
          items: [{ name: "read src/ui/Tool.tsx", tokens: 2_200 }],
        },
      ],
      free: 253_600,
      compactions: 2,
      reclaimed: 40_000,
      compactAt: 199_000,
    });
    expect(parseUsage({ used: 1, size: 2 })?.context).toBeUndefined();
  });
});
