import { describe, expect, test } from "bun:test";
import type { SessionSummary } from "../acp/types";
import {
  advanceDrag,
  applyOrder,
  DRAG_THRESHOLD,
  keepThreads,
  moveBy,
  reorder,
  sectionOrderKey,
  startDrag,
  swallowsClick,
  withoutThreads,
} from "./sidebarOrder";

const row = (id: string): SessionSummary => ({ id, cwd: "/code/rustcode" });
const ids = (sessions: SessionSummary[]) => sessions.map((s) => s.id);

describe("the order a section draws", () => {
  test("nothing arranged keeps the section's own order", () => {
    expect(ids(applyOrder([row("a"), row("b")], undefined))).toEqual(["a", "b"]);
    expect(ids(applyOrder([row("a"), row("b")], []))).toEqual(["a", "b"]);
  });

  test("arranged ids come first in their arranged order, new ones after them", () => {
    const items = [row("new"), row("b"), row("a")];
    expect(ids(applyOrder(items, ["a", "b"]))).toEqual(["a", "b", "new"]);
    // Several threads that arrived since: they keep the order the section gave them.
    expect(ids(applyOrder([row("x"), row("y"), row("b"), row("a")], ["a", "b"]))).toEqual([
      "a",
      "b",
      "x",
      "y",
    ]);
  });

  test("ids the section does not draw are skipped, and a duplicate in storage is ignored", () => {
    expect(ids(applyOrder([row("c"), row("a")], ["a", "b", "a", "c"]))).toEqual(["a", "c"]);
  });

  test("a section split by project keeps one order for each project", () => {
    expect(sectionOrderKey("threads")).toBe("threads");
    expect(sectionOrderKey("threads", "/code/a")).toBe("threads:/code/a");
  });
});

describe("moving a row", () => {
  const drawn = ["a", "b", "c", "d"];

  test("to the top, the middle and the bottom", () => {
    expect(reorder([], drawn, "c", 0)).toEqual(["c", "a", "b", "d"]);
    expect(reorder([], drawn, "a", 2)).toEqual(["b", "c", "a", "d"]);
    expect(reorder([], drawn, "a", 3)).toEqual(["b", "c", "d", "a"]);
    // Dropping a row back where it was changes nothing.
    expect(reorder([], drawn, "b", 1)).toEqual(drawn);
  });

  test("moves from the order the user already arranged, not from what the section gave", () => {
    // Arranged d, c, b, a but only a, b, c are drawn: d keeps the front, a moves inside it.
    expect(reorder(["d", "c", "b", "a"], ["a", "b", "c"], "a", 0)).toEqual(["d", "a", "c", "b"]);
    expect(reorder(["d", "c", "b", "a"], ["a", "b", "c"], "c", 0)).toEqual(["d", "c", "b", "a"]);
  });

  test("a thread the section is not drawing keeps its place", () => {
    // Searching for two of four rows: the other two are not rearranged by the one that moves.
    expect(reorder(["a", "b", "c", "d"], ["a", "c"], "c", 0)).toEqual(["c", "a", "b", "d"]);
    // The end of the drawn list is the end of the drawn rows, not of every arranged one.
    expect(reorder(["a", "b", "c", "d"], ["a", "c"], "a", 1)).toEqual(["b", "c", "a", "d"]);
  });

  test("⌥↑/↓ moves one place, and stops at either end", () => {
    expect(moveBy([], drawn, "c", -1)).toEqual(["a", "c", "b", "d"]);
    expect(moveBy([], drawn, "c", 1)).toEqual(["a", "b", "d", "c"]);
    expect(moveBy([], drawn, "a", -1)).toBeNull();
    expect(moveBy([], drawn, "d", 1)).toBeNull();
    // A row that is not drawn has nowhere to go.
    expect(moveBy([], drawn, "z", 1)).toBeNull();
  });
});

describe("a press that becomes a drag", () => {
  test("a still pointer is a click, and a small shiver still is", () => {
    const press = startDrag(20, 120);
    expect(press.phase).toBe("pending");
    const still = advanceDrag(press, 20, 120);
    // The same phase back: nothing to redraw, and nothing to suppress.
    expect(still.next).toBe(press);
    expect(still.began).toBe(false);
    expect(advanceDrag(press, 20 + DRAG_THRESHOLD, 120).next).toBe(press);
    expect(swallowsClick(press)).toBe(false);
  });

  test("past the threshold it is a drag, and stays one back over the row", () => {
    const press = startDrag(20, 120);
    const crossed = advanceDrag(press, 20, 120 + DRAG_THRESHOLD + 1);
    expect(crossed.began).toBe(true);
    expect(crossed.next.phase).toBe("dragging");
    expect(swallowsClick(crossed.next)).toBe(true);
    // Returning to where it started does not turn it back into a click, and is not a second
    // beginning.
    expect(advanceDrag(crossed.next, 20, 120).next.phase).toBe("dragging");
    expect(advanceDrag(crossed.next, 20, 120).began).toBe(false);
    // Sideways counts too: the row is dragged by its whole press.
    expect(advanceDrag(press, 20 + DRAG_THRESHOLD + 1, 120).began).toBe(true);
  });

  test("several moves in a row: one beginning, then every move carries the row", () => {
    // The bug the phase alone cannot catch: from the move that crosses the threshold on, the
    // phase stops changing, so a caller that only compared phases stopped carrying the row.
    let press = startDrag(20, 120);
    const moves = [121, 126, 180, 320, 640];
    const phases: string[] = [];
    const beginnings: boolean[] = [];
    for (const y of moves) {
      const step = advanceDrag(press, 20, y);
      press = step.next;
      phases.push(press.phase);
      beginnings.push(step.began);
    }
    expect(phases).toEqual(["pending", "dragging", "dragging", "dragging", "dragging"]);
    expect(beginnings).toEqual([false, true, false, false, false]);
    // Each move still measures from where the press went down, so the row follows the pointer.
    expect(moves.map((y) => y - press.y)).toEqual([1, 6, 60, 200, 520]);
  });

  test("a custom threshold is the caller's", () => {
    const press = startDrag(0, 0);
    expect(advanceDrag(press, 0, 3, 2).began).toBe(true);
    expect(advanceDrag(press, 0, 3, 8).next).toBe(press);
  });
});

describe("an arrangement that forgets a thread", () => {
  const order = { pinned: ["a", "b"], threads: ["b", "c"], "threads:/code/b": ["c"] };

  test("a trashed thread leaves every section", () => {
    expect(withoutThreads(order, ["b"])).toEqual({
      pinned: ["a"],
      threads: ["c"],
      "threads:/code/b": ["c"],
    });
  });

  test("nothing to forget keeps the same value, so nothing is written", () => {
    expect(withoutThreads(order, ["z"])).toBe(order);
    expect(withoutThreads(order, [])).toBe(order);
  });

  test("a section that loses its last id goes with it", () => {
    expect(withoutThreads({ pinned: ["a"], threads: ["b", "c"] }, ["a"])).toEqual({
      threads: ["b", "c"],
    });
  });

  test("the engine's list keeps only the threads it names", () => {
    expect(keepThreads(order, (id) => id !== "c")).toEqual({ pinned: ["a", "b"], threads: ["b"] });
    // Every id still there: the stored value is handed back untouched.
    expect(keepThreads(order, () => true)).toBe(order);
    expect(keepThreads(order, () => false)).toEqual({});
  });
});
