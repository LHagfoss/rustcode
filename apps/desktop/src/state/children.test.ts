import { describe, expect, test } from "bun:test";
import {
  type ChildInfo,
  childInfo,
  childrenListed,
  childState,
  childSummary,
  childVisible,
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

describe("the group's dot", () => {
  test("says a child finished unseen, and failure reads stronger", () => {
    expect(unseenChild([child("a", "finished")])).toBeUndefined();
    expect(unseenChild([child("a", "finished", true)])).toBe("finished");
    expect(unseenChild([child("a", "finished", true), child("b", "failed", true)])).toBe("failed");
  });
});
