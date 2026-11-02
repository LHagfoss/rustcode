import { describe, expect, test } from "bun:test";
import { foldLabel, foldTurns } from "./fold";
import type { TranscriptItem } from "./thread";

const user = (id: string): TranscriptItem => ({ kind: "user", id, text: id });
const reply = (id: string): TranscriptItem => ({ kind: "assistant", id, text: id });
const thought = (id: string): TranscriptItem => ({
  kind: "thought",
  id,
  text: id,
  summary: false,
  startedAt: 0,
  done: true,
});
const tool = (id: string, status: "completed" | "failed" = "completed", path?: string): TranscriptItem => ({
  kind: "tool",
  id,
  title: id,
  status,
  diffs: path ? [{ path, oldText: "a", newText: "b" } as never] : [],
});
const end = (id: string, elapsedMs = 80_000): TranscriptItem => ({
  kind: "turnEnd",
  id,
  elapsedMs,
  outcome: "completed",
});
const error = (id: string): TranscriptItem => ({ kind: "notice", id, text: id, tone: "error" });

const keys = (rows: ReturnType<typeof foldTurns>) =>
  rows.map((r) => ("fold" in r ? `[${r.fold.key}]` : r.key));

describe("turn folding", () => {
  const items = [
    user("u1"),
    thought("t1"),
    reply("r1"), // between steps: folds
    tool("c1", "completed", "src/a.ts"),
    tool("c2", "failed"),
    tool("c3", "completed", "src/a.ts"),
    reply("r2"), // the final answer
    end("e1"),
    user("u2"),
    thought("t2"),
    tool("c4"),
    reply("r3"),
  ];

  test("a finished turn folds its work above the final answer", () => {
    const rows = foldTurns(items, true, new Set());
    expect(keys(rows).slice(0, 4)).toEqual(["u1", "[fold-e1]", "r2", "e1"]);
    const fold = rows.find((r) => "fold" in r)!;
    expect("fold" in fold && fold.fold).toMatchObject({
      steps: 5,
      tools: 3,
      failed: 1,
      files: 1,
      open: false,
    });
  });

  test("the running turn shows every step", () => {
    const rows = foldTurns(items, true, new Set());
    expect(keys(rows).slice(4)).toEqual(["u2", "t2", "c4", "r3"]);
  });

  test("once it is not running, the last turn folds too (a replayed thread)", () => {
    const rows = foldTurns(items, false, new Set());
    expect(keys(rows).slice(4)).toEqual(["u2", "[fold-t2]", "r3"]);
  });

  test("an opened turn shows its steps in order after the summary", () => {
    const rows = foldTurns(items, true, new Set(["fold-e1"]));
    expect(keys(rows).slice(0, 9)).toEqual(["u1", "[fold-e1]", "t1", "r1", "c1", "c2", "c3", "r2", "e1"]);
    expect(rows.filter((r) => "inFold" in r && r.inFold === "fold-e1").length).toBe(5);
  });

  test("errors and warnings stay visible; a turn with no work does not fold", () => {
    const rows = foldTurns(
      [user("u1"), tool("c1"), error("x1"), reply("r1"), end("e1"), user("u2"), reply("r2"), end("e2")],
      false,
      new Set(),
    );
    expect(keys(rows)).toEqual(["u1", "[fold-e1]", "x1", "r1", "e1", "u2", "r2", "e2"]);
  });

  test("the summary reads time, steps, files and failures", () => {
    const rows = foldTurns(items, true, new Set());
    const fold = rows.find((r) => "fold" in r);
    const label = fold && "fold" in fold ? foldLabel(fold.fold, (ms) => `${Math.round(ms / 1000)}s`) : "";
    expect(label).toBe("Worked for 80s · 5 steps · 1 file changed · 1 failed");
  });
});
