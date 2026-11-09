import { describe, expect, test } from "bun:test";
import type { FileDiff } from "../acp/types";
import {
  contextRow,
  isShelf,
  keepListed,
  listedPicks,
  nextRow,
  opensRowMenu,
  type Placement,
  placeThread,
  rangeRows,
  rowKeyAction,
  rowStatus,
  SECTION_ORDER,
  SHELF_DEFAULT_OPEN,
  SHELVES,
  shelfKey,
  shelfOpen,
  threadDiff,
  titleFirstTrailing,
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

describe("the selection bar's threads", () => {
  test("only picked threads still in the list are acted on", () => {
    const sessions = [
      { id: "a", cwd: "/one" },
      { id: "b", cwd: "/two" },
    ];
    expect(listedPicks(sessions, new Set(["a", "b", "gone"])).map((s) => s.id)).toEqual(["a", "b"]);
    expect(listedPicks(sessions, new Set(["gone"]))).toEqual([]);
    expect(listedPicks(sessions, new Set())).toEqual([]);
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

describe("the keys that open a row's menu", () => {
  test("Shift+F10 and the ContextMenu key, and nothing else", () => {
    expect(opensRowMenu({ key: "F10", shiftKey: true })).toBe(true);
    expect(opensRowMenu({ key: "ContextMenu" })).toBe(true);
    // F10 alone is the window's, and Shift with another key is not this.
    expect(opensRowMenu({ key: "F10" })).toBe(false);
    expect(opensRowMenu({ key: "F2", shiftKey: true })).toBe(false);
    expect(opensRowMenu({ key: "Enter" })).toBe(false);
  });
});

describe("a row's one status chip", () => {
  const idle = { running: false };

  test("an idle or settled row has no chip: it shows its age instead", () => {
    expect(rowStatus(idle)).toBeUndefined();
    expect(rowStatus({ running: false, failed: false, review: false })).toBeUndefined();
  });

  test("a request outranks everything, and says whether it is a question or an approval", () => {
    const read = { running: true, waiting: { label: "Waiting on 1 thread", title: "Ship it" }, failed: true };
    expect(rowStatus({ ...read, request: { title: "edit src/app.ts" } })).toEqual({
      kind: "needs",
      label: "Needs you",
      title: "Approval: edit src/app.ts",
      approval: true,
    });
    expect(
      rowStatus({ ...read, request: { title: "Which runner?", question: { text: "Which test runner?" } } }),
    ).toEqual({ kind: "needs", label: "Needs you", title: "Which test runner?", approval: false });
  });

  test("working outranks waiting, and waiting outranks the failure it is clearing up", () => {
    const waiting = { label: "Waiting on 2 threads", title: "Ship it, Review it" };
    expect(rowStatus({ running: true, waiting, failed: true })).toEqual({
      kind: "working",
      label: "Working",
    });
    expect(rowStatus({ running: false, waiting, failed: true })).toEqual({
      kind: "waiting",
      label: "Waiting",
      title: "Waiting on 2 threads: Ship it, Review it",
    });
  });

  test("a wait says what it waits on, and one kind of wait needs no list", () => {
    expect(
      rowStatus({ running: false, waiting: { label: "Waiting on a background command", title: "" } }),
    ).toEqual({
      kind: "waiting",
      label: "Waiting",
      title: "Waiting on a background command",
    });
  });

  test("a failure, then a review, each in its own words", () => {
    expect(rowStatus({ running: false, failed: true, review: true })).toEqual({
      kind: "failed",
      label: "Failed",
      title: "Its last turn failed",
    });
    expect(rowStatus({ running: false, review: true })).toEqual({
      kind: "review",
      label: "Review",
      title: "It waits for your review",
    });
  });
});

describe("a thread's diff totals", () => {
  const diff = (added: number, removed: number, path = "a.ts"): FileDiff => ({
    path,
    oldText: Array.from({ length: removed }, (_, i) => `old ${i}`).join("\n"),
    newText: Array.from({ length: added }, (_, i) => `new ${i}`).join("\n"),
    oldStart: 1,
    newStart: 1,
  });
  const tool = (id: string, diffs: FileDiff[] = []): TranscriptItem =>
    ({ id, kind: "tool", title: "Edit", status: "completed", diffs }) as unknown as TranscriptItem;

  test("a file counts once, by the widest edit it was given", () => {
    // Going back to the same file never adds its lines again: a row read +12404.
    expect(threadDiff([tool("1", [diff(3, 1)]), tool("2", [diff(0, 2)])])).toEqual({ added: 3, removed: 2 });
    expect(threadDiff([tool("1", [diff(3, 1)]), tool("2", [diff(2, 0)])])).toEqual({ added: 3, removed: 1 });
    const many = Array.from({ length: 40 }, (_, i) => tool(String(i), [diff(10, 10)]));
    expect(threadDiff(many)).toEqual({ added: 10, removed: 10 });
  });

  test("different files add up", () => {
    expect(threadDiff([tool("1", [diff(3, 1, "a.ts")]), tool("2", [diff(2, 4, "b.ts")])])).toEqual({
      added: 5,
      removed: 5,
    });
    expect(
      threadDiff([tool("1", [diff(3, 1, "a.ts")]), tool("2", [diff(9, 0, "a.ts"), diff(2, 4, "b.ts")])]),
    ).toEqual({ added: 11, removed: 5 });
  });

  test("a thread with nothing read yet has no totals", () => {
    expect(threadDiff(undefined)).toBeNull();
    expect(threadDiff([])).toBeNull();
    expect(threadDiff([tool("1")])).toBeNull();
  });
});

describe("the shelves' fold state", () => {
  test("Working starts open, Snoozed and Settled shut", () => {
    expect(shelfOpen("working", undefined)).toBe(true);
    expect(shelfOpen("snoozed", undefined)).toBe(false);
    expect(shelfOpen("settled", undefined)).toBe(false);
    expect(SHELF_DEFAULT_OPEN).toEqual({ working: true, snoozed: false, settled: false });
  });

  test("every shelf folds and unfolds on the user's choice, not only Settled", () => {
    for (const id of SHELVES) {
      expect(shelfOpen(id, true)).toBe(true);
      expect(shelfOpen(id, false)).toBe(false);
    }
  });

  test("a search shows every shelf, and the delegate chain opens the shelf it hangs in", () => {
    expect(shelfOpen("snoozed", false, { searching: true })).toBe(true);
    expect(shelfOpen("settled", false, { forced: true })).toBe(true);
    expect(shelfOpen("working", false, {})).toBe(false);
  });

  test("each shelf keeps its own key, Settled's the one older releases wrote", () => {
    expect(shelfKey("settled")).toBe("rustcode.sidebar.settledOpen");
    expect(new Set(SHELVES.map(shelfKey)).size).toBe(SHELVES.length);
  });

  test("only the shelves fold", () => {
    expect(SHELVES.every(isShelf)).toBe(true);
    expect(isShelf("pinned")).toBe(false);
    expect(isShelf("threads")).toBe(false);
    expect(isShelf("project:/code/app")).toBe(false);
  });
});

describe("which section a thread is drawn under", () => {
  const base: Placement = {
    pinned: false,
    settled: false,
    aged: false,
    snoozed: false,
    needs: false,
    working: false,
    fresh: false,
    showWorking: true,
    byProject: false,
  };
  const place = (over: Partial<Placement>) => placeThread({ ...base, ...over });

  test("Pinned and Active lead, the shelves follow at the foot", () => {
    expect(SECTION_ORDER).toEqual(["pinned", "threads", "working", "snoozed", "settled"]);
  });

  test("an idle thread is Active, a pinned one Pinned", () => {
    expect(place({})).toBe("threads");
    expect(place({ pinned: true })).toBe("pinned");
  });

  test("a pinned thread that works stays Pinned, never under Working", () => {
    expect(place({ pinned: true, working: true })).toBe("pinned");
  });

  test("a settled thread is never under Pinned", () => {
    expect(place({ pinned: true, settled: true })).toBe("settled");
    // …unless it needs you or works again: then it is shown where it can be seen.
    expect(place({ pinned: true, settled: true, needs: true })).toBe("pinned");
    expect(place({ pinned: true, settled: true, working: true })).toBe("pinned");
  });

  test("running threads go under Working, unless it is off or the list is grouped by project", () => {
    expect(place({ working: true })).toBe("working");
    expect(place({ working: true, showWorking: false })).toBe("threads");
    expect(place({ working: true, byProject: true })).toBe("threads");
  });

  test("a snoozed thread sleeps until it needs you or works", () => {
    expect(place({ snoozed: true })).toBe("snoozed");
    expect(place({ snoozed: true, needs: true })).toBe("threads");
    expect(place({ snoozed: true, working: true })).toBe("working");
  });

  test("a thread untouched for days is Settled, a freshly finished one is not", () => {
    expect(place({ aged: true })).toBe("settled");
    expect(place({ aged: true, fresh: true })).toBe("threads");
    expect(place({ aged: true, needs: true })).toBe("threads");
  });

  test("every section id a thread can be placed in is one the list draws", () => {
    const placed = [
      place({}),
      place({ pinned: true }),
      place({ working: true }),
      place({ snoozed: true }),
      place({ settled: true }),
    ];
    for (const id of placed) expect(SECTION_ORDER).toContain(id);
  });
});

describe("titleFirstTrailing", () => {
  test("prioritizes attention, running, done, status, then age", () => {
    expect(titleFirstTrailing({ needs: true, running: true, done: true, status: true })).toBe("needs");
    expect(titleFirstTrailing({ needs: false, running: true, done: true, status: true })).toBe("running");
    expect(titleFirstTrailing({ needs: false, running: false, done: true, status: true })).toBe("done");
    expect(titleFirstTrailing({ needs: false, running: false, done: false, status: true })).toBe("status");
    expect(titleFirstTrailing({ needs: false, running: false, done: false, status: false })).toBe("age");
  });
});
