import { describe, expect, test } from "bun:test";
import type { SubagentInfo } from "../acp/rustcode";
import { contextPercent, planProgress, subagentLines } from "./details";
import type { TranscriptItem } from "./thread";

const tool = (over: Partial<Extract<TranscriptItem, { kind: "tool" }>>) =>
  ({
    kind: "tool",
    toolName: "Agent",
    diffs: [],
    id: "a",
    title: "Agent",
    status: "completed",
    input: { description: "look around", subagent_type: "Explore" },
    ...over,
  }) as TranscriptItem;

const agent = (over: Partial<SubagentInfo>): SubagentInfo => ({
  id: 1,
  name: "worker",
  task: "first line\nsecond line",
  agentType: "default",
  status: "running",
  activeTurn: true,
  depth: 1,
  writeAccess: false,
  toolCalls: 0,
  requests: 0,
  inputTokens: 0,
  outputTokens: 0,
  cachedInputTokens: 0,
  ...over,
});

describe("subagentLines", () => {
  test("reads a harness subagent's state from its call", () => {
    const lines = subagentLines(
      [
        tool({ id: "a", status: "in_progress" }),
        tool({ id: "b", status: "completed" }),
        tool({ id: "c", status: "failed" }),
        tool({ id: "d", status: "completed", background: "agent" }),
      ],
      [],
    );
    expect(lines.map((l) => l.state)).toEqual(["working", "done", "failed", "working"]);
  });

  test("leaves out tool calls that are not subagents", () => {
    expect(subagentLines([tool({ title: "Read", toolName: "Read", input: { path: "x" } })], [])).toEqual([]);
  });

  test("names a RustCode agent by its type and its task's first line", () => {
    const [line] = subagentLines(
      [],
      [agent({ agentType: "reviewer", status: "completed", activeTurn: false })],
    );
    expect(line).toMatchObject({ name: "reviewer", task: "first line", state: "done" });
  });

  test("a default agent keeps its own name", () => {
    expect(subagentLines([], [agent({})])[0]?.name).toBe("worker");
  });
});

test("planProgress counts finished steps and stays quiet without a plan", () => {
  expect(planProgress([])).toBeUndefined();
  expect(
    planProgress([
      { content: "a", status: "completed" },
      { content: "b", status: "in_progress" },
    ]),
  ).toBe("1 of 2 done");
});

test("contextPercent is capped and needs a window size", () => {
  expect(contextPercent(undefined)).toBeUndefined();
  expect(contextPercent({ used: 5, size: 0, estimated: false })).toBeUndefined();
  expect(contextPercent({ used: 50, size: 200, estimated: false })).toBe(25);
  expect(contextPercent({ used: 300, size: 200, estimated: false })).toBe(100);
});
