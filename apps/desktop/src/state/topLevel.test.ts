import { describe, expect, test } from "bun:test";
import type { Link } from "./orchestration";
import { type DelegatedThread, openChain, parentOf, topLevel, userTypedPrompt } from "./topLevel";

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

/** Every thread the app knows, not the filtered list. */
const known = (...ids: string[]) => new Set(ids);

describe("a delegated thread's place", () => {
  test("a known parent keeps it nested, with no row of its own", () => {
    expect(topLevel(child(), [], known("parent", "child"))).toBe("nested");
    // The start link says the same, and is preferred while it exists.
    expect(
      topLevel(child(), [link({ orchestrator: "parent", target: "child", started: true })], known("parent")),
    ).toBe("nested");
  });

  test("a settled parent is still known, so the child goes with it, with no Settled row", () => {
    // Settled rows are drawn; `known` is every session, whatever section it lands in.
    expect(topLevel(child(), [], known("parent", "child"))).toBe("nested");
    // Nested means no row: only the threads that are not nested are drawn.
    const threads = [{ id: "parent" }, child()];
    const drawn = threads.filter((t) => topLevel(t, [], known("parent", "child")) !== "nested");
    expect(drawn.map((t) => t.id)).toEqual(["parent"]);
  });

  test("a project filter cannot un-nest it: the parent is not in the drawn list but exists", () => {
    // Only `known` decides, so the child stays nested while its parent is filtered out, and
    // opening it draws it under the parent rather than as a top-level row.
    expect(topLevel(child(), [], known("parent", "child"))).toBe("nested");
    expect(topLevel(child(), [], known("parent"))).toBe("nested");
  });

  test("a search cannot un-nest it either", () => {
    // The search row is drawn by the sidebar with its hint; the tree itself is unchanged.
    expect(topLevel(child(), [], known("parent", "child"))).toBe("nested");
  });

  test("the order the list arrives in does not matter: the child is never a row of its own", () => {
    const threads = [{ id: "parent" }, { id: "child", parent: { id: "parent" } }];
    const ids = known(...threads.map((t) => t.id));
    // Child before parent, as a refresh that lists the newer thread first.
    const childFirst = [threads[1]!, threads[0]!].filter((t) => topLevel(t, [], ids) !== "nested");
    // Parent before child.
    const parentFirst = [...threads].filter((t) => topLevel(t, [], ids) !== "nested");
    expect(childFirst.map((t) => t.id)).toEqual(["parent"]);
    expect(parentFirst.map((t) => t.id)).toEqual(["parent"]);
  });

  test("a child whose parent is gone from the app entirely keeps a row of its own", () => {
    // Deleted, not merely filtered: with no parent row anywhere, hiding it would take it out of
    // the app altogether.
    expect(topLevel(child(), [], known())).toBe("shown");
    expect(topLevel(child(), [], known("other"))).toBe("shown");
  });

  test("a thread with no parent at all stays shown", () => {
    expect(topLevel({ id: "plain" }, [], known())).toBe("shown");
    expect(topLevel({ id: "plain" }, [link({ orchestrator: "x", target: "plain" })], known())).toBe("shown");
  });

  test("a pinned child is the user's thread and stays where they put it", () => {
    expect(topLevel(child({ pinned: true }), [], known("parent"))).toBe("shown");
    expect(topLevel(child({ pinned: true }), [], known())).toBe("shown");
  });

  test("a child the user messaged is theirs too, in the list as a row of its own", () => {
    expect(topLevel(child({ messaged: true }), [], known("parent"))).toBe("shown");
    expect(topLevel(child({ messaged: true }), [], known())).toBe("shown");
  });

  test("a link pruned from the book does not lose the parent the metadata recorded", () => {
    const pruned = [link({ orchestrator: "parent", target: "child", state: "done" })];
    // `message_session` follow-ups carry no `started`, so they prove nothing;
    // the recorded parent is what nests it.
    expect(parentOf(child(), pruned)).toBe("parent");
    expect(topLevel(child(), pruned, known("parent"))).toBe("nested");
    expect(topLevel(child(), [], known("parent"))).toBe("nested");
  });

  test("the newest start link names the parent, not the oldest", () => {
    const links = [
      link({ orchestrator: "old", target: "child", started: true, createdAt: 5 }),
      link({ orchestrator: "new", target: "child", started: true, createdAt: 9 }),
      // A newer follow-up from another thread is not a start, and never becomes the parent.
      link({ orchestrator: "other", target: "child", createdAt: 20 }),
    ];
    expect(parentOf(child({ parent: undefined }), links)).toBe("new");
    expect(topLevel(child({ parent: undefined }), links, known("old", "new"))).toBe("nested");
  });

  test("an old thread knows only its parent's title: it is delegated, and keeps its row", () => {
    const old: DelegatedThread = { id: "child", parent: { title: "my orchestrator app" } };
    expect(parentOf(old, [])).toBeUndefined();
    // No id to nest it under: with no parent row to sit under, it keeps one of its own.
    expect(topLevel(old, [], known())).toBe("shown");
    // The moment no record exists at all it is a plain thread again.
    expect(topLevel({ id: "plain" }, [], known())).toBe("shown");
  });
});

describe("the open delegated thread's chain", () => {
  const nested = new Map([
    ["child", "parent"],
    ["grandchild", "child"],
  ]);
  const drawn = known("parent");

  test("the open child hangs under its parent's row", () => {
    expect(openChain("child", nested, drawn)).toEqual([{ id: "child", parent: "parent", depth: 1 }]);
  });

  test("a grandchild brings the threads between it and the row of its own, deepest first", () => {
    expect(openChain("grandchild", nested, drawn)).toEqual([
      { id: "grandchild", parent: "child", depth: 2 },
      { id: "child", parent: "parent", depth: 1 },
    ]);
  });

  test("a chosen row, or a not-drawn parent, has no chain", () => {
    // A top-level thread is already a row of its own.
    expect(openChain("parent", nested, drawn)).toEqual([]);
    expect(openChain(null, nested, drawn)).toEqual([]);
    // The list filtered the parent out: the caller draws the open thread as a row of its own.
    expect(openChain("child", nested, known())).toEqual([]);
  });

  test("a broken record cannot make a loop of sub-rows", () => {
    const loop = new Map([
      ["a", "b"],
      ["b", "a"],
    ]);
    expect(openChain("a", loop, known("a", "b"))).toEqual([]);
  });
});

describe("what makes a delegated thread the user's own", () => {
  test("a message a person typed is theirs", () => {
    expect(userTypedPrompt({})).toBe(true);
    expect(userTypedPrompt({ limitResume: false })).toBe(true);
  });

  test("a limit resume, a wake, a link or any automatic turn is not", () => {
    expect(userTypedPrompt({ automatic: true, limitResume: true })).toBe(false);
    expect(userTypedPrompt({ limitResume: true })).toBe(false);
    expect(userTypedPrompt({ wake: true })).toBe(false);
    expect(userTypedPrompt({ link: "parent/1" })).toBe(false);
    expect(userTypedPrompt({ automatic: true })).toBe(false);
  });
});
