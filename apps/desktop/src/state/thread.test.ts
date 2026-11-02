import { describe, expect, test } from "bun:test";
import { parseSessionUpdate, type SessionUpdate } from "../acp/types";
import {
  appendSteer,
  applyUpdate,
  beginTurn,
  diffCounts,
  diffLines,
  emptyThread,
  finishLoad,
  finishTurn,
  foldingBatch,
  formatClock,
  formatDuration,
  idleSettled,
  requeueSteers,
  shortAge,
  thoughtHeadings,
  workingLabel,
} from "./thread";

const apply = (updates: SessionUpdate[], state = emptyThread("s", "/p")) =>
  updates.reduce((s, u) => applyUpdate(s, u, 1000), state);

describe("thread reducer", () => {
  test("streams reply chunks into one message and drops the leading blank lines", () => {
    const s = apply([
      { kind: "user", text: "hi" },
      { kind: "agent", text: "\n\nHel" },
      { kind: "agent", text: "lo" },
    ]);
    expect(s.items.map((i) => i.kind)).toEqual(["user", "assistant"]);
    expect(s.items[1]).toMatchObject({ text: "Hello" });
  });

  test("a thought closes when the reply starts", () => {
    const s = apply([
      { kind: "thought", text: "**A**", summary: true },
      { kind: "thought", text: " more", summary: true },
      { kind: "agent", text: "x" },
    ]);
    expect(s.items[0]).toMatchObject({ kind: "thought", text: "**A** more", done: true });
  });

  test("tool calls update in place, accumulate output deltas and keep diffs", () => {
    const s = apply([
      { kind: "toolCall", id: "c", title: "bash", diffs: [] },
      { kind: "toolCallUpdate", id: "c", outputDelta: "a\n", diffs: [] },
      { kind: "toolCallUpdate", id: "c", outputDelta: "b\n", diffs: [] },
      {
        kind: "toolCallUpdate",
        id: "c",
        status: "completed",
        diffs: [{ path: "f", newText: "x", oldStart: 1, newStart: 1 }],
        exitCode: 0,
      },
      { kind: "toolCallUpdate", id: "unknown", status: "failed", diffs: [] },
    ]);
    expect(s.items).toHaveLength(1);
    expect(s.items[0]).toMatchObject({ kind: "tool", status: "completed", output: "a\nb\n", exitCode: 0 });
  });

  test("replayed turn timing becomes a footer before the next user message and at the end of the load", () => {
    const s = finishLoad(
      apply([
        { kind: "user", text: "1" },
        { kind: "agent", text: "a", turn: { elapsedMs: 5000, outcome: "completed" } },
        { kind: "user", text: "2" },
        { kind: "agent", text: "b", turn: { elapsedMs: 7000, outcome: "completed" } },
      ]),
    );
    expect(s.items.map((i) => i.kind)).toEqual([
      "user",
      "assistant",
      "turnEnd",
      "user",
      "assistant",
      "turnEnd",
    ]);
    expect(s.items[5]).toMatchObject({ elapsedMs: 7000 });
    expect(s.loaded).toBe(true);
  });

  test("a turn ends: running tools settle, permissions clear, and the footer has the elapsed time", () => {
    let s = beginTurn(emptyThread("s", "/p"), "go", 0);
    s = applyUpdate(s, { kind: "toolCall", id: "c", title: "bash", status: "in_progress", diffs: [] }, 10);
    s = finishTurn(s, "cancelled", 4000);
    expect(s.running).toBe(false);
    expect(s.items[1]).toMatchObject({ status: "failed" });
    expect(s.items.at(-1)).toMatchObject({ kind: "turnEnd", outcome: "cancelled", elapsedMs: 4000 });
  });

  test("unused steers leave the transcript and go back to the front of the queue", () => {
    let s = beginTurn(emptyThread("s", "/p"), "go", 0);
    s = { ...s, queue: [{ id: "q", text: "later" }] };
    s = appendSteer(s, "also this");
    s = requeueSteers(s, ["also this"]);
    expect(s.items.some((i) => i.kind === "user" && i.text === "also this")).toBe(false);
    expect(s.queue.map((q) => q.text)).toEqual(["also this", "later"]);
  });

  test("plan, title, usage and config options", () => {
    const s = apply([
      { kind: "plan", entries: [{ content: "a", status: "completed" }] },
      { kind: "title", title: "T" },
      { kind: "usage", usage: { used: 1, size: 2, estimated: false } },
      {
        kind: "configOptions",
        options: [{ id: "model", name: "Model", type: "select", current: "m", choices: [] }],
      },
    ]);
    expect(s).toMatchObject({
      title: "T",
      plan: [{ content: "a" }],
      usage: { used: 1 },
      configOptions: [{ id: "model" }],
    });
  });
});

describe("helpers", () => {
  test("line diff keeps context and numbers lines", () => {
    const lines = diffLines({
      path: "f",
      oldText: "a\nb\nc",
      newText: "a\nB\nc\nd",
      oldStart: 10,
      newStart: 10,
    });
    expect(lines.map((l) => `${l.kind[0]}${l.text}`)).toEqual(["ca", "rb", "aB", "cc", "ad"]);
    expect(lines[4]).toMatchObject({ newNumber: 13 });
    expect(
      diffCounts([{ path: "f", oldText: "a\nb\nc", newText: "a\nB\nc\nd", oldStart: 1, newStart: 1 }]),
    ).toEqual({ added: 2, removed: 1 });
  });

  test("a created file is all additions", () => {
    expect(
      diffLines({ path: "f", newText: "x\ny", oldStart: 1, newStart: 1 }).every((l) => l.kind === "added"),
    ).toBe(true);
  });

  test("durations and thought headings", () => {
    expect(formatDuration(12_000)).toBe("12s");
    expect(formatDuration(65_000)).toBe("1m 5s");
    expect(thoughtHeadings("**Reading**\ntext **Editing files**")).toEqual(["Reading", "Editing files"]);
  });
});

describe("folding a batch", () => {
  const updates: SessionUpdate[] = [];
  for (let i = 0; i < 50; i++) {
    updates.push({ kind: "user", text: `q${i}` });
    updates.push({ kind: "thought", text: "**T**", summary: true });
    updates.push({ kind: "toolCall", id: `c${i}`, title: "rg", diffs: [] });
    updates.push({ kind: "toolCallUpdate", id: `c${i}`, status: "completed", outputDelta: "ok", diffs: [] });
    updates.push({ kind: "agent", text: "Hel" });
    updates.push({ kind: "agent", text: "lo" });
  }

  test("gives the same thread as folding one update at a time", () => {
    const plain = apply(updates);
    let batched = emptyThread("s", "/p");
    foldingBatch(() => {
      batched = apply(updates, batched);
    });
    const shape = (s: typeof plain) => ({
      items: s.items.map(({ id: _id, ...rest }) => rest),
      tools: Object.keys(s.toolIndex).length,
    });
    expect(shape(batched)).toEqual(shape(plain));
  });

  test("never changes arrays made before the batch", () => {
    const before = apply(updates.slice(0, 6));
    const items = before.items;
    const copy = items.slice();
    const toolIndex = { ...before.toolIndex };
    foldingBatch(() => {
      apply(updates.slice(6), before);
    });
    expect(before.items).toBe(items);
    expect(items).toEqual(copy);
    expect(before.toolIndex).toEqual(toolIndex);
  });

  test("arrays from a finished batch are not changed by the next one", () => {
    let first = emptyThread("s", "/p");
    foldingBatch(() => {
      first = apply(updates.slice(0, 6), first);
    });
    const length = first.items.length;
    foldingBatch(() => {
      apply(updates.slice(6, 12), first);
    });
    expect(first.items.length).toBe(length);
  });

  test("a streamed chunk closes only the open thought at the tail", () => {
    const s = apply([
      { kind: "thought", text: "a", summary: false },
      { kind: "agent", text: "x" },
      { kind: "thought", text: "b", summary: false },
      { kind: "user", text: "next" },
    ]);
    expect(s.items.filter((i) => i.kind === "thought").every((t) => t.kind === "thought" && t.done)).toBe(
      true,
    );
  });
});

describe("sidebar times", () => {
  test("a running turn's clock", () => {
    expect(formatClock(0)).toBe("0:00");
    expect(formatClock(42_900)).toBe("0:42");
    expect(formatClock(12 * 60_000 + 3_000)).toBe("12:03");
    expect(formatClock(3_723_000)).toBe("1:02:03");
  });

  test("a compact age", () => {
    const now = 10 * 86_400_000;
    expect(shortAge(undefined, now)).toBe("");
    expect(shortAge(now - 30_000, now)).toBe("now");
    expect(shortAge(now - 5 * 60_000, now)).toBe("5m");
    expect(shortAge(now - 3 * 3_600_000, now)).toBe("3h");
    expect(shortAge(now - 2 * 86_400_000, now)).toBe("2d");
  });
});

describe("the working line", () => {
  const user = { kind: "user", id: "u", text: "go" } as const;
  test("says what the newest step of the turn does", () => {
    expect(workingLabel([user])).toBe("Working");
    expect(
      workingLabel([
        user,
        {
          kind: "thought",
          id: "t",
          text: "**Reading the code**\n…\n**Planning the fix**",
          summary: false,
          startedAt: 0,
          done: false,
        },
      ]),
    ).toBe("Thinking · Planning the fix");
    expect(
      workingLabel([
        user,
        {
          kind: "tool",
          id: "x",
          title: "bash",
          toolKind: "execute",
          summary: "bun test",
          status: "in_progress",
          diffs: [],
        },
      ]),
    ).toBe("Running bun test");
    expect(workingLabel([user, { kind: "assistant", id: "a", text: "Here" }])).toBe("Writing the reply");
    expect(
      workingLabel([
        user,
        { kind: "thought", id: "t", text: "done", summary: false, startedAt: 0, done: true },
      ]),
    ).toBe("Working");
  });
});

describe("compaction", () => {
  test("a harness's compaction card becomes one quiet row", () => {
    const update = parseSessionUpdate({
      sessionId: "s",
      update: {
        sessionUpdate: "agent_message_chunk",
        content: { type: "text", text: "Context compacted: Codex summarized the conversation.\n\nSummary…" },
        _meta: { rustcode: { kind: "compaction" } },
      },
    });
    expect(update.kind).toBe("compaction");
    const state = applyUpdate(emptyThread("s", "/p"), update);
    expect(state.items.at(-1)).toMatchObject({
      kind: "notice",
      tone: "info",
      text: "Context compacted: Codex summarized the conversation.",
    });
  });
});

describe("auto-settle", () => {
  test("a thread untouched for three days settles", () => {
    const now = 10 * 86_400_000;
    expect(idleSettled(undefined, now)).toBe(false);
    expect(idleSettled(now - 2 * 86_400_000, now)).toBe(false);
    expect(idleSettled(now - 3 * 86_400_000, now)).toBe(true);
  });
});
