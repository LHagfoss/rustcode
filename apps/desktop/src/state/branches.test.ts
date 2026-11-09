import { describe, expect, test } from "bun:test";
import type { SessionSummary } from "../acp/types";
import { branchesOf, branchKindOf, branchLine, branchRows, isBranch, originLine, originOf } from "./branches";

const session = (id: string, over: Partial<SessionSummary> = {}): SessionSummary => ({
  id,
  cwd: "/repo",
  title: id,
  ...over,
});

const threads = [
  session("root", { title: "Rework the sidebar" }),
  session("fork", { parent: { id: "root", title: "Rework the sidebar", relationship: "fork" } }),
  session("side", { parent: { id: "root", title: "Rework the sidebar", relationship: "side_chat" } }),
  session("delegate", { parent: { id: "root", title: "Rework the sidebar", relationship: "delegate" } }),
  session("old-delegate", { parent: { id: "root" } }),
  session("gone", { title: "", parent: { id: "missing", title: "Deleted thread", relationship: "fork" } }),
];

describe("forks and side chats", () => {
  test("only a fork or a side chat is a branch, never a delegate", () => {
    expect(threads.map((t) => [t.id, isBranch(t)])).toEqual([
      ["root", false],
      ["fork", true],
      ["side", true],
      ["delegate", false],
      ["old-delegate", false],
      ["gone", true],
    ]);
    expect(branchKindOf({ id: "x", relationship: "other" })).toBeUndefined();
    expect(branchKindOf({ id: "x" })).toBeUndefined();
  });

  test("a parent lists its branches in a stable order, without its delegates", () => {
    expect(branchesOf(threads, "root")).toEqual([
      { id: "fork", title: "fork", kind: "fork" },
      { id: "side", title: "side", kind: "side_chat" },
    ]);
    expect(branchesOf([...threads].reverse(), "root").map((b) => b.id)).toEqual(["fork", "side"]);
    expect(branchesOf(threads, "fork")).toEqual([]);
  });

  test("a branch names where it came from, as the parent is now", () => {
    expect(originOf(threads, "fork")).toEqual({
      id: "root",
      title: "Rework the sidebar",
      kind: "fork",
      listed: true,
    });
    expect(originOf(threads, "side")?.kind).toBe("side_chat");
    // A renamed parent shows its new title; a deleted one keeps the name it had, and cannot be opened.
    expect(
      originOf([...threads.slice(0, 1).map((t) => ({ ...t, title: "Renamed" })), ...threads.slice(1)], "fork")
        ?.title,
    ).toBe("Renamed");
    expect(originOf(threads, "gone")).toEqual({
      id: "missing",
      title: "Deleted thread",
      kind: "fork",
      listed: false,
    });
  });

  test("a thread that is not a branch has no origin", () => {
    expect(originOf(threads, "root")).toBeUndefined();
    expect(originOf(threads, "delegate")).toBeUndefined();
    expect(originOf(threads, "nope")).toBeUndefined();
  });

  test("the lines the transcript and the header draw", () => {
    expect(branchLine({ kind: "fork", title: "A" })).toEqual({ label: "Forked to", title: "A" });
    expect(branchLine({ kind: "side_chat", title: "B" })).toEqual({ label: "Side chat", title: "B" });
    expect(originLine({ kind: "fork", title: "A" })).toEqual({ label: "Forked from", title: "A" });
    expect(originLine({ kind: "side_chat", title: "B" })).toEqual({ label: "Side chat on", title: "B" });
  });
});

describe("the branches drawn under a row", () => {
  const kids = (map: Record<string, SessionSummary[]>) => (id: string) => map[id] ?? [];
  const fork = (id: string, parent: string) => session(id, { parent: { id: parent, relationship: "fork" } });
  const side = (id: string, parent: string) =>
    session(id, { parent: { id: parent, relationship: "side_chat" } });
  const delegate = (id: string, parent: string) => session(id, { parent: { id: parent } });

  test("a branch of a branch is drawn one level in", () => {
    const f = fork("f1", "root");
    const ff = fork("f2", "f1");
    const { rows, more } = branchRows("root", kids({ root: [f], f1: [ff] }), new Set(), 6);
    expect(rows.map((r) => [r.session.id, r.depth])).toEqual([
      ["f1", 1],
      ["f2", 2],
    ]);
    expect(more).toBe(0);
  });

  test("a side chat of a side chat, and one opened from a delegate the list does not draw", () => {
    const s1 = side("s1", "root");
    const s2 = side("s2", "s1");
    const d = delegate("d1", "root");
    const sd = side("s3", "d1");
    const { rows } = branchRows("root", kids({ root: [s1, d], s1: [s2], d1: [sd] }), new Set(), 6);
    // The delegate has no row; what was opened from it hangs from the same row.
    expect(rows.map((r) => [r.session.id, r.depth])).toEqual([
      ["s1", 1],
      ["s2", 2],
      ["s3", 1],
    ]);
  });

  test("past the cap the newest stay and the rest are counted", () => {
    const list = ["a", "b", "c", "d", "e", "f", "g", "h"].map((id) => fork(id, "root"));
    const { rows, more } = branchRows("root", kids({ root: list }), new Set(), 6);
    expect(rows.map((r) => r.session.id)).toEqual(["c", "d", "e", "f", "g", "h"]);
    expect(more).toBe(2);
    expect(branchRows("root", kids({ root: list }), new Set(), Number.POSITIVE_INFINITY).more).toBe(0);
  });

  test("the open chain is drawn another way, and a loop in the records ends", () => {
    const a = fork("a", "root");
    const b = fork("b", "a");
    const { rows } = branchRows("root", kids({ root: [a], a: [b] }), new Set(["a"]), 6);
    expect(rows.map((r) => r.session.id)).toEqual(["b"]);
    const loop = branchRows(
      "root",
      kids({ root: [a], a: [fork("c", "a")], c: [a, fork("root2", "c")] }),
      new Set(),
      6,
    );
    expect(loop.rows.map((r) => r.session.id)).toEqual(["a", "c", "root2"]);
  });
});
