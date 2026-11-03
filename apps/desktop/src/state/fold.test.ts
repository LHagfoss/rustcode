import { describe, expect, test } from "bun:test";
import { foldLabel, foldTurns, groupTools, toolGroupLabel } from "./fold";
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

  test("a short running turn shows every step", () => {
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

describe("tool groups", () => {
  const kinded = (
    id: string,
    toolKind: string,
    status: "completed" | "failed" | "in_progress" = "completed",
  ): TranscriptItem => ({
    kind: "tool",
    id,
    title: id,
    toolKind,
    status,
    diffs: [],
  });
  const live = (items: TranscriptItem[], open: ReadonlySet<string> = new Set()) =>
    groupTools(foldTurns(items, true, new Set()), open);

  test("three or more finished calls in a row fold into one row", () => {
    const rows = live([
      user("u"),
      kinded("a", "read"),
      kinded("b", "read"),
      kinded("c", "execute"),
      kinded("d", "edit", "in_progress"),
    ]);
    expect(rows.map((r) => r.key)).toEqual(["u", "tools-a", "d"]);
    const group = rows[1]!;
    expect("group" in group && group.group.label).toBe("Read 2 files · Ran 1 command");
  });

  test("two calls stay as they are, and an open group lists its calls", () => {
    expect(live([user("u"), kinded("a", "read"), kinded("b", "read")]).map((r) => r.key)).toEqual([
      "u",
      "a",
      "b",
    ]);
    const rows = live(
      [user("u"), kinded("a", "read"), kinded("b", "read"), kinded("c", "read")],
      new Set(["tools-a"]),
    );
    expect(rows.map((r) => r.key)).toEqual(["u", "tools-a", "a", "b", "c"]);
    expect(rows[2] && "inFold" in rows[2] ? rows[2].inFold : undefined).toBe("tools-a");
  });

  test("the label counts what the calls did, most first", () => {
    expect(toolGroupLabel(["execute", "read", "execute", "search", undefined])).toBe(
      "Ran 2 commands · Read 1 file · Searched once · 1 other step",
    );
    expect(toolGroupLabel(["read", "edit", "execute", "search", "fetch"])).toBe(
      "Read 1 file · Edited 1 file · Ran 1 command · 2 more",
    );
  });
});

describe("the running turn", () => {
  const long = [
    user("u"),
    thought("t"),
    tool("a1", "completed", "src/x.ts"),
    tool("a2", "failed"),
    tool("a3"),
    reply("r"),
    tool("b1"),
    tool("b2"),
  ];

  test("folds what came before its latest reply", () => {
    const rows = foldTurns(long, true, new Set());
    expect(keys(rows)).toEqual(["u", "[live-u]", "r", "b1", "b2"]);
    const fold = rows.find((r) => "fold" in r);
    expect(fold && "fold" in fold && fold.fold).toMatchObject({
      steps: 4,
      tools: 3,
      failed: 1,
      files: 1,
      live: true,
    });
    expect(foldLabel((fold as { fold: Parameters<typeof foldLabel>[0] }).fold, String)).toBe(
      "4 earlier steps · 1 file changed · 1 failed",
    );
  });

  test("opens like a finished turn", () => {
    const rows = foldTurns(long, true, new Set(["live-u"]));
    expect(keys(rows)).toEqual(["u", "[live-u]", "t", "a1", "a2", "a3", "r", "b1", "b2"]);
  });

  test("with no reply yet, keeps only its last steps", () => {
    const steps = [user("u"), ...Array.from({ length: 10 }, (_, i) => tool(`c${i}`))];
    expect(keys(foldTurns(steps, true, new Set()))).toEqual([
      "u",
      "[live-u]",
      "c4",
      "c5",
      "c6",
      "c7",
      "c8",
      "c9",
    ]);
  });
});
