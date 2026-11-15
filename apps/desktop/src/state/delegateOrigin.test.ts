import { describe, expect, test } from "bun:test";
import type { SessionSummary } from "../acp/types";
import { delegateOriginOf, detachedDelegates, originText, projectKeyOf, sameProject } from "./delegateOrigin";
import type { Link } from "./orchestration";
import { openChain } from "./topLevel";

const session = (id: string, over: Partial<SessionSummary> = {}): SessionSummary => ({
  id,
  cwd: "/Users/me/code/rustcode",
  title: id,
  ...over,
});

const started = (orchestrator: string, target: string, over: Partial<Link> = {}): Link => ({
  id: `${orchestrator}/${target}`,
  orchestrator,
  orchestratorTitle: "Parent (link)",
  target,
  targetTitle: "Child",
  message: "do the work",
  state: "done",
  createdAt: 1,
  started: true,
  ...over,
});

// The reported case: an orchestrator in `~/code` started delegates in `~/code/rustcode`.
const parent = session("parent", { cwd: "/Users/me/code", title: "Remote images, machines rename, menus" });
const delegate = session("delegate", {
  title: "Remote thread images",
  parent: { id: "parent", title: "Remote images, machines rename, menus", relationship: "delegate" },
});
const inWorktree = session("worktree-delegate", {
  cwd: "/Users/me/.config/rustcode/worktrees/rustcode/feat-x",
  worktree: {
    path: "/Users/me/.config/rustcode/worktrees/rustcode/feat-x",
    branch: "feat/x",
    repo: "/Users/me/code/rustcode",
  },
  parent: { id: "parent", relationship: "delegate" },
});
const grandchild = session("grandchild", { parent: { id: "delegate", relationship: "delegate" } });
const sibling = session("sibling", {
  cwd: "/Users/me/code",
  parent: { id: "parent", relationship: "delegate" },
});
const all = [parent, delegate, inWorktree, grandchild, sibling];
const parentOf = new Map([
  ["delegate", "parent"],
  ["worktree-delegate", "parent"],
  ["grandchild", "delegate"],
  ["sibling", "parent"],
]);

/** The ids the project filter keeps for a folder, as ui/Sidebar.tsx filters. */
const inProject = (folder: string) =>
  new Set(all.filter((s) => s.cwd === folder || s.worktree?.repo === folder).map((s) => s.id));

describe("delegates started from another project", () => {
  test("a worktree thread belongs to its repository's project, on its own computer", () => {
    expect(sameProject(delegate, inWorktree)).toBe(true);
    expect(sameProject(delegate, parent)).toBe(false);
    expect(sameProject(delegate, { ...delegate, host: "studio" })).toBe(false);
    expect(projectKeyOf(inWorktree)).toBe(projectKeyOf(delegate));
  });

  test("filtered to the delegate's project, it keeps a row of its own; its own delegates nest under it", () => {
    const drawn = inProject("/Users/me/code/rustcode");
    expect([...detachedDelegates(drawn, parentOf)].sort()).toEqual(["delegate", "worktree-delegate"]);
  });

  test("filtered to the parent's project, nothing is detached: the delegates nest under the parent", () => {
    expect(detachedDelegates(inProject("/Users/me/code"), parentOf).size).toBe(0);
  });

  test("with no filter every parent is drawn, so no thread gets a second row in the same list", () => {
    expect(detachedDelegates(new Set(all.map((s) => s.id)), parentOf).size).toBe(0);
  });

  test("the open grandchild hangs under its detached parent's row", () => {
    const drawn = inProject("/Users/me/code/rustcode");
    const detached = detachedDelegates(drawn, parentOf);
    const listParentOf = new Map([...parentOf].filter(([id]) => !detached.has(id)));
    const topRows = new Set([...drawn].filter((id) => !listParentOf.has(id)));
    expect(topRows.has("delegate")).toBe(true);
    expect(openChain("grandchild", listParentOf, topRows)).toEqual([
      { id: "grandchild", parent: "delegate", depth: 1 },
    ]);
  });
});

describe("where a delegate came from", () => {
  test("a delegate in another project names the parent and the parent's project", () => {
    const origin = delegateOriginOf(all, "delegate", []);
    expect(origin).toEqual({
      id: "parent",
      title: "Remote images, machines rename, menus",
      project: "code",
      listed: true,
    });
    expect(originText(origin!)).toBe("from Remote images, machines rename, menus · code");
  });

  test("a delegate in its parent's project names only the parent", () => {
    expect(delegateOriginOf(all, "grandchild", [])).toEqual({
      id: "delegate",
      title: "Remote thread images",
      listed: true,
    });
    expect(delegateOriginOf(all, "sibling", [])?.project).toBeUndefined();
  });

  test("the parent's title is what it is called now", () => {
    const renamed = all.map((s) => (s.id === "parent" ? { ...s, title: "Renamed parent" } : s));
    expect(delegateOriginOf(renamed, "delegate", [])?.title).toBe("Renamed parent");
  });

  test("a start link names the parent when the metadata does not", () => {
    const linked = session("linked");
    expect(delegateOriginOf([parent, linked], "linked", [started("parent", "linked")])).toEqual({
      id: "parent",
      title: "Remote images, machines rename, menus",
      project: "code",
      listed: true,
    });
  });

  test("a parent that is gone keeps the name it had, and cannot be opened", () => {
    const orphan = session("orphan", { parent: { id: "gone", title: "Old orchestrator" } });
    expect(delegateOriginOf([orphan], "orphan", [])).toEqual({
      id: "gone",
      title: "Old orchestrator",
      listed: false,
    });
  });

  test("a parent on another computer says which one", () => {
    const remoteParent = { ...parent, id: "studio/parent", host: "studio" };
    const child = session("child", { parent: { id: "studio/parent", relationship: "delegate" } });
    expect(delegateOriginOf([remoteParent, child], "child", [])?.host).toBe("studio");
  });

  test("forks, side chats, unknown relationships and threads no agent started have no delegate origin", () => {
    const fork = session("fork", { parent: { id: "parent", relationship: "fork" } });
    const side = session("side", { parent: { id: "parent", relationship: "side_chat" } });
    const other = session("other", { parent: { id: "parent", relationship: "other" } });
    const list = [parent, fork, side, other];
    expect(delegateOriginOf(list, "fork", [])).toBeUndefined();
    expect(delegateOriginOf(list, "side", [])).toBeUndefined();
    expect(delegateOriginOf(list, "other", [])).toBeUndefined();
    expect(delegateOriginOf(list, "parent", [])).toBeUndefined();
    expect(delegateOriginOf(list, "missing", [])).toBeUndefined();
  });

  test("a thread that names itself as its parent has no origin", () => {
    const self = session("self", { parent: { id: "self", relationship: "delegate" } });
    expect(delegateOriginOf([self], "self", [])).toBeUndefined();
  });
});
