// The rules the right panel's overview reads (ui/overview.ts): which sections
// are folded, what a pull request's row says, and what the thread delivered and
// spent. No React here, so the panel's tests drive the data directly.

import { describe, expect, test } from "bun:test";
import type { PullRequest } from "../acp/rustcode";
import type { TranscriptItem } from "../state/thread";
import {
  delegatedIds,
  deliverables,
  FOLDED_BY_DEFAULT,
  logTotals,
  parseFolded,
  pullBranches,
  pullPill,
  pullsForBranches,
  replyHeadline,
  toggleFolded,
  turnEntries,
  turnHasTrace,
} from "./overview";

function pull(over: Partial<PullRequest> = {}): PullRequest {
  return {
    number: 1,
    title: "A pull request",
    author: "someone",
    state: "open",
    draft: false,
    checks: { passed: 0, failed: 0, pending: 0, skipped: 0 },
    headRef: "feat/one",
    baseRef: "main",
    headSha: "abc1234",
    additions: 10,
    deletions: 2,
    changedFiles: 3,
    ...over,
  };
}

const checks = (over: Partial<PullRequest["checks"]> = {}) => ({
  passed: 0,
  failed: 0,
  pending: 0,
  skipped: 0,
  ...over,
});

describe("the sections a reader folds away", () => {
  test("a user who never folded anything opens on the overview, with Details away", () => {
    expect(parseFolded(null)).toEqual([...FOLDED_BY_DEFAULT]);
    expect(parseFolded("")).toEqual([...FOLDED_BY_DEFAULT]);
  });

  test("a broken value reads as the default instead of losing every section", () => {
    expect(parseFolded("{")).toEqual([...FOLDED_BY_DEFAULT]);
    expect(parseFolded('{"details":true}')).toEqual([...FOLDED_BY_DEFAULT]);
  });

  test("kept ids are read in the panel's order, and ids the panel no longer has are dropped", () => {
    expect(parseFolded('["log","details"]')).toEqual(["log", "details"]);
    expect(parseFolded('["log","gone"]')).toEqual(["log"]);
    expect(parseFolded("[]")).toEqual([]);
  });

  test("a press on a header flips only that section", () => {
    expect(toggleFolded(FOLDED_BY_DEFAULT, "details")).toEqual(["delegates"]);
    expect(toggleFolded([], "details")).toEqual(["details"]);
    expect(toggleFolded(["properties"], "log")).toEqual(["properties", "log"]);
    expect(toggleFolded(["properties", "log"], "properties")).toEqual(["log"]);
  });
});

describe("what a pull request's row says", () => {
  test("a merged pull keeps the word the thread is judged by", () => {
    expect(pullPill(pull({ state: "merged" }))).toEqual({ label: "Merged", tone: "accent" });
    expect(pullPill(pull({ state: "closed" }))).toEqual({ label: "Closed", tone: "muted" });
  });

  test("a draft says so before its checks do", () => {
    expect(pullPill(pull({ draft: true, checks: checks({ failed: 2 }) }))).toEqual({
      label: "Draft",
      tone: "muted",
    });
  });

  test("an open pull reads its checks, and is ready when there are none to fail", () => {
    expect(pullPill(pull({ checks: checks({ failed: 1, passed: 3 }) }))).toEqual({
      label: "Checks failing",
      tone: "warning",
    });
    expect(pullPill(pull({ checks: checks({ pending: 2 }) }))).toEqual({
      label: "Checks running",
      tone: "muted",
    });
    expect(pullPill(pull({ checks: checks({ passed: 4 }) }))).toEqual({
      label: "Ready",
      tone: "success",
    });
    expect(pullPill(pull())).toEqual({ label: "Open", tone: "muted" });
  });
});

describe("the pulls a thread's branches have", () => {
  test("its own branch first, then its delegates', each pull once", () => {
    const mine = pull({ number: 7, headRef: "feat/mine" });
    const child = pull({ number: 8, headRef: "feat/child" });
    const found = pullsForBranches(
      [child, mine, pull({ number: 9, headRef: "feat/other" })],
      ["feat/mine", "feat/child", "feat/mine"],
    );
    expect(found.map((p) => p.number)).toEqual([7, 8]);
  });

  test("open work reads before what became of the older branches", () => {
    const merged = pull({ number: 1, state: "merged", updatedAt: 9, headRef: "a" });
    const open = pull({ number: 2, updatedAt: 1, headRef: "b" });
    const closed = pull({ number: 3, state: "closed", updatedAt: 2, headRef: "c" });
    expect(pullsForBranches([merged, closed, open], ["a", "b", "c"]).map((p) => p.number)).toEqual([2, 1, 3]);
  });

  test("a thread with no branch matches nothing", () => {
    expect(pullsForBranches([pull()], [undefined])).toEqual([]);
    expect(pullsForBranches(undefined, ["feat/one"])).toEqual([]);
    expect(pullBranches(undefined, [undefined])).toEqual([]);
    expect(pullBranches("feat/one", ["feat/two"])).toEqual(["feat/one", "feat/two"]);
  });
});

describe("the threads a thread started", () => {
  test("every delegate, however deep", () => {
    const link = (orchestrator: string, target: string) => ({
      id: `${orchestrator}-${target}`,
      orchestrator,
      target,
      orchestratorTitle: orchestrator,
      targetTitle: target,
      message: "go",
      state: "done" as const,
      createdAt: 1,
      started: true,
    });
    expect(delegatedIds("root", [link("root", "a"), link("a", "b"), link("root", "c")])).toEqual([
      "a",
      "b",
      "c",
    ]);
  });
});

const turnEnd = (
  id: string,
  over: Partial<Extract<TranscriptItem, { kind: "turnEnd" }>> = {},
): Extract<TranscriptItem, { kind: "turnEnd" }> => ({
  kind: "turnEnd",
  id,
  outcome: "completed",
  ...over,
});

describe("the thread's turns", () => {
  test("each turn keeps the first line of the message that opened it", () => {
    const items: TranscriptItem[] = [
      { kind: "user", id: "u1", text: "Fix the panel\n\nand its tests" },
      turnEnd("e1", { elapsedMs: 4000 }),
      { kind: "user", id: "u2", text: "Now the sidebar" },
      turnEnd("e2", { elapsedMs: 6000, outcome: "failed" }),
    ];
    const turns = turnEntries(items);
    expect(turns.map((turn) => turn.prompt)).toEqual(["Fix the panel", "Now the sidebar"]);
    expect(turns.map((turn) => turn.outcome)).toEqual(["completed", "failed"]);
    expect(turns.map((turn) => turn.elapsedMs)).toEqual([4000, 6000]);
  });

  test("a turn the app woke the thread for says so instead of borrowing the message above", () => {
    const items: TranscriptItem[] = [
      { kind: "user", id: "u1", text: "Do the work" },
      turnEnd("e1"),
      { kind: "user", id: "u2", text: "A delegate finished", wake: true },
      turnEnd("e2"),
    ];
    expect(turnEntries(items).map((turn) => turn.prompt)).toEqual(["Do the work", "Woken for a delegate"]);
  });

  test("a turn the app started is named by its last reply, and says what started it", () => {
    const items: TranscriptItem[] = [
      { kind: "user", id: "u1", text: "Fix it all" },
      turnEnd("e1"),
      {
        kind: "user",
        id: "u2",
        text: "Continued after compaction\n\nContinue.\n\n<!-- rustcode:continued -->",
        continued: true,
      },
      { kind: "assistant", id: "a1", text: "Looking at the inbox first." },
      { kind: "assistant", id: "a2", text: "## Done\n\nI opened **PR 14**: the inbox approves. More here." },
      turnEnd("e2"),
      {
        kind: "user",
        id: "u3",
        text: "Resumed after restart.\n\n<!-- rustcode:resumed -->",
        continued: true,
      },
      turnEnd("e3"),
    ];
    const turns = turnEntries(items);
    expect(turns.map((turn) => turn.prompt)).toEqual([
      "Fix it all",
      "I opened PR 14: the inbox approves.",
      "Resumed after restart.",
    ]);
    expect(turns.map((turn) => turn.origin)).toEqual([undefined, "After compaction", undefined]);
  });

  test("a reply's headline is its first plain sentence", () => {
    expect(replyHeadline("I opened [PR 13](https://x/13). Next the inbox.")).toBe("I opened PR 13.");
    expect(replyHeadline("```ts\ncode\n```\n- **Merged** twelve PRs")).toBe("Merged twelve PRs");
    expect(replyHeadline("Version 1.2 ships")).toBe("Version 1.2 ships");
    expect(replyHeadline("## Only a heading")).toBe("Only a heading");
  });

  test("a turn's cost is its last sample minus what the session had already spent", () => {
    const items: TranscriptItem[] = [
      { kind: "user", id: "u1", text: "Go" },
      turnEnd("e1", {
        usage: {
          before: { used: 0, size: 0, estimated: false, inputTokens: 100, outputTokens: 20, cost: 0.1 },
          samples: [
            {
              at: 2,
              usage: { used: 0, size: 0, estimated: false, inputTokens: 300, outputTokens: 80, cost: 0.4 },
            },
          ],
        },
      }),
    ];
    const [turn] = turnEntries(items);
    expect(turn?.cost).toBeCloseTo(0.3);
    expect(turn?.tokens).toBe(260);
  });

  test("a turn begun before the app knew the totals is priced from the turn before it", () => {
    const sample = (inputTokens: number, cost: number) => ({
      used: 0,
      size: 0,
      estimated: false,
      inputTokens,
      outputTokens: 0,
      cost,
    });
    const turns = turnEntries([
      { kind: "user", id: "u1", text: "One" },
      turnEnd("e1", { usage: { before: sample(0, 0), samples: [{ at: 1, usage: sample(100, 0.2) }] } }),
      { kind: "user", id: "u2", text: "Two" },
      turnEnd("e2", { usage: { samples: [{ at: 2, usage: sample(250, 0.5) }] } }),
    ]);
    expect(turns[1]?.cost).toBeCloseTo(0.3);
    expect(turns[1]?.tokens).toBe(150);
  });

  test("a turn the engine priced nothing for has no numbers, rather than a guess", () => {
    const [turn] = turnEntries([
      { kind: "user", id: "u1", text: "Go" },
      turnEnd("e1", { usage: { samples: [] } }),
    ]);
    expect(turn?.cost).toBeUndefined();
    expect(turn?.tokens).toBeUndefined();
  });

  test("only a turn the app watched a waterfall for opens one", () => {
    expect(turnHasTrace({ id: "e1", prompt: "", outcome: "completed", startedAt: 1, at: 5 })).toBe(true);
    expect(turnHasTrace({ id: "e1", prompt: "", outcome: "completed", startedAt: 5, at: 5 })).toBe(false);
    expect(turnHasTrace({ id: "e1", prompt: "", outcome: "completed", at: 5 })).toBe(false);
  });

  test("replayed turns without turnEnd footers stay listed across restarts and while a new turn runs", () => {
    const replayed: TranscriptItem[] = [
      { kind: "user", id: "u1", text: "Inspect the sidebar" },
      { kind: "tool", id: "c1", title: "Read", status: "completed", diffs: [] },
      { kind: "assistant", id: "a1", text: "Found the turns section." },
      { kind: "user", id: "u2", text: "Fix the replay" },
      { kind: "assistant", id: "a2", text: "Done." },
    ];
    expect(turnEntries(replayed)).toEqual([
      { id: "u1", prompt: "Inspect the sidebar", outcome: "completed" },
      { id: "u2", prompt: "Fix the replay", outcome: "completed" },
    ]);
    const withLiveTail: TranscriptItem[] = [
      ...replayed,
      { kind: "user", id: "u3", text: "Run the checks" },
      { kind: "thought", id: "t3", text: "Checking", summary: false, startedAt: 1, done: false },
    ];
    expect(turnEntries(withLiveTail, true).map((turn) => turn.id)).toEqual(["u1", "u2"]);
    const afterLiveEnd: TranscriptItem[] = [...withLiveTail, turnEnd("e3", { elapsedMs: 3000 })];
    expect(turnEntries(afterLiveEnd, false).map((turn) => [turn.id, turn.prompt])).toEqual([
      ["u1", "Inspect the sidebar"],
      ["u2", "Fix the replay"],
      ["e3", "Run the checks"],
    ]);
  });
});

describe("the log's stats", () => {
  const turns = [
    { id: "e1", prompt: "one", outcome: "completed" as const, elapsedMs: 4000, tokens: 300 },
    { id: "e2", prompt: "two", outcome: "completed" as const, elapsedMs: 6000, tokens: 100 },
  ];

  test("spent is the thread's own reported cost, and the turns' time adds up", () => {
    const totals = logTotals(turns, { inputTokens: 320, outputTokens: 80, cost: 0.42 });
    expect(totals.spent).toBeCloseTo(0.42);
    expect(totals.tokens).toBe(400);
    expect(totals.agentMs).toBe(10_000);
    expect(totals.currency).toBe("USD");
  });

  test("a harness with no session total keeps the turns' own tokens", () => {
    expect(logTotals(turns, undefined).tokens).toBe(400);
    expect(logTotals(turns, undefined).spent).toBeUndefined();
    expect(logTotals([], undefined)).toEqual({
      spent: undefined,
      currency: "USD",
      tokens: undefined,
      agentMs: 0,
    });
  });
});

describe("what a thread delivered", () => {
  test("the pictures its replies show, but not the web's or a data URL's", () => {
    const items: TranscriptItem[] = [
      {
        kind: "assistant",
        id: "a1",
        text: [
          "Here it is:",
          "![The panel](/tmp/shot.png)",
          '![Another](shots/two.png "A title")',
          "![Web](https://example.com/x.png)",
          "![Inline](data:image/png;base64,AAAA)",
          "![The panel again](/tmp/shot.png)",
        ].join("\n"),
      },
    ];
    const delivered = deliverables(items);
    expect(delivered.pictures.map((p) => p.src)).toEqual(["/tmp/shot.png", "shots/two.png"]);
    expect(delivered.pictures.map((p) => p.label)).toEqual(["The panel", "Another"]);
  });

  test("the files it wrote, once each, with every edit's lines added up", () => {
    const diff = (path: string) => ({
      path,
      oldText: "one\ntwo\n",
      newText: "one\nTWO\nthree\n",
      oldStart: 1,
      newStart: 1,
    });
    const items: TranscriptItem[] = [
      {
        kind: "tool",
        id: "t1",
        title: "Write",
        status: "completed",
        diffs: [diff("src/a.ts"), diff("src/b.ts")],
      },
      {
        kind: "tool",
        id: "t2",
        title: "Write",
        status: "completed",
        diffs: [diff("src/a.ts")],
      },
      {
        kind: "tool",
        id: "t3",
        title: "Write",
        status: "failed",
        diffs: [diff("src/c.ts")],
      },
    ];
    const delivered = deliverables(items);
    expect(delivered.files.map((f) => f.path)).toEqual(["src/a.ts", "src/b.ts"]);
    expect(delivered.files.map((f) => f.label)).toEqual(["a.ts", "b.ts"]);
    expect(delivered.files[0]?.added).toBe(4);
    expect(delivered.files[0]?.removed).toBe(2);
  });
});
