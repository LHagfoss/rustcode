import { describe, expect, test } from "bun:test";
import { foldTurns } from "./fold";
import type { TranscriptItem } from "./thread";
import { foldUpdateRuns, UPDATE_MAX_MS, updateRunLabel } from "./updateRuns";

const user = (id: string, text = id): TranscriptItem => ({ kind: "user", id, text });
const pull = (id: string, n: number): TranscriptItem => ({
  kind: "user",
  id,
  text: `Pull request #${n}: Merged`,
  continued: true,
});
const wake = (id: string): TranscriptItem => ({
  kind: "notice",
  id: `wake-${id}`,
  text: "Background task finished",
  tone: "info",
});
const reply = (id: string): TranscriptItem => ({ kind: "assistant", id, text: id });
const tool = (id: string): TranscriptItem => ({
  kind: "tool",
  id,
  title: id,
  status: "completed",
  diffs: [],
});
const end = (id: string, elapsedMs = 2_000): TranscriptItem => ({
  kind: "turnEnd",
  id,
  elapsedMs,
  outcome: "completed",
});

/** A quiet turn woken by pull request `n`'s news. */
const news = (n: number, elapsedMs?: number) => [pull(`p${n}`, n), reply(`r${n}`), end(`e${n}`, elapsedMs)];

const rowsOf = (items: TranscriptItem[], open: ReadonlySet<string> = new Set()) =>
  foldUpdateRuns(foldTurns(items, false, open), open);

const keys = (rows: ReturnType<typeof rowsOf>) =>
  rows.map((r) => ("updates" in r ? `[${r.key}]` : "fold" in r && r.fold ? `(${r.key})` : r.key));

describe("runs of short automatic turns", () => {
  test("three or more fold under one row, and the newest stays in view", () => {
    const items = [user("u"), reply("r"), end("e", 60_000), ...news(1), ...news(2), ...news(3)];
    const rows = rowsOf(items);
    expect(keys(rows)).toEqual(["u", "(fold-e)", "r", "e", "[updates-p1]", "p3", "r3", "e3"]);
    const run = rows.find((r) => "updates" in r);
    expect(run && "updates" in run ? updateRunLabel(run.updates) : undefined).toEqual({
      label: "2 earlier updates",
      detail: "#1, #2",
    });
  });

  test("an opened run shows its turns under its row", () => {
    const items = [...news(1), ...news(2), ...news(3)];
    const rows = rowsOf(items, new Set(["updates-p1"]));
    expect(keys(rows)).toEqual(["[updates-p1]", "p1", "r1", "e1", "p2", "r2", "e2", "p3", "r3", "e3"]);
  });

  test("two in a row stay as they are", () => {
    const items = [...news(1), ...news(2)];
    expect(keys(rowsOf(items))).toEqual(["p1", "r1", "e1", "p2", "r2", "e2"]);
  });

  test("a turn the engine woke on its own counts, by its notice", () => {
    const items = [...news(1), wake("w"), reply("rw"), end("ew"), ...news(2)];
    const rows = rowsOf(items);
    expect(keys(rows)).toEqual(["[updates-p1]", "p2", "r2", "e2"]);
    const run = rows[0]!;
    expect("updates" in run && run.updates.count).toBe(2);
  });

  test("a long or busy turn breaks the run: its report keeps its place", () => {
    const long = [...news(1), ...news(2, UPDATE_MAX_MS), ...news(3), ...news(4)];
    expect(keys(rowsOf(long)).some((k) => k.startsWith("[updates"))).toBe(false);
    // A message the user typed is never folded away.
    const typed = [...news(1), user("u"), reply("ru"), end("eu"), ...news(2), ...news(3)];
    expect(keys(rowsOf(typed)).some((k) => k.startsWith("[updates"))).toBe(false);
  });

  test("a turn that changed files breaks the run", () => {
    const edit: TranscriptItem = {
      kind: "tool",
      id: "edit",
      title: "Edit",
      status: "completed",
      diffs: [{ path: "a.ts", oldText: "a", newText: "b" } as never],
    };
    const items = [...news(1), pull("p2", 2), edit, reply("r2"), end("e2"), ...news(3), ...news(4)];
    expect(keys(rowsOf(items)).some((k) => k.startsWith("[updates"))).toBe(false);
  });

  test("a quiet turn that ran a check still folds with its steps", () => {
    const items = [...news(1), pull("p2", 2), tool("gh"), reply("r2"), end("e2"), ...news(3)];
    expect(keys(rowsOf(items))).toEqual(["[updates-p1]", "p3", "r3", "e3"]);
  });
});
