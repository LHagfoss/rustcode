import { describe, expect, test } from "bun:test";
import type { FileDiff } from "../acp/types";
import {
  cleanFolds,
  contextRow,
  isShelf,
  keepListed,
  listedPicks,
  nextRow,
  type OrderKey,
  opensRowMenu,
  type Placement,
  placeThread,
  rangeRows,
  rowKeyAction,
  rowMeta,
  rowStatus,
  SHELF_DEFAULT_OPEN,
  sectionLabelled,
  sectionsOf,
  shelfKey,
  shelfOpen,
  shelvesOf,
  sortSection,
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

  test("a failure, then a completion you have not seen, then a review, each in its own words", () => {
    expect(rowStatus({ running: false, failed: true, review: true })).toEqual({
      kind: "failed",
      label: "Failed",
      title: "Its last turn failed",
    });
    expect(rowStatus({ running: false, failed: true, fresh: true })).toEqual({
      kind: "failed",
      label: "Failed",
      title: "Its last turn failed",
    });
    expect(rowStatus({ running: false, fresh: true, review: true })).toEqual({
      kind: "completed",
      label: "Done",
      title: "It finished while you were elsewhere",
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
    for (const id of shelvesOf("shelf")) {
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
    expect(new Set(shelvesOf("shelf").map(shelfKey)).size).toBe(shelvesOf("shelf").length);
  });

  test("the default layout folds only the foot shelves: Working leads it", () => {
    expect(shelvesOf("top")).toEqual(["snoozed", "settled"]);
    expect(shelvesOf("shelf")).toEqual(["working", "snoozed", "settled"]);
  });

  test("only the shelves fold, and which ones depends on the layout", () => {
    expect(shelvesOf("shelf").every((id) => isShelf(id, "shelf"))).toBe(true);
    expect(isShelf("working", "top")).toBe(false);
    expect(isShelf("working", "shelf")).toBe(true);
    expect(isShelf("pinned", "shelf")).toBe(false);
    expect(isShelf("needs", "top")).toBe(false);
    expect(isShelf("threads", "top")).toBe(false);
    expect(isShelf("project:/code/app", "top")).toBe(false);
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
    layout: "top",
  };
  const place = (over: Partial<Placement>) => placeThread({ ...base, ...over });

  test("the default layout is Pinned above one Threads list, then the shelves", () => {
    expect(sectionsOf("top")).toEqual(["pinned", "threads", "snoozed", "settled"]);
    // The layout before it keeps Working as a shelf at the foot, between Threads and Snoozed.
    expect(sectionsOf("shelf")).toEqual(["pinned", "threads", "working", "snoozed", "settled"]);
  });

  test("an idle thread is in Threads, a pinned one in Pinned", () => {
    expect(place({})).toBe("threads");
    expect(place({ pinned: true })).toBe("pinned");
  });

  test("a pinned thread that works stays Pinned, never in Working", () => {
    expect(place({ pinned: true, working: true })).toBe("pinned");
    expect(place({ pinned: true, working: true, layout: "shelf" })).toBe("pinned");
  });

  test("a settled thread is never under Pinned", () => {
    expect(place({ pinned: true, settled: true })).toBe("settled");
    // …unless it needs you or works again: then it is shown where it can be seen.
    expect(place({ pinned: true, settled: true, needs: true })).toBe("pinned");
    expect(place({ pinned: true, settled: true, working: true })).toBe("pinned");
  });

  test("running threads join the Threads list, which the shelf layout still gives its own shelf", () => {
    expect(place({ working: true })).toBe("threads");
    expect(place({ working: true, layout: "shelf" })).toBe("working");
  });

  test("an approval or a question keeps the thread in Threads, with its chip", () => {
    expect(place({ needs: true })).toBe("threads");
    expect(place({ needs: true, working: true })).toBe("threads");
    expect(place({ needs: true, aged: true })).toBe("threads");
    expect(place({ needs: true, snoozed: true })).toBe("threads");
    expect(place({ needs: true, settled: true, fresh: true })).toBe("threads");
    expect(place({ needs: true, layout: "shelf" })).toBe("threads");
  });

  test("a snoozed thread sleeps until it needs you or works", () => {
    expect(place({ snoozed: true })).toBe("snoozed");
    expect(place({ snoozed: true, needs: true })).toBe("threads");
    expect(place({ snoozed: true, working: true })).toBe("threads");
    expect(place({ snoozed: true, working: true, layout: "shelf" })).toBe("working");
  });

  test("a thread untouched for days is Settled, a freshly finished one is not", () => {
    expect(place({ aged: true })).toBe("settled");
    expect(place({ aged: true, fresh: true })).toBe("threads");
    expect(place({ aged: true, needs: true })).toBe("threads");
  });

  test("every section id a thread can be placed in is one the layout draws", () => {
    // Every combination of the flags, in both layouts: nothing can be placed where nothing is drawn.
    for (const layout of ["top", "shelf"] as const) {
      for (const pinned of [false, true])
        for (const settled of [false, true])
          for (const aged of [false, true])
            for (const snoozed of [false, true])
              for (const needs of [false, true])
                for (const working of [false, true])
                  for (const fresh of [false, true]) {
                    const id = place({ pinned, settled, aged, snoozed, needs, working, fresh, layout });
                    expect(sectionsOf(layout)).toContain(id);
                  }
    }
  });

  test("neither layout draws a separate Needs you section: the chip is the whole of it", () => {
    expect(sectionsOf("top") as readonly string[]).not.toContain("needs");
    expect(sectionsOf("shelf") as readonly string[]).not.toContain("needs");
  });
});

describe("the headings the list draws", () => {
  test("the shelves always name themselves, and Pinned names itself while it has rows", () => {
    expect(sectionLabelled("snoozed", ["threads", "snoozed"] as const, "top")).toBe(true);
    expect(sectionLabelled("pinned", ["pinned"] as const, "top")).toBe(true);
  });

  test("Threads names itself only while Pinned is drawn above it", () => {
    expect(sectionLabelled("threads", ["pinned", "threads"] as const, "top")).toBe(true);
    expect(sectionLabelled("threads", ["threads", "settled"] as const, "top")).toBe(false);
    expect(sectionLabelled("threads", ["threads"] as const, "top")).toBe(false);
  });
});

describe("the folds the list keeps", () => {
  test("a stale section id is dropped and a project's own folder stays", () => {
    expect(cleanFolds(["working", "threads", "project:/code/app", "needs", "settled"])).toEqual([
      "project:/code/app",
    ]);
    expect(cleanFolds(["project:/one", "project:/two"])).toEqual(["project:/one", "project:/two"]);
    expect(cleanFolds([])).toEqual([]);
  });
});

describe("the order a section keeps", () => {
  type Row = { id: string; at: OrderKey };
  const rows = (list: [string, OrderKey][]): Row[] => list.map(([id, at]) => ({ id, at }));
  const ids = (list: Row[]) => list.map((r) => r.id);
  const order = (section: Parameters<typeof sortSection>[0], list: Row[]) =>
    ids(sortSection(section, list, (r) => r.at));

  test("Working puts the thread the user last sent work to first", () => {
    const list = rows([
      ["older", { backAt: 10, sentAt: 100 }],
      ["newest", { backAt: 20, sentAt: 300 }],
      ["middle", { backAt: 30, sentAt: 200 }],
    ]);
    expect(order("working", list)).toEqual(["newest", "middle", "older"]);
  });

  test("a working thread with no turn of its own falls back to when it last moved", () => {
    const list = rows([
      ["waiting", { backAt: 50 }],
      ["sent", { backAt: 10, sentAt: 40 }],
    ]);
    expect(order("working", list)).toEqual(["waiting", "sent"]);
  });

  test("Threads leads with what runs, then what asks you, then the rest", () => {
    const list = rows([
      ["idle", { backAt: 100 }],
      ["asking-recent", { backAt: 10, askedAt: 900, needs: true }],
      ["running-earlier", { backAt: 5, sentAt: 200, running: true }],
      ["asking-oldest", { backAt: 20, askedAt: 100, needs: true }],
      ["running-latest", { backAt: 1, sentAt: 300, running: true }],
    ]);
    expect(order("threads", list)).toEqual([
      "running-latest",
      "running-earlier",
      "asking-oldest",
      "asking-recent",
      "idle",
    ]);
  });

  test("a request with no recorded arrival reads by how recently it moved", () => {
    const list = rows([
      ["quiet", { backAt: 10 }],
      ["asking", { backAt: 20, askedAt: 5, needs: true }],
      ["fresh", { backAt: 30 }],
    ]);
    expect(order("threads", list)).toEqual(["asking", "fresh", "quiet"]);
  });

  test("the rest of Threads is the last one back first, with what came back unseen above it all", () => {
    const list = rows([
      ["read-hours-ago", { backAt: 100 }],
      ["unseen-yesterday", { backAt: 50, unseen: true }],
      ["read-newest", { backAt: 300 }],
      ["unseen-just-now", { backAt: 400, unseen: true }],
    ]);
    expect(order("threads", list)).toEqual([
      "unseen-just-now",
      "unseen-yesterday",
      "read-newest",
      "read-hours-ago",
    ]);
  });

  test("Pinned follows the pin order and Snoozed the soonest wake, unmarked rows last", () => {
    const pins = rows([
      ["third", { backAt: 1, pinRank: 2 }],
      ["first", { backAt: 2, pinRank: 0 }],
      ["unpinned", { backAt: 3 }],
      ["second", { backAt: 4, pinRank: 1 }],
    ]);
    expect(order("pinned", pins)).toEqual(["first", "second", "third", "unpinned"]);
    const sleeps = rows([
      ["tomorrow", { backAt: 1, wakeAt: 90 }],
      ["an-hour", { backAt: 2, wakeAt: 10 }],
      ["no-wake", { backAt: 3 }],
    ]);
    expect(order("snoozed", sleeps)).toEqual(["an-hour", "tomorrow", "no-wake"]);
  });

  test("Settled keeps the order the engine gave it", () => {
    const list = rows([
      ["old", { backAt: 1 }],
      ["new", { backAt: 900 }],
      ["middle", { backAt: 5 }],
    ]);
    expect(order("settled", list)).toEqual(["old", "new", "middle"]);
  });

  test("the order is the rows' own, and a fresh array every time", () => {
    const list = rows([
      ["a", { backAt: 2 }],
      ["b", { backAt: 1 }],
    ]);
    const sorted = sortSection("threads", list, (r) => r.at);
    expect(sorted).toEqual([list[0]!, list[1]!]);
    expect(sorted).not.toBe(list);
  });
});

describe("where a row puts what it has besides its title", () => {
  const meta = (over: Partial<Parameters<typeof rowMeta>[0]>) => rowMeta({ layout: "compact", ...over });
  const diff = { added: 1, removed: 0 };

  test("a branch, a pull request or a stack is worth a second line, whatever the layout", () => {
    for (const layout of ["compact", "comfortable"] as const) {
      expect(meta({ layout, branch: true })).toBe("line");
      expect(meta({ layout, pull: true })).toBe("line");
      expect(meta({ layout, stack: true })).toBe("line");
    }
  });

  test("a diff alone is never a line of its own, whatever the layout", () => {
    expect(meta({ diff })).toBe("trailing");
    expect(meta({ diff, branch: true })).toBe("line");
    // The card puts it at the end of its title line, so a card is two lines unless it has a branch.
    expect(meta({ diff, layout: "comfortable" })).toBe("trailing");
  });

  test("the harness mark alone rides in the trailing slot beside the state, never a line", () => {
    // One 12px glyph at the right edge of a second line reads as the state's icon wrapping.
    for (const layout of ["compact", "comfortable"] as const) {
      expect(meta({ layout, harness: true })).toBe("trailing");
    }
    expect(meta({ harness: true, diff })).toBe("trailing");
    // It earns the line when the line already holds something that does.
    expect(meta({ harness: true, branch: true })).toBe("line");
    expect(meta({ harness: true, stack: true })).toBe("line");
  });

  test("nothing to say is no line at all, so the mark is never alone on one", () => {
    expect(meta({})).toBe("none");
    expect(meta({ diff: null })).toBe("none");
    expect(meta({ harness: false, diff: null })).toBe("none");
    expect(meta({ layout: "comfortable" })).toBe("none");
  });
});
