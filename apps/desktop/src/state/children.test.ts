import { describe, expect, test } from "bun:test";
import {
  type ChildInfo,
  childInfo,
  childrenListed,
  childState,
  childSummary,
  childVisible,
  FINISHED_FOLD_AT,
  flatChildren,
  foldFinished,
  GRANDCHILD_INSET,
  orderChildren,
  pruneOpenGroups,
  unseenChild,
} from "./children";

const child = (id: string, state: ChildInfo["state"], unseen = false): ChildInfo => ({ id, state, unseen });

describe("a child's state", () => {
  test("a thread waiting on you comes before one that works, and the rest are finished", () => {
    expect(childState("needs_input")).toBe("needs");
    expect(childState("running")).toBe("working");
    expect(childState("idle")).toBe("finished");
    expect(childState("review")).toBe("finished");
    expect(childState("done")).toBe("finished");
    expect(childState("idle", true)).toBe("failed");
  });
});

describe("a child read as its group sees it", () => {
  test("a live link beats a status that has not caught up", () => {
    expect(childInfo("a", { status: "idle", link: "working" }).state).toBe("working");
    expect(childInfo("a", { status: "idle", link: "needs_user" }).state).toBe("needs");
    expect(childInfo("a", { status: "needs_input", link: "done" }).state).toBe("needs");
  });

  test("a finished link leaves the thread's own status, and a failure marks it", () => {
    expect(childInfo("a", { status: "running", link: "done" }).state).toBe("working");
    expect(childInfo("a", { status: "idle", link: "cancelled", failed: true }).state).toBe("failed");
    expect(childInfo("a", {}).state).toBe("finished");
  });

  test("it carries whether the reader has seen the change", () => {
    expect(childInfo("a", { status: "idle", unseen: true }).unseen).toBe(true);
    expect(childInfo("a", { status: "idle" }).unseen).toBeUndefined();
  });
});

describe("the group's one line", () => {
  test("counts the threads, the working ones and the ones that want you", () => {
    const five = [
      child("a", "working"),
      child("b", "working"),
      child("c", "needs"),
      child("d", "finished"),
      child("e", "finished"),
    ];
    expect(childSummary(five)).toBe("5 threads · 2 working · 1 needs you");
  });

  test("leaves out the parts that are zero, and gets one right", () => {
    expect(childSummary([child("a", "working")])).toBe("1 thread · 1 working");
    expect(childSummary([child("a", "needs")])).toBe("1 thread · 1 needs you");
    expect(childSummary([child("a", "finished"), child("b", "failed")])).toBe("2 threads");
    expect(childSummary([child("a", "needs"), child("b", "needs")])).toBe("2 threads · 2 need you");
    expect(childSummary([])).toBe("0 threads");
  });
});

describe("what stays a row", () => {
  const children = [
    child("done", "finished"),
    child("work", "working"),
    child("needs", "needs"),
    child("open", "finished"),
  ];

  test("what works, what wants you and what you opened", () => {
    expect(children.filter((c) => childVisible(c, "open")).map((c) => c.id)).toEqual([
      "work",
      "needs",
      "open",
    ]);
    expect(children.filter((c) => childVisible(c, null)).map((c) => c.id)).toEqual(["work", "needs"]);
  });

  test("they are ordered what wants you, what works, what you opened, then the rest", () => {
    expect(orderChildren(children, "open").map((c) => c.id)).toEqual(["needs", "work", "open", "done"]);
    // Ties keep the order they came in.
    expect(orderChildren([child("x", "finished"), child("y", "finished")], null).map((c) => c.id)).toEqual([
      "x",
      "y",
    ]);
  });
});

describe("when the group is a line", () => {
  const quiet = [child("a", "finished"), child("b", "working")];

  test("a parent you are not reading folds, unless one of its children wants you", () => {
    expect(childrenListed(quiet, { expanded: false, parentSelected: false, selectedId: null })).toBe(false);
    expect(
      childrenListed([...quiet, child("c", "needs")], {
        expanded: false,
        parentSelected: false,
        selectedId: null,
      }),
    ).toBe(true);
  });

  test("opening the group or the parent lists them", () => {
    expect(childrenListed(quiet, { expanded: true, parentSelected: false, selectedId: null })).toBe(true);
    expect(childrenListed(quiet, { expanded: false, parentSelected: true, selectedId: null })).toBe(true);
    expect(childrenListed(quiet, { expanded: false, parentSelected: false, selectedId: "a" })).toBe(true);
  });

  test("closing the group by hand overrides the parent, but never a child that wants you", () => {
    const closed = { expanded: false, collapsed: true, parentSelected: true, selectedId: "a" } as const;
    expect(childrenListed(quiet, closed)).toBe(false);
    expect(childrenListed([...quiet, child("c", "needs")], closed)).toBe(true);
    // Opening it again wins over an earlier close.
    expect(childrenListed(quiet, { ...closed, expanded: true })).toBe(true);
  });
});

describe("the groups the reader opened", () => {
  test("writes leave out the parents that are no longer listed", () => {
    const groups = { a: true, gone: false, b: true };
    expect(pruneOpenGroups(groups, ["a", "b", "c"])).toEqual({ a: true, b: true });
    expect(pruneOpenGroups(groups, [])).toEqual({});
    // A parent that is still listed keeps the state it was given.
    expect(pruneOpenGroups({ a: false }, ["a"])).toEqual({ a: false });
  });
});

describe("the finished ones behind the group's last line", () => {
  test("a few finished children with nothing live beside them simply show", () => {
    const two = [child("a", "finished"), child("b", "finished")];
    expect(foldFinished(two, null)).toEqual({ rows: two, folded: [], fold: false });
    expect(foldFinished([child("a", "failed"), child("b", "finished")], null).fold).toBe(false);
  });

  test("finished children beside a live one fold, however few they are", () => {
    const mixed = [child("needs", "needs"), child("done", "finished")];
    expect(foldFinished(mixed, null)).toEqual({
      rows: [mixed[0]!],
      folded: [mixed[1]!],
      fold: true,
    });
  });

  test("more than three finished ones fold on their own", () => {
    const many = [
      child("a", "finished"),
      child("b", "finished"),
      child("c", "finished"),
      child("d", "finished"),
    ];
    expect(many.length).toBeGreaterThan(FINISHED_FOLD_AT);
    expect(foldFinished(many, null).fold).toBe(true);
    // Exactly three still show: the line costs more than it saves.
    expect(foldFinished(many.slice(0, FINISHED_FOLD_AT), null).fold).toBe(false);
  });

  test("a group closed by hand still lists what cannot be closed away", () => {
    const children = [child("needs", "needs"), child("work", "working"), child("done", "finished")];
    const closed = foldFinished(children, null, true);
    expect(closed.rows.map((c) => c.id)).toEqual(["needs", "work"]);
    expect(closed.folded.map((c) => c.id)).toEqual(["done"]);
    // The one they opened stays too, so closing the group never hides the thread in the pane.
    expect(foldFinished([child("open", "finished")], "open", true).rows.map((c) => c.id)).toEqual(["open"]);
  });

  test("the thread you opened stays a row, and keeps its place in the fold", () => {
    const children = [child("work", "working"), child("open", "finished"), child("shut", "finished")];
    const { rows, folded } = foldFinished(children, "open");
    expect(rows.map((c) => c.id)).toEqual(["work", "open"]);
    expect(folded.map((c) => c.id)).toEqual(["shut"]);
  });
});

describe("the group's dot", () => {
  test("says a child finished unseen, and failure reads stronger", () => {
    expect(unseenChild([child("a", "finished")])).toBeUndefined();
    expect(unseenChild([child("a", "finished", true)])).toBe("finished");
    expect(unseenChild([child("a", "finished", true), child("b", "failed", true)])).toBe("failed");
  });
});

describe("a grandchild's row", () => {
  test("is shown flat under the top-level group, one level in", () => {
    const b = child("b", "working");
    const c = child("c", "needs");
    const grandkids: Record<string, ChildInfo[]> = { b: [c] };
    const flat = flatChildren([b], (id) => grandkids[id]);
    // It is a row of the same group, with one extra level of inset and no second chevron.
    expect(flat).toEqual([
      { ...b, level: 0 },
      { ...c, level: 1 },
    ]);
    expect(GRANDCHILD_INSET).toBe("pl-10");
    // A child with none of its own adds nothing.
    expect(flatChildren([c], () => undefined)).toEqual([{ ...c, level: 0 }]);
  });

  test("keeps the group's order and folding, so nothing is lost", () => {
    const children = [child("b", "finished"), child("c", "working")];
    const grandkids: Record<string, ChildInfo[]> = { b: [child("d", "working")] };
    const flat = flatChildren(children, (id) => grandkids[id]);
    // What wants you first, then what works; the grandchild keeps its level.
    expect(orderChildren(flat, null).map((x) => [x.id, x.level])).toEqual([
      ["d", 1],
      ["c", 0],
      ["b", 0],
    ]);
    const { rows, folded, fold } = foldFinished(orderChildren(flat, null), null);
    expect(rows.map((x) => x.id)).toEqual(["d", "c"]);
    expect(folded.map((x) => x.id)).toEqual(["b"]);
    expect(fold).toBe(true);
    // The group's one line counts what it draws, grandchildren included.
    expect(childSummary(flat)).toBe("3 threads · 2 working");
  });

  test("a finished child stays a row while a grandchild of its is still live", () => {
    const finished: ChildInfo = { ...child("b", "finished"), below: "working" };
    // Otherwise its working grandchild would be folded away with it.
    expect(childVisible(finished, null)).toBe(true);
    const { rows, folded } = foldFinished([finished], null);
    expect(rows.map((x) => x.id)).toEqual(["b"]);
    expect(folded).toEqual([]);
    // A finished child with nothing live under it folds as it did before.
    const quiet: ChildInfo = { ...child("d", "finished"), below: undefined };
    expect(childVisible(quiet, null)).toBe(false);
  });
});
