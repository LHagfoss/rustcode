import { describe, expect, test } from "bun:test";
import type { FileDiff } from "../acp/types";
import {
  contextRow,
  keepListed,
  nextRow,
  rangeRows,
  rowKeyAction,
  threadDiff,
  visibleRows,
} from "./sidebarRows";
import type { TranscriptItem } from "./thread";

const ROWS = ["a", "b", "c", "d", "e"];

describe("the rows the list draws", () => {
  test("sections keep their order, and a shut one draws none of its rows", () => {
    expect(
      visibleRows([
        { id: "working", rows: [{ id: "w1" }, { id: "w2" }] },
        { id: "settled", shut: true, rows: [{ id: "s1" }] },
        { id: "threads", rows: [{ id: "a" }] },
      ]),
    ).toEqual(["w1", "w2", "a"]);
  });

  test("a grouped section draws its open projects, each project's shut one draw nothing", () => {
    expect(
      visibleRows([
        {
          id: "threads",
          groups: [
            { key: "/one", rows: [{ id: "a" }] },
            { key: "/two", shut: true, rows: [{ id: "b" }] },
            { key: "/three", rows: [{ id: "c" }] },
          ],
        },
      ]),
    ).toEqual(["a", "c"]);
  });

  test("a delegated thread has no row of its own: only what is drawn is drawn", () => {
    const rows = [{ id: "p" }, { id: "q" }, { id: "r" }];
    expect(visibleRows([{ id: "threads", rows }])).toEqual(["p", "q", "r"]);
    // The tail limit counts rows.
    expect(visibleRows([{ id: "settled", rows, limit: 2 }])).toEqual(["p", "q"]);
  });

  test("nothing drawn is nothing to move through", () => {
    expect(visibleRows([{ id: "settled", shut: true, rows: [{ id: "a" }] }])).toEqual([]);
  });
});

describe("the open thread's context", () => {
  const parentOf = new Map([
    ["child", "parent"],
    ["grandchild", "child"],
  ]);

  test("the open thread's own row when it has one", () => {
    expect(contextRow("a", ROWS, parentOf)).toBe("a");
    expect(contextRow(null, ROWS, parentOf)).toBeNull();
  });

  test("a delegated thread answers with the nearest ancestor that is drawn", () => {
    // Both children have no row; the parent does, and it keeps the highlight.
    expect(contextRow("child", ["parent", "b"], parentOf)).toBe("parent");
    expect(contextRow("grandchild", ["parent", "b"], parentOf)).toBe("parent");
    // A delegation tree with no row at all has no context, so the keyboard starts at the top.
    expect(contextRow("grandchild", ["b"], parentOf)).toBeNull();
  });
});

describe("moving the highlight", () => {
  test("↓ and ↑ step one row and stop at the ends", () => {
    expect(nextRow(ROWS, "a", 1)).toBe("b");
    expect(nextRow(ROWS, "e", 1)).toBe("e");
    expect(nextRow(ROWS, "a", -1)).toBe("a");
    expect(nextRow(ROWS, "c", -1)).toBe("b");
  });

  test("with no highlight, ↓ starts at the top and ↑ at the bottom", () => {
    expect(nextRow(ROWS, null, 1)).toBe("a");
    expect(nextRow(ROWS, null, -1)).toBe("e");
  });

  test("a highlight that left the list starts again from its end", () => {
    expect(nextRow(ROWS, "gone", 1)).toBe("a");
    expect(nextRow(ROWS, "gone", -1)).toBe("e");
    expect(nextRow([], null, 1)).toBeNull();
  });
});

describe("selecting a range", () => {
  test("a range runs both ways and includes both ends", () => {
    expect(rangeRows(ROWS, "b", "d")).toEqual(["b", "c", "d"]);
    expect(rangeRows(ROWS, "d", "b")).toEqual(["b", "c", "d"]);
    expect(rangeRows(ROWS, "c", "c")).toEqual(["c"]);
  });

  test("without an anchor the range is the row itself", () => {
    expect(rangeRows(ROWS, null, "c")).toEqual(["c"]);
  });

  test("an end that is not drawn gives nothing", () => {
    expect(rangeRows(ROWS, "a", "gone")).toEqual([]);
    expect(rangeRows(ROWS, "gone", "c")).toEqual(["c"]);
  });
});

describe("dropping what left the list", () => {
  test("threads the list no longer draws leave the selection", () => {
    expect([...keepListed(new Set(["a", "b", "gone"]), ROWS)]).toEqual(["a", "b"]);
  });

  test("the same set comes back when nothing left, so nothing re-renders", () => {
    const selected = new Set(["a", "c"]);
    expect(keepListed(selected, ROWS)).toBe(selected);
  });
});

describe("the keys the list answers", () => {
  test("↑ ↓ ⏎ and Esc are the list's", () => {
    expect(rowKeyAction({ key: "ArrowDown" })).toEqual({ kind: "move", delta: 1 });
    expect(rowKeyAction({ key: "ArrowUp" })).toEqual({ kind: "move", delta: -1 });
    expect(rowKeyAction({ key: "Enter" })).toEqual({ kind: "open" });
    expect(rowKeyAction({ key: "Escape" })).toEqual({ kind: "clear" });
  });

  test("a field inside the list keeps its keys", () => {
    expect(rowKeyAction({ key: "ArrowDown", editable: true })).toBeNull();
    expect(rowKeyAction({ key: "Enter", editable: true })).toBeNull();
  });

  test("⌘ ⌃ ⌥ and other keys belong to the app", () => {
    expect(rowKeyAction({ key: "ArrowDown", metaKey: true })).toBeNull();
    expect(rowKeyAction({ key: "Enter", ctrlKey: true })).toBeNull();
    expect(rowKeyAction({ key: "Escape", altKey: true })).toBeNull();
    expect(rowKeyAction({ key: "a" })).toBeNull();
  });
});

describe("a thread's diff totals", () => {
  const diff = (added: number, removed: number): FileDiff => ({
    path: "a.ts",
    oldText: Array.from({ length: removed }, (_, i) => `old ${i}`).join("\n"),
    newText: Array.from({ length: added }, (_, i) => `new ${i}`).join("\n"),
    oldStart: 1,
    newStart: 1,
  });
  const tool = (id: string, diffs: FileDiff[] = []): TranscriptItem =>
    ({ id, kind: "tool", title: "Edit", status: "completed", diffs }) as unknown as TranscriptItem;

  test("adds up every call that changed a file", () => {
    expect(threadDiff([tool("1", [diff(3, 1)]), tool("2", [diff(0, 2)])])).toEqual({ added: 3, removed: 3 });
    expect(threadDiff([tool("1", [diff(3, 1)]), tool("2", [diff(2, 0)])])).toEqual({
      added: 5,
      removed: 1,
    });
  });

  test("a thread with nothing read yet has no totals", () => {
    expect(threadDiff(undefined)).toBeNull();
    expect(threadDiff([])).toBeNull();
    expect(threadDiff([tool("1")])).toBeNull();
  });
});
