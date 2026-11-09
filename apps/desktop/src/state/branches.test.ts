import { describe, expect, test } from "bun:test";
import type { SessionSummary } from "../acp/types";
import { branchesOf, branchKindOf, branchLine, isBranch, originLine, originOf } from "./branches";

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
