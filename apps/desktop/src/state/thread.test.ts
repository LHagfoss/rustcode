import { describe, expect, test } from "bun:test";
import type { SessionUpdate } from "../acp/types";
import {
  appendSteer,
  applyUpdate,
  beginTurn,
  diffCounts,
  diffLines,
  emptyThread,
  finishLoad,
  finishTurn,
  formatDuration,
  requeueSteers,
  thoughtHeadings,
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
