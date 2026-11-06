import { describe, expect, test } from "bun:test";
import type { Link } from "./orchestration";
import { type DelegatedThread, parentOf, topLevel } from "./topLevel";

const link = (over: Partial<Link> & Pick<Link, "orchestrator" | "target">): Link => ({
  id: `${over.orchestrator}/1`,
  orchestratorTitle: "Parent",
  targetTitle: "Child",
  message: "do the work",
  state: "done",
  createdAt: 1,
  ...over,
});

const child = (over: Partial<DelegatedThread> = {}): DelegatedThread => ({
  id: "child",
  parent: { id: "parent", title: "Parent" },
  ...over,
});

const listed = (...ids: string[]) => new Set(ids);

describe("a delegated thread's place", () => {
  test("a listed parent keeps it nested, with no row of its own", () => {
    expect(topLevel(child(), [], listed("parent"))).toBe("nested");
    // The start link says the same, and is preferred while it exists.
    expect(
      topLevel(child(), [link({ orchestrator: "parent", target: "child", started: true })], listed("parent")),
    ).toBe("nested");
  });

  test("a settled parent is still listed, so the child goes with it, with no Settled row", () => {
    // Settled rows are drawn; `listed` is the filtered list, not only Threads.
    expect(topLevel(child(), [], listed("parent", "child"))).toBe("nested");
    // Nested means no row: only the threads that are not nested are drawn.
    const threads = [{ id: "parent" }, child()];
    const drawn = threads.filter((t) => topLevel(t, [], listed("parent", "child")) !== "nested");
    expect(drawn.map((t) => t.id)).toEqual(["parent"]);
  });

  test("the order the list arrives in does not matter: the child is never a row of its own", () => {
    const threads = [{ id: "parent" }, { id: "child", parent: { id: "parent" } }];
    const ids = new Set(threads.map((t) => t.id));
    // Child before parent, as a refresh that lists the newer thread first.
    const childFirst = [threads[1]!, threads[0]!].filter((t) => topLevel(t, [], ids) !== "nested");
    // Parent before child.
    const parentFirst = [...threads].filter((t) => topLevel(t, [], ids) !== "nested");
    expect(childFirst.map((t) => t.id)).toEqual(["parent"]);
    expect(parentFirst.map((t) => t.id)).toEqual(["parent"]);
  });

  test("a child whose parent is not in the list keeps a row of its own, not a hidden one", () => {
    // The parent is gone, deleted, filtered out or in another project: the child's row is the
    // only place it can be seen, so it has one (Settled is a shut shelf by default).
    expect(topLevel(child(), [], listed())).toBe("shown");
    expect(topLevel(child(), [], listed("other"))).toBe("shown");
    // The first frame of a reopen, before any parent is listed: the same, not a row that goes.
    expect(topLevel(child({ live: true }), [], listed())).toBe("shown");
    expect(topLevel(child({ live: true }), [], listed("other"))).toBe("shown");
  });

  test("a thread with no parent at all stays shown", () => {
    expect(topLevel({ id: "plain" }, [], listed())).toBe("shown");
    expect(topLevel({ id: "plain" }, [link({ orchestrator: "x", target: "plain" })], listed())).toBe("shown");
  });

  test("a pinned child is the user's thread and stays where they put it", () => {
    expect(topLevel(child({ pinned: true }), [], listed("parent"))).toBe("shown");
    expect(topLevel(child({ pinned: true }), [], listed())).toBe("shown");
  });

  test("a child the user messaged is theirs too, in the list as a row of its own", () => {
    expect(topLevel(child({ messaged: true }), [], listed("parent"))).toBe("shown");
    expect(topLevel(child({ messaged: true }), [], listed())).toBe("shown");
  });

  test("a link pruned from the book does not lose the parent the metadata recorded", () => {
    const pruned = [link({ orchestrator: "parent", target: "child", state: "done" })];
    // `message_session` follow-ups carry no `started`, so they prove nothing;
    // the recorded parent is what nests it.
    expect(parentOf(child(), pruned)).toBe("parent");
    expect(topLevel(child(), pruned, listed("parent"))).toBe("nested");
    expect(topLevel(child(), [], listed("parent"))).toBe("nested");
  });

  test("an old thread knows only its parent's title: it is delegated, and keeps its row", () => {
    const old: DelegatedThread = { id: "child", parent: { title: "my orchestrator app" } };
    expect(parentOf(old, [])).toBeUndefined();
    // No id to nest it under: with no parent row to sit under, it keeps one of its own.
    expect(topLevel(old, [], listed())).toBe("shown");
    expect(topLevel({ ...old, live: true }, [], listed())).toBe("shown");
    // The moment no record exists at all it is a plain thread again.
    expect(topLevel({ id: "plain" }, [], listed())).toBe("shown");
  });
});
