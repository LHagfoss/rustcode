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

  test("the divider reads how long it worked, or that you stopped it", () => {
    const seconds = (ms: number) => `${Math.round(ms / 1000)}s`;
    const rows = foldTurns(items, true, new Set());
    const fold = rows.find((r) => "fold" in r);
    expect(fold && "fold" in fold ? foldLabel(fold.fold, seconds) : "").toBe("Worked for 80s");
    const stopped = foldTurns(
      [
        user("u"),
        tool("c"),
        reply("r"),
        { kind: "turnEnd", id: "e", elapsedMs: 12_000, outcome: "cancelled" },
      ],
      false,
      new Set(),
    ).find((r) => "fold" in r);
    expect(stopped && "fold" in stopped ? foldLabel(stopped.fold, seconds) : "").toBe(
      "You stopped after 12s",
    );
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

  test("two or more finished calls in a row fold into one row", () => {
    const rows = live([
      user("u"),
      kinded("a", "read"),
      kinded("b", "read"),
      kinded("c", "execute"),
      kinded("d", "edit", "in_progress"),
    ]);
    expect(rows.map((r) => r.key)).toEqual(["u", "tools-a", "d"]);
    const group = rows[1]!;
    expect("group" in group && group.group.label).toBe("Read 2 files and ran 1 command");
    expect("group" in group && group.group.kind).toBe("read");
  });

  test("one call stays as it is, and an open group lists its calls", () => {
    expect(
      live([user("u"), kinded("a", "read"), kinded("b", "read", "in_progress")]).map((r) => r.key),
    ).toEqual(["u", "a", "b"]);
    const rows = live(
      [user("u"), kinded("a", "read"), kinded("b", "read"), kinded("c", "read")],
      new Set(["tools-a"]),
    );
    expect(rows.map((r) => r.key)).toEqual(["u", "tools-a", "a", "b", "c"]);
    expect(rows[2] && "inFold" in rows[2] ? rows[2].inFold : undefined).toBe("tools-a");
  });

  test("the label counts what the calls did, most first", () => {
    expect(toolGroupLabel(["execute", "read", "execute", "search", undefined])).toBe(
      "Ran 2 commands, read 1 file and performed 2 other actions",
    );
    expect(toolGroupLabel(["read", "read"])).toBe("Read 2 files");
    expect(toolGroupLabel(["read", "edit", "execute"])).toBe(
      "Read 1 file, edited 1 file and performed 1 other action",
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

  test("shows as it streams, nothing folded", () => {
    expect(keys(foldTurns(long, true, new Set()))).toEqual(["u", "t", "a1", "a2", "a3", "r", "b1", "b2"]);
  });

  test("its finished calls (and the thoughts among them) read as one line per run", () => {
    const rows = groupTools(foldTurns(long, true, new Set()), new Set());
    expect(rows.map((r) => r.key)).toEqual(["u", "tools-t", "r", "tools-b1"]);
    const first = rows[1]!;
    expect("group" in first && first.group).toMatchObject({ count: 4, failed: 1 });
  });
});
