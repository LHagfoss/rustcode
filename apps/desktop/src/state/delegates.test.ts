// The delegate tree's rules: how a branch's clock counts (work only, never a
// wait on the user), what a node is doing, who sits under whom, and how a
// branch's cost and time add up.

import { describe, expect, test } from "bun:test";
import {
  agentStateOf,
  branchBar,
  branchWord,
  costLabel,
  type DelegateRead,
  delegateCount,
  delegateTree,
  delegateViews,
  elapsedOf,
  flattenViews,
  forestTotal,
  runningAgents,
  runningSubagents,
  timeLabel,
} from "./delegates";
import type { Link } from "./orchestration";
import type { TranscriptItem } from "./thread";

const MINUTE = 60_000;

const link = (orchestrator: string, target: string, extra: Partial<Link> = {}): Link => ({
  id: `${orchestrator}/${target}`,
  orchestrator,
  target,
  orchestratorTitle: orchestrator,
  targetTitle: target,
  message: `Ask ${target} to help`,
  state: "working",
  started: true,
  createdAt: 0,
  ...extra,
});

describe("a branch's clock", () => {
  test("counts up while the thread works", () => {
    expect(elapsedOf({ startedAt: 0 }, 90_000)).toEqual({ ms: 90_000, live: true, frozen: false });
  });

  test("holds still while it waits on the user", () => {
    // It worked a minute, then asked; ten minutes later the row still says a minute.
    expect(elapsedOf({ startedAt: 0, waitingSince: MINUTE }, 11 * MINUTE)).toEqual({
      ms: MINUTE,
      live: false,
      frozen: true,
    });
    // A wait already over is folded into `waitedMs`, so the clock resumes without it.
    expect(elapsedOf({ startedAt: 0, waitedMs: 2 * MINUTE }, 3 * MINUTE)).toEqual({
      ms: MINUTE,
      live: true,
      frozen: false,
    });
  });

  test("leaves out every wait, including one it was in when it settled", () => {
    // It waited once before, and was waiting again when it stopped: neither counts.
    expect(
      elapsedOf(
        { startedAt: 0, endedAt: 5 * MINUTE, waitedMs: MINUTE, waitingSince: 4 * MINUTE },
        9 * MINUTE,
      ),
    ).toEqual({ ms: 3 * MINUTE, live: false, frozen: false });
  });

  test("never counts below zero", () => {
    expect(elapsedOf({ startedAt: MINUTE, waitingSince: 0 }, MINUTE)).toMatchObject({ ms: 0 });
    expect(elapsedOf({ startedAt: 0, endedAt: 0 }, MINUTE)).toMatchObject({ ms: 0 });
  });
});

describe("what a node is doing", () => {
  test("the thread's live status wins over the link's record", () => {
    expect(agentStateOf(link("a", "b", { state: "done" }), "running")).toBe("working");
    expect(agentStateOf(link("a", "b", { state: "working" }), "needs_input")).toBe("needs");
  });

  test("a failed or stopped branch keeps its word", () => {
    expect(agentStateOf(link("a", "b", { state: "failed" }), "running")).toBe("failed");
    expect(agentStateOf(link("a", "b", { state: "cancelled" }), "running")).toBe("stopped");
  });

  test("a link a restart could not resume takes the thread's own status", () => {
    expect(agentStateOf(link("a", "b", { state: "interrupted" }), "running")).toBe("working");
    expect(agentStateOf(link("a", "b", { state: "interrupted" }), "needs_input")).toBe("needs");
    expect(agentStateOf(link("a", "b", { state: "interrupted" }))).toBe("stopped");
  });

  test("a queued or waiting link reads as the app shows it", () => {
    expect(agentStateOf(link("a", "b", { state: "queued" }))).toBe("working");
    expect(agentStateOf(link("a", "b", { state: "needs_user" }))).toBe("needs");
  });
});

describe("who sits under whom", () => {
  test("the threads it started, and the threads those started", () => {
    const links = [
      link("root", "research", { createdAt: 100 }),
      link("research", "tests", { createdAt: 200 }),
      link("research", "docs", { createdAt: 300 }),
      link("root", "other", { createdAt: 400 }),
    ];
    const tree = delegateTree("root", links);
    expect(tree.map((node) => node.id)).toEqual(["research", "other"]);
    expect(tree[0]!.children.map((node) => node.id)).toEqual(["tests", "docs"]);
    expect(delegateCount(tree)).toBe(4);
  });

  test("a thread it only messaged is not a delegate", () => {
    const links = [link("root", "friend", { started: undefined }), link("root", "child")];
    expect(delegateTree("root", links).map((node) => node.id)).toEqual(["child"]);
  });

  test("one node per thread, however many times it was messaged", () => {
    const links = [
      link("root", "child", { id: "root/child", createdAt: 100 }),
      link("root", "child", { id: "root/child#2", createdAt: 200, state: "done" }),
    ];
    const tree = delegateTree("root", links);
    expect(tree).toHaveLength(1);
    expect(tree[0]!.messages).toBe(2);
    // The live record speaks for the relation, not the newer settled one.
    expect(tree[0]!.link.id).toBe("root/child");
  });

  test("stops at the depth delegation itself stops at", () => {
    const links = [link("a", "b"), link("b", "c"), link("c", "d")];
    const tree = delegateTree("a", links);
    expect(tree[0]!.id).toBe("b");
    expect(tree[0]!.children[0]!.id).toBe("c");
    expect(tree[0]!.children[0]!.children).toEqual([]);
  });

  test("a broken book that loops back to the top draws each thread once", () => {
    const links = [link("a", "b"), link("b", "a")];
    const tree = delegateTree("a", links);
    expect(tree.map((node) => node.id)).toEqual(["b"]);
    expect(tree[0]!.children).toEqual([]);
  });
});

describe("a branch's cost and time", () => {
  const links = [
    link("root", "research", { createdAt: 0 }),
    link("research", "tests", { createdAt: 0, state: "done", settledAt: 2 * MINUTE }),
  ];
  const costs = new Map([
    ["research", { usd: 2, reportedUsd: 2, unknownTurns: 0 }],
    ["tests", { usd: 0.5, estimatedUsd: 0.5, unknownTurns: 0 }],
  ]);
  const read = (over: Partial<DelegateRead> = {}): DelegateRead => ({
    now: 4 * MINUTE,
    title: (_id, link) => link.targetTitle,
    harness: () => "codex",
    activity: (id) => (id === "research" ? "running" : undefined),
    waitingSince: () => undefined,
    cost: (id) => costs.get(id),
    ...over,
  });

  test("a parent adds up its own figures and every thread under it", () => {
    const tree = delegateViews(delegateTree("root", links), read());
    const research = tree[0]!;
    expect(research.state).toBe("working");
    expect(research.total.threads).toBe(2);
    expect(research.total.usd).toBe(2.5);
    // One part is a list-price estimate, so the branch's total says so.
    expect(research.total.estimated).toBe(true);
    expect(research.total.counts).toEqual({ working: 1, needs: 0, done: 1, failed: 0, stopped: 0 });
    expect(research.total.ms).toBe(6 * MINUTE);
  });

  test("an unknown cost stays unknown, never zero", () => {
    const tree = delegateViews(delegateTree("root", links), read({ cost: () => undefined }));
    expect(tree[0]!.total.usd).toBeUndefined();
    expect(costLabel(tree[0]!.total.usd, tree[0]!.total.estimated)).toBe("—");
  });

  test("the forest sums what a Board card shows", () => {
    const tree = delegateViews(delegateTree("root", links), read());
    const total = forestTotal(tree);
    expect(total.threads).toBe(2);
    expect(total.usd).toBe(2.5);
    expect(branchWord(total)).toBe("1 working");
  });

  test("a finished branch is finished, a waiting one is not", () => {
    const allDone = [link("root", "child", { state: "done", settledAt: MINUTE })];
    const done = delegateViews(delegateTree("root", allDone), read());
    expect(done[0]!.total.finished).toBe(true);
    expect(branchWord(done[0]!.total)).toBe("done");
    const waiting = delegateViews(delegateTree("root", links), read({ activity: () => "needs_input" }));
    expect(waiting[0]!.total.finished).toBe(false);
    expect(branchWord(waiting[0]!.total)).toBe("2 need you");
  });

  test("what needs you is drawn first, then what works, then what ended", () => {
    const mixed = [
      link("root", "finished", { createdAt: 300, state: "done", settledAt: MINUTE }),
      link("root", "busy", { createdAt: 200 }),
      link("root", "asking", { createdAt: 100 }),
    ];
    const views = delegateViews(
      delegateTree("root", mixed),
      read({ activity: (id) => (id === "asking" ? "needs_input" : id === "busy" ? "running" : undefined) }),
    );
    expect(views.map((view) => [view.id, view.state])).toEqual([
      ["asking", "needs"],
      ["busy", "working"],
      ["finished", "done"],
    ]);
    // Two rows in one state: the newer one first, as the sidebar lists threads.
    const done = [
      link("root", "older", { createdAt: 100, state: "done", settledAt: MINUTE }),
      link("root", "newer", { createdAt: 200, state: "done", settledAt: MINUTE }),
    ];
    expect(
      delegateViews(delegateTree("root", done), read({ activity: () => undefined })).map((view) => view.id),
    ).toEqual(["newer", "older"]);
  });

  test("the flat list keeps parents before their children", () => {
    const tree = delegateViews(delegateTree("root", links), read());
    expect(flattenViews(tree).map((view) => view.id)).toEqual(["research", "tests"]);
  });
});

describe("the strip above the composer", () => {
  const links = [
    link("root", "longest", { createdAt: 0 }),
    link("root", "second", { createdAt: 4 * MINUTE }),
    link("root", "asking", { createdAt: 5 * MINUTE, state: "needs_user", waitingSince: 6 * MINUTE }),
    link("root", "finished", { createdAt: 1 * MINUTE, state: "done", settledAt: 3 * MINUTE }),
    link("root", "older", { createdAt: 0, state: "done", settledAt: 2 * MINUTE }),
  ];
  const read = (over: Partial<DelegateRead> = {}): DelegateRead => ({
    now: 20 * MINUTE,
    title: (_id, link) => link.targetTitle,
    harness: () => "codex",
    activity: (id) => (id === "asking" ? "needs_input" : undefined),
    waitingSince: (_id, link) => link.waitingSince,
    cost: () => undefined,
    ...over,
  });
  const total = (over: Partial<DelegateRead> = {}) =>
    forestTotal(delegateViews(delegateTree("root", links), read(over)));
  // The longest working thread started at 0, the second at 4m.
  const starts = [0, 4 * MINUTE];

  test("counts live work, not every delegate ever started", () => {
    const bar = branchBar(total(), starts, 20 * MINUTE);
    expect(bar.working).toBe(2);
    expect(bar.needs).toBe(1);
    expect(bar.live).toBe(3);
    // The two finished delegates are history: the header counts what runs now.
    expect(bar.label).toBe("2 running");
    expect(bar.status).toBe("1 needs you");
    expect(bar.show).toBe(true);
  });

  test("the time is the longest working thread's wall clock, never a sum", () => {
    // The branch's own summed work time is far larger; the strip reads the clock.
    expect(total().ms).toBeGreaterThan(20 * MINUTE);
    expect(branchBar(total(), starts, 20 * MINUTE).longestMs).toBe(20 * MINUTE);
    // The clock reads from the earliest of the working threads, so the newest start never wins.
    expect(branchBar(total(), [4 * MINUTE, 10 * MINUTE], 20 * MINUTE).longestMs).toBe(16 * MINUTE);
  });

  test("a thread only waiting on the user says that alone", () => {
    const asking = [
      link("root", "one", { state: "needs_user", waitingSince: MINUTE }),
      link("root", "two", { state: "needs_user", waitingSince: 2 * MINUTE }),
    ];
    const bar = branchBar(
      forestTotal(delegateViews(delegateTree("root", asking), read({ activity: () => undefined }))),
      [],
      3 * MINUTE,
    );
    expect(bar.working).toBe(0);
    expect(bar.needs).toBe(2);
    expect(bar.label).toBe("2 need you");
    expect(bar.status).toBe("");
    expect(bar.state).toBe("needs");
    expect(bar.show).toBe(true);
  });

  test("a branch that only finished has nothing to show, however long it ran", () => {
    const done = [
      link("root", "one", { state: "done", settledAt: MINUTE }),
      link("root", "two", { state: "done", settledAt: 2 * MINUTE }),
      link("root", "three", { state: "done", settledAt: 3 * MINUTE }),
    ];
    const bar = branchBar(
      forestTotal(delegateViews(delegateTree("root", done), read({ activity: () => undefined }))),
      [],
      60 * MINUTE,
    );
    expect(bar.working).toBe(0);
    expect(bar.live).toBe(0);
    expect(bar.longestMs).toBeUndefined();
    expect(bar.label).toBe("");
    expect(bar.status).toBe("");
    expect(bar.show).toBe(false);
    // A stopped delegate is the user's own doing, and history too.
    const stopped = branchBar(
      forestTotal(
        delegateViews(delegateTree("root", [link("root", "stopped", { state: "cancelled" })]), read()),
      ),
      [],
      2 * MINUTE,
    );
    expect(stopped.show).toBe(false);
    expect(stopped.label).toBe("");
  });

  test("a failure that settled stops naming itself", () => {
    // A failed delegate from hours ago is history: the inbox and the details panel keep it, and it
    // does not pin a header over the composer forever.
    const failed = [link("root", "broken", { state: "failed", settledAt: MINUTE })];
    const bar = branchBar(
      forestTotal(delegateViews(delegateTree("root", failed), read({ activity: () => undefined }))),
      [],
      2 * MINUTE,
    );
    expect(bar.failed).toBe(1);
    expect(bar.label).toBe("");
    expect(bar.status).toBe("");
    expect(bar.state).toBe("done");
    expect(bar.longestMs).toBeUndefined();
    expect(bar.show).toBe(false);
  });
});

describe("the words on a row", () => {
  test("a time under a second says nothing rather than 0s", () => {
    expect(timeLabel(0)).toBe("—");
    expect(timeLabel(999)).toBe("—");
    expect(timeLabel(124_000)).toBe("2m 04s");
    expect(timeLabel(3_600_000)).toBe("1h 00m");
  });

  test("a cost is exact, estimated, or unknown", () => {
    expect(costLabel(0.31)).toBe("$0.31");
    expect(costLabel(0.31, true)).toBe("≈$0.31");
    expect(costLabel(undefined, true)).toBe("—");
  });
});

/**
 * A tool call as the transcript holds it. `diffs` is all else a call needs:
 * what it started, who it is and how it goes are what these tests read.
 */
const call = (id: string, title: string, extra: Partial<TranscriptItem> = {}): TranscriptItem =>
  ({
    kind: "tool",
    id,
    title,
    status: "in_progress",
    diffs: [],
    ...extra,
  }) as TranscriptItem;

describe("a harness's own subagents", () => {
  test("one at work reads beside the delegates, with the steps it is on", () => {
    const items: TranscriptItem[] = [
      call("agent", "Agent", {
        toolName: "Agent",
        input: { subagent_type: "Explore", description: "Study t3.codes" },
      }),
      call("step", "Read", { parent: "agent", title: "Read" }),
      call("done", "Agent", {
        status: "completed",
        toolName: "Agent",
        input: { subagent_type: "Explore", description: "Already answered" },
      }),
    ];
    const rows = runningSubagents(items);
    expect(rows.length).toBe(1);
    expect(rows[0]!.item.id).toBe("agent");
    expect(rows[0]!.steps.map((step) => step.id)).toEqual(["step"]);
  });

  test("a finished agent stays in the transcript, not in the strip", () => {
    const items: TranscriptItem[] = [
      call("done", "Task", { status: "completed", toolName: "Task", input: { description: "Did it" } }),
    ];
    expect(runningSubagents(items)).toEqual([]);
  });

  test("RustCode's own delegate calls are the links, never native agent rows", () => {
    const items: TranscriptItem[] = [
      call("start", "start_session", { input: { prompt: "Fix the failing test" } }),
      call("read", "Read"),
    ];
    expect(runningSubagents(items)).toEqual([]);
  });
});

describe("RustCode's own subagents", () => {
  const agent = (status: "running" | "completed", id: number) => ({
    id,
    name: id === 1 ? "Explore" : "Reviewer",
    task: `Task ${id}`,
    agentType: "default",
    status,
    activeTurn: false,
    depth: 0,
    writeAccess: false,
    toolCalls: 0,
    requests: 0,
    inputTokens: 0,
    outputTokens: 0,
    cachedInputTokens: 0,
  });

  test("one still at work is a row, though its spawn_agent call already returned", () => {
    const spawn = call("spawn", "spawn_agent", {
      status: "completed",
      output: '{"agent_id": 1, "name": "Explore"}',
    });
    expect(runningSubagents([spawn])).toEqual([]);
    const rows = runningAgents([agent("running", 1), agent("completed", 2)]);
    expect(rows.map((a) => a.name)).toEqual(["Explore"]);
  });
});
