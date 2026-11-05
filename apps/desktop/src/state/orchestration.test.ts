import { describe, expect, test } from "bun:test";
import {
  checkpointsByTurn,
  parseTurnDiff,
  parseWorktree,
  removeRefusal,
  suggestBranch,
  validBranch,
} from "../acp/v2";
import {
  activeDescendants,
  activeInTree,
  activityOf,
  capped,
  checkManage,
  checkMessage,
  checkQueryData,
  checkStart,
  depthOf,
  emptyBook,
  failureJson,
  fillAnswers,
  forget,
  interruptActive,
  isActive,
  isFailure,
  type Link,
  type LinkBook,
  latestReply,
  linkCounts,
  linkGroupStatus,
  linkResult,
  linksLeft,
  listSessions,
  MAX_ACTIVE_LINKS,
  MAX_DATA_ROWS,
  MAX_DEPTH,
  MAX_TREE_LINKS,
  markWoken,
  mergeBooks,
  nextWakeAt,
  orphanedLinks,
  pageOf,
  parseBook,
  parseCall,
  pendingWakeUnder,
  quietTree,
  type RuleThread,
  renderRead,
  restateLinks,
  sameProject,
  stripLinks,
  supersededLinks,
  updateLink,
  usageScope,
  WAKE_ANSWER_CHARS,
  WAKE_COALESCE_MS,
  type WakeContext,
  wakeMessage,
  wakesDue,
} from "./orchestration";
import { isWakeMessage } from "./thread";

const thread = (id: string, folder = "/repo", extra: Partial<RuleThread> = {}): RuleThread => ({
  id,
  title: `Thread ${id}`,
  folder,
  running: false,
  ...extra,
});

const link = (orchestrator: string, target: string, state: Link["state"] = "working"): Link => ({
  id: `${orchestrator}/${target}`,
  orchestrator,
  target,
  orchestratorTitle: orchestrator,
  targetTitle: target,
  message: "hi",
  state,
  createdAt: Date.now(),
});

const book = (links: Link[]): LinkBook => ({ ...emptyBook(), links });

describe("management tools", () => {
  test("parseCall reads them and refuses what cannot run", () => {
    expect(parseCall({ tool: "list_harnesses", args: {} })).toEqual({ tool: "list_harnesses" });
    expect(parseCall({ tool: "search_sessions", args: { query: " login " } })).toEqual({
      tool: "search_sessions",
      query: "login",
      allProjects: false,
    });
    expect(isFailure(parseCall({ tool: "search_sessions", args: {} }))).toBe(true);
    expect(isFailure(parseCall({ tool: "interrupt_session", args: {} }))).toBe(true);
    expect(isFailure(parseCall({ tool: "update_session", args: { session_id: "a" } }))).toBe(true);
    expect(isFailure(parseCall({ tool: "update_session", args: { status: "gone" } }))).toBe(true);
    expect(parseCall({ tool: "list_projects", args: {} })).toEqual({ tool: "list_projects" });
    expect(parseCall({ tool: "list_pull_requests", args: {} })).toEqual({
      tool: "list_pull_requests",
      state: "open",
    });
    expect(parseCall({ tool: "list_pull_requests", args: { state: "merged" } })).toEqual({
      tool: "list_pull_requests",
      state: "merged",
    });
    expect(isFailure(parseCall({ tool: "list_pull_requests", args: { state: "draft" } }))).toBe(true);
    expect(parseCall({ tool: "update_session", args: { pinned: false, status: "settled" } })).toMatchObject({
      pinned: false,
      status: "settled",
    });
    expect(parseCall({ tool: "configure_session", args: { effort: "high" } })).toMatchObject({
      effort: "high",
    });
    expect(parseCall({ tool: "transfer_session", args: { session_id: "a", harness: "codex" } })).toEqual({
      tool: "transfer_session",
      target: "a",
      harness: "codex",
    });
    expect(isFailure(parseCall({ tool: "transfer_session", args: { session_id: "a" } }))).toBe(true);
    expect(parseCall({ tool: "list_pending_requests", args: {} })).toEqual({ tool: "list_pending_requests" });
  });

  test("checkManage keeps a thread to its own project", () => {
    const a = { id: "a", title: "A", folder: "/repo", running: false };
    expect(checkManage(a, undefined)?.code).toBe("not_found");
    expect(checkManage(a, { ...a, id: "b", folder: "/other" })?.code).toBe("not_allowed");
    expect(checkManage(a, { ...a, id: "b" })).toBeNull();
  });
});

describe("orchestration rules", () => {
  test("parses the v2 calls and refuses bad arguments", () => {
    expect(parseCall({ tool: "read_session", args: { session_id: "b", last_turns: 2 } })).toEqual({
      tool: "read_session",
      target: "b",
      lastTurns: 2,
      offset: 0,
    });
    expect(parseCall({ tool: "read_session", args: { session_id: "b", offset: 16384 } })).toMatchObject({
      offset: 16384,
    });
    expect(isFailure(parseCall({ tool: "read_session", args: { session_id: "b", offset: -1 } }))).toBe(true);
    expect(isFailure(parseCall({ tool: "read_session", args: { session_id: "b", last_turns: 11 } }))).toBe(
      true,
    );
    const fan = parseCall({
      tool: "fan_out",
      args: { prompt: "do it", harnesses: [{ harness: "rustcode" }, { harness: "codex", model: "m" }] },
    });
    expect(!isFailure(fan) && fan.tool === "fan_out" && fan.harnesses.length).toBe(2);
    expect(
      isFailure(parseCall({ tool: "fan_out", args: { prompt: "x", harnesses: [{ harness: "a" }] } })),
    ).toBe(true);
    expect(isFailure(parseCall({ tool: "start_session", args: { prompt: "  " } }))).toBe(true);
    expect(
      isFailure(
        parseCall({ tool: "message_session", args: { session_id: "b", message: "x".repeat(33_000) } }),
      ),
    ).toBe(true);
    const wait = parseCall({ tool: "wait_session", args: { link_id: "a/1", timeout_ms: 9e9 } });
    expect(!isFailure(wait) && wait.tool === "wait_session" && wait.timeoutMs).toBe(3_600_000);
    // Each answer once: a repeated link id is asked about once.
    expect(parseCall({ tool: "wait_session", args: { link_ids: ["a/1", "a/1", "a/2"] } })).toMatchObject({
      links: ["a/1", "a/2"],
    });
  });

  test("same project, the thread limit, one orchestrator per target", () => {
    const a = thread("a");
    expect(checkMessage(a, thread("b", "/other"), emptyBook())?.code).toBe("not_allowed");
    expect(checkMessage(a, a, emptyBook())?.code).toBe("not_allowed");
    expect(checkMessage(a, undefined, emptyBook())?.code).toBe("not_found");
    // A worktree thread belongs to its repository's project.
    const wt = thread("w", "/wt/x", { worktree: { path: "/wt/x", branch: "b", repo: "/repo" } });
    expect(checkMessage(a, wt, emptyBook())).toBeNull();
    expect(checkMessage(a, thread("b"), book([link("z", "b")]))?.code).toBe("target_linked");
    const full = ["c", "d", "e", "f"].map((t) => link("a", t));
    expect(checkMessage(a, thread("b"), book(full), false, false, 4)?.code).toBe("busy");
    expect(checkStart(a, book(full.slice(0, 2)), 3, 4)?.code).toBe("busy");
    expect(checkStart(a, book(full.slice(0, 2)), 2, 4)).toBeNull();
    // The default is eight now, and the setting's own ends are two and sixteen.
    expect(checkStart(a, book(full.slice(0, 2)), 2)).toBeNull();
    const eight = ["c", "d", "e", "f", "g", "h", "i", "j"].map((t) => link("a", t));
    expect(checkStart(a, book(eight.slice(0, 7)), 1)).toBeNull();
    expect(checkStart(a, book(eight), 1)?.code).toBe("busy");
    expect(checkStart(a, book(eight.slice(0, 2)), 7, 2)?.code).toBe("busy");
    expect(checkStart(a, book(eight.slice(0, 1)), 1, 2)).toBeNull();
    expect(checkStart(a, book(eight), 1, 16)).toBeNull();
  });

  test("a follow-up to a thread the caller already runs takes no slot", () => {
    // The live shape: a thread at the limit messaging a thread it is already running.
    const targets = Array.from({ length: MAX_ACTIVE_LINKS }, (_, i) => `t${i}`);
    const full = book(targets.map((t) => link("a", t)));
    expect(checkStart(thread("a"), full, 1)?.code).toBe("busy");
    expect(checkMessage(thread("a"), thread(targets[0]!), full)).toBeNull();
    // A second link to the same thread is still tracked, and a wait answers each once.
    const again = book([...full.links, link("a", targets[0]!)]);
    expect(checkMessage(thread("a"), thread(targets[0]!), again)).toBeNull();
    expect(again.links.filter((l) => l.target === targets[0])).toHaveLength(2);
    // Slots count distinct threads, not links: two links to one thread take one.
    const pair = book([link("a", "b"), link("a", "b"), link("a", "c")]);
    expect(linksLeft(pair, "a", 4)).toBe(2);
    expect(linksLeft(full, "a")).toBe(0);
    // A full tree does not refuse a follow-up either.
    const tree = book([
      ...targets.map((t) => link("a", t)),
      ...Array.from({ length: MAX_ACTIVE_LINKS }, (_, i) => link(targets[0]!, `k${i}`)),
    ]);
    expect(activeInTree(tree, "a")).toBe(MAX_TREE_LINKS);
    expect(checkMessage(thread("a"), thread(targets[0]!), tree, false, false, 2)).toBeNull();
    // A thread it does not already run is still one more.
    expect(checkMessage(thread("a"), thread("new"), full)?.code).toBe("busy");
  });

  test("a child may delegate one level further, and a grandchild is told the limit", () => {
    // a (depth 0) started b (depth 1): b may still start or message someone.
    const one = book([link("a", "b")]);
    expect(depthOf(one, "a")).toBe(0);
    expect(depthOf(one, "b")).toBe(1);
    expect(checkStart(thread("b"), one, 1)).toBeNull();
    expect(checkMessage(thread("b"), thread("c"), one)).toBeNull();
    // c is at the limit: it is refused, and the reason names the limit.
    const two = book([link("a", "b"), link("b", "c")]);
    expect(depthOf(two, "c")).toBe(MAX_DEPTH);
    const refused = checkStart(thread("c"), two, 1)!;
    expect(refused.code).toBe("not_allowed");
    expect(refused.message).toContain(`delegation stops at ${MAX_DEPTH}`);
    expect(checkMessage(thread("c"), thread("d"), two)?.code).toBe("not_allowed");
    // Reading is never blocked by the depth: c can still read thread ids.
    expect(checkMessage(thread("c"), thread("d"), two, true)).toBeNull();
    // A cycle in the book is answered, not looped over.
    const loop = book([link("a", "b"), link("b", "a")]);
    expect(depthOf(loop, "a")).toBeGreaterThan(MAX_DEPTH);
    expect(checkStart(thread("a"), loop, 1)?.code).toBe("not_allowed");
  });

  test("messaging a thread that already works for the caller would close a loop", () => {
    const two = book([link("a", "b"), link("b", "c")]);
    const refused = checkMessage(thread("c"), thread("a"), two)!;
    expect(refused.code).toBe("not_allowed");
    expect(refused.message).toContain("loop");
    // The thread directly above is an ancestor too, and a sibling is not.
    expect(checkMessage(thread("b"), thread("a"), two)?.code).toBe("not_allowed");
    expect(checkMessage(thread("c"), thread("b"), two)?.code).toBe("not_allowed");
    const sibling = book([link("a", "b"), link("a", "c")]);
    expect(checkMessage(thread("b"), thread("c"), sibling)?.code).toBe("target_linked");
  });

  test("a whole tree holds twice the thread limit in active threads", () => {
    // Two levels: eight threads under a, eight under b (b is a's child).
    const under = (parent: string, n: number) =>
      Array.from({ length: n }, (_, i) => link(parent, `${parent}-${i}`));
    const tree = book([link("a", "b"), ...under("a", MAX_ACTIVE_LINKS - 1), ...under("b", MAX_ACTIVE_LINKS)]);
    expect(activeInTree(tree, "b")).toBe(MAX_TREE_LINKS);
    const refused = checkStart(thread("a-0"), tree, 1)!;
    expect(refused.code).toBe("busy");
    expect(refused.message).toContain(`tree holds at most ${MAX_TREE_LINKS}`);
    expect(refused.linksLeft).toBe(0);
    // A message to another thread opens a link too, so the tree cap holds for it as well.
    expect(checkMessage(thread("a-0"), thread("j"), tree)?.code).toBe("busy");
    // One less in the tree: the last slot is there, and a fan-out of two is not.
    const room = book([
      link("a", "b"),
      ...under("a", MAX_ACTIVE_LINKS - 1),
      ...under("b", MAX_ACTIVE_LINKS - 1),
    ]);
    expect(checkStart(thread("a-0"), room, 1)).toBeNull();
    expect(checkStart(thread("a-0"), room, 2)?.code).toBe("busy");
  });

  test("stopping a thread takes its active descendants, and nothing on a finish", () => {
    const tree = book([
      link("a", "b"),
      link("b", "c"),
      link("c", "d"),
      link("b", "e", "done"),
      link("z", "y"),
    ]);
    expect(activeDescendants(tree, "a").map((l) => l.target)).toEqual(["b", "c", "d"]);
    expect(activeDescendants(tree, "c").map((l) => l.target)).toEqual(["d"]);
    expect(activeDescendants(tree, "d")).toEqual([]);
    // A cycle would otherwise repeat forever.
    const loop = book([link("a", "b"), link("b", "a")]);
    expect(activeDescendants(loop, "a").map((l) => l.target)).toEqual(["b"]);
  });

  test("lists only the caller's project, with harness, status and worktree", () => {
    const wt = { path: "/wt/x", branch: "feat/x", repo: "/repo" };
    const out = listSessions(
      [thread("a"), thread("b", "/wt/x", { worktree: wt, harness: "codex" }), thread("c", "/elsewhere")],
      thread("a"),
      "",
    );
    const sessions = out.sessions as { session_id: string; harness: string; worktree?: unknown }[];
    expect(sessions.map((x) => x.session_id)).toEqual(["b"]);
    expect(sessions[0]?.harness).toBe("codex");
    expect(sessions[0]?.worktree).toEqual(wt);
  });

  test("all_projects lists every project, each entry naming its own", () => {
    const all = [thread("a"), thread("b", "/repo"), thread("c", "/elsewhere")];
    const out = listSessions(all, thread("a"), "", true);
    const sessions = out.sessions as { session_id: string; project?: string }[];
    expect(sessions.map((x) => x.session_id)).toEqual(["b", "c"]);
    expect(sessions.map((x) => x.project)).toEqual(["/repo", "/elsewhere"]);
    // Without it, this project's threads only, and no project on the entries.
    const scoped = listSessions(all, thread("a"), "", false);
    expect((scoped.sessions as { session_id: string }[]).map((x) => x.session_id)).toEqual(["b"]);
    expect((scoped.sessions as { project?: string }[])[0]?.project).toBeUndefined();
  });

  test("reading reaches another project only when allowed; writing never does", () => {
    const a = thread("a");
    const other = thread("b", "/other");
    expect(checkMessage(a, other, emptyBook(), true)?.code).toBe("not_allowed");
    expect(checkMessage(a, other, emptyBook(), true, true)).toBeNull();
    expect(checkMessage(a, other, emptyBook(), false, true)?.code).toBe("not_allowed");
    expect(checkMessage(a, a, emptyBook(), true, true)?.code).toBe("not_allowed");
    expect(checkMessage(a, undefined, emptyBook(), true, true)?.code).toBe("not_found");
  });

  test("parseCall reads all_projects as a flag, never a string", () => {
    expect(parseCall({ tool: "list_sessions", args: { all_projects: true } })).toEqual({
      tool: "list_sessions",
      query: "",
      allProjects: true,
    });
    expect(parseCall({ tool: "list_sessions", args: { all_projects: "yes" } })).toEqual({
      tool: "list_sessions",
      query: "",
      allProjects: false,
    });
    expect(parseCall({ tool: "search_sessions", args: { query: "x", all_projects: true } })).toMatchObject({
      allProjects: true,
    });
    expect(parseCall({ tool: "list_worktrees", args: {} })).toEqual({
      tool: "list_worktrees",
      allProjects: false,
    });
  });

  test("caps on a character boundary, interrupts after restart, forgets deleted threads", () => {
    const { text, truncated } = capped("é".repeat(10), 5);
    expect(text).toBe("éé");
    expect(truncated).toBe(true);
    const b = interruptActive(book([link("a", "b"), link("a", "c", "done")]));
    expect(b.links.map((l) => l.state)).toEqual(["interrupted", "done"]);
    const f = forget(book([link("a", "b"), link("c", "a")]), "a");
    expect(f.links.map((l) => [l.target, l.state])).toEqual([["a", "failed"]]);
  });
});

describe("v2 shapes", () => {
  test("branch suggestions and checks", () => {
    expect(suggestBranch("Fix the composer focus ring")).toBe("fix/composer-focus-ring");
    expect(suggestBranch("Add a login form")).toBe("feat/add-login-form");
    expect(suggestBranch("")).toBe("feat/thread");
    expect(validBranch("feat/login-form")).toBe(true);
    expect(validBranch("-x")).toBe(false);
    expect(validBranch("a..b")).toBe(false);
    expect(validBranch("a b")).toBe(false);
  });

  test("worktrees, refusals, turn diffs and checkpoint pairing", () => {
    expect(parseWorktree({ path: "/p", branch: "b", repo: "/r", dirty: true, ahead: 2 })?.ahead).toBe(2);
    expect(removeRefusal({ reason: "dirty" })).toBe("dirty");
    expect(removeRefusal({ reason: "x" })).toBeUndefined();
    expect(
      parseTurnDiff({ files: [{ path: "a", status: "added", insertions: 1, patch: "+x" }] })[0]?.status,
    ).toBe("added");
    const cps = [1, 2, 3].map((turn) => ({
      turn,
      ref: "",
      prompt: `prompt ${turn}`,
      filesChanged: 1,
      insertions: 1,
      deletions: 0,
    }));
    const paired = checkpointsByTurn(
      [
        { id: "t2", prompt: "prompt 2" },
        { id: "t3", prompt: "something else" },
      ],
      cps,
    );
    expect(paired.t2?.turn).toBe(2);
    expect(paired.t3).toBeUndefined();
  });
});

describe("the data store", () => {
  test("parses thread_usage and query_data, and refuses bad arguments", () => {
    expect(parseCall({ tool: "thread_usage", args: {} })).toEqual({
      tool: "thread_usage",
      since: undefined,
      groupBy: "harness",
      project: undefined,
    });
    expect(parseCall({ tool: "thread_usage", args: { since: "2026-11", group_by: "day" } })).toMatchObject({
      since: "2026-11",
      groupBy: "day",
    });
    expect(isFailure(parseCall({ tool: "thread_usage", args: { since: "last week" } }))).toBe(true);
    expect(isFailure(parseCall({ tool: "thread_usage", args: { group_by: "tool" } }))).toBe(true);
    const query = parseCall({ tool: "query_data", args: { sql: " SELECT 1 " } });
    expect(query).toEqual({ tool: "query_data", sql: "SELECT 1", limit: MAX_DATA_ROWS });
    expect(parseCall({ tool: "query_data", args: { sql: "SELECT 1", limit: 5 } })).toEqual({
      tool: "query_data",
      sql: "SELECT 1",
      limit: 5,
    });
    // A limit is clamped, never refused: the answer is bounded either way.
    expect(parseCall({ tool: "query_data", args: { sql: "SELECT 1", limit: 1e9 } })).toEqual({
      tool: "query_data",
      sql: "SELECT 1",
      limit: MAX_DATA_ROWS,
    });
    expect(isFailure(parseCall({ tool: "query_data", args: { sql: "  " } }))).toBe(true);
  });

  test("usage is scoped to the caller's project while reading across projects is off", () => {
    const a = thread("a");
    expect(usageScope(a, true)).toEqual({ scope: "every project" });
    expect(usageScope(a, true, "/other")).toEqual({ project: "/other", scope: "this project" });
    expect(usageScope(a, false)).toEqual({ project: "/repo", scope: "this project" });
    // Asking for its own project by name is fine; another one is refused.
    expect(usageScope(a, false, "/repo")).toEqual({ project: "/repo", scope: "this project" });
    const refused = usageScope(a, false, "/other");
    expect(isFailure(refused) && refused.code).toBe("not_allowed");
    expect(isFailure(refused) ? refused.message : "").toContain("Settings › Agents");
  });

  test("SQL needs the setting: it cannot be scoped to one project", () => {
    expect(checkQueryData(true)).toBeNull();
    const refused = checkQueryData(false);
    expect(refused?.code).toBe("not_allowed");
    expect(refused?.message).toContain("Let agents read threads in other projects");
  });
});

describe("the link book kept in the data store", () => {
  test("parses what the store answers, and refuses what cannot be a book", () => {
    expect(parseBook(undefined)).toBeNull();
    expect(parseBook([] as never)).toBeNull();
    expect(parseBook("nope" as never)).toBeNull();
    const parsed = parseBook({
      links: [
        {
          ...link("a", "b", "done"),
          id: "a/b",
          settledAt: 5_000,
          notifiedAt: 6_000,
          answer: "it is done",
          laterField: { kept: true },
        },
        // A link without an id or with a state this build cannot read is dropped.
        { orchestrator: "a", target: "c", state: "working", createdAt: 1 },
        { ...link("a", "d", "working"), state: "somewhere-new" },
      ],
      grants: { a: ["start", 7] },
      counters: { a: 3, b: "two" },
      fanOuts: [{ id: "fan-1", orchestrator: "a", links: ["a/b"] }, { id: "lost" }],
      // A field a later release added is dropped, not an error.
      future: { kept: false },
    })!;
    expect(parsed.links.map((l) => l.id)).toEqual(["a/b"]);
    expect(parsed.links[0]).toMatchObject({ state: "done", settledAt: 5_000, answer: "it is done" });
    expect((parsed.links[0] as unknown as { laterField: unknown }).laterField).toEqual({ kept: true });
    expect(parsed.grants).toEqual({ a: ["start"] });
    expect(parsed.counters).toEqual({ a: 3 });
    expect(parsed.fanOuts.map((f) => f.id)).toEqual(["fan-1"]);
  });

  test("merges the store's copy with this window's", () => {
    const shared = {
      ...emptyBook(),
      links: [{ ...link("a", "b", "working"), askedRequest: "7" }],
      grants: { a: ["start"] },
    };
    const mine = {
      ...emptyBook(),
      links: [
        // Settled here, still working in the store: the settled record wins.
        { ...link("a", "b", "working"), state: "done" as const, settledAt: 9_000, answer: "done" },
        // Started while the store was answering: nothing may lose it.
        link("a", "c", "working"),
      ],
      grants: { a: ["a/c"], b: ["start"] },
      counters: { a: 2 },
      fanOuts: [{ id: "fan-1", orchestrator: "a", title: "T", prompt: "p", links: ["a/c"], createdAt: 1 }],
    };
    const merged = mergeBooks(shared, mine);
    expect(merged.links.map((l) => [l.id, l.state])).toEqual([
      ["a/b", "done"],
      ["a/c", "working"],
    ]);
    // A mark either copy has is kept: the question reported before is not reported again.
    expect(merged.links.find((l) => l.id === "a/b")!.askedRequest).toBe("7");
    // Nothing is opened twice: the counter is the highest anyone saw, and a grant is kept.
    expect(merged.counters).toEqual({ a: 2 });
    expect(merged.grants).toEqual({ a: ["start", "a/c"], b: ["start"] });
    expect(merged.fanOuts.map((f) => f.id)).toEqual(["fan-1"]);
    // The store's copy of a link it alone has is kept as it is.
    const theirs = { ...emptyBook(), links: [link("z", "q", "done")] };
    expect(mergeBooks(theirs, emptyBook()).links.map((l) => l.id)).toEqual(["z/q"]);
  });
});

describe("projects", () => {
  test("a macOS /private path is the folder it links from", () => {
    expect(sameProject("/private/tmp/app", "/tmp/app")).toBe(true);
    expect(sameProject("/private/var/folders/x/", "/var/folders/x")).toBe(true);
    expect(sameProject("/private/tmpx/app", "/tmpx/app")).toBe(false);
    expect(sameProject("/Users/me/a", "/Users/me/b")).toBe(false);
  });
});

describe("waking a parent", () => {
  const done = (orchestrator: string, target: string, at = 1_000, state: Link["state"] = "done"): Link => ({
    ...link(orchestrator, target, state),
    settledAt: at,
    answer: `${target} says done`,
    filesChanged: 2,
  });

  const ctx = (over: Partial<WakeContext> = {}): WakeContext => ({
    states: {},
    pending: {},
    waiting: {},
    threads: { a: { title: "Parent a", running: false } },
    now: 10_000,
    ...over,
  });

  test("coalesces parallel children into one message, once", () => {
    const two = book([done("a", "b"), done("a", "c")]);
    const wakes = wakesDue(two, ctx());
    expect(wakes).toHaveLength(1);
    expect(wakes[0]!.settled.map((l) => l.target)).toEqual(["b", "c"]);
    // Delivered: the same links are not reported again, in this run or after a reload.
    const after = markWoken(two, wakes[0]!, ctx(), 10_000);
    expect(wakesDue(after, ctx({ now: 20_000 }))).toEqual([]);
    expect(after.links.every((l) => l.notifiedAt === 10_000)).toBe(true);
  });

  test("a settled child waits for a sibling that is still running", () => {
    const mixed = book([done("a", "b", 9_500), link("a", "c", "working")]);
    // Just settled: inside the coalescing window, nothing is sent yet.
    expect(wakesDue(mixed, ctx())).toEqual([]);
    expect(nextWakeAt(mixed, ctx())).toBe(9_500 + WAKE_COALESCE_MS);
    // The window passed and the sibling still runs: report what is done.
    const later = ctx({ now: 9_500 + WAKE_COALESCE_MS });
    expect(wakesDue(mixed, later)[0]!.settled.map((l) => l.target)).toEqual(["b"]);
  });

  test("a parent already blocked in wait_session gets no duplicate", () => {
    const b = book([done("a", "b"), done("a", "c")]);
    const both = ctx({ waiting: { a: ["a/b", "a/c"] } });
    expect(wakesDue(b, both)).toEqual([]);
    // One of the two is taken out of the wait: only the other is new to it.
    const one = ctx({ waiting: { a: ["a/b"] } });
    expect(wakesDue(b, one)[0]!.settled.map((l) => l.target)).toEqual(["c"]);
  });

  test("a child parent is woken by its own children, and its parent hears only when it settles", () => {
    // c started a, a started b: b's result is a's to read, not c's.
    const chain = book([link("c", "a", "working"), done("a", "b")]);
    const threads = {
      a: { title: "A", running: false },
      c: { title: "C", running: false },
    };
    const wakes = wakesDue(chain, ctx({ threads }));
    expect(wakes.map((w) => w.parent)).toEqual(["a"]);
    expect(wakes[0]!.settled.map((l) => l.target)).toEqual(["b"]);
    // a's own link from c is still working, so c has nothing to hear yet.
    expect(wakes.every((w) => w.parent !== "c")).toBe(true);
    // A wake is not a delegation: marking b heard leaves a's own link alone.
    const heard = markWoken(chain, wakes[0]!, ctx({ threads }), 10_000);
    expect(heard.links.find((l) => l.id === "c/a")!.state).toBe("working");
    expect(heard.links.find((l) => l.id === "a/b")!.notifiedAt).toBe(10_000);
    // a's own turn ends: its parent hears about a, once.
    const settledA = updateLink(heard, "c/a", { state: "done", settledAt: 9_500 });
    const up = wakesDue(settledA, ctx({ threads }));
    expect(up.map((w) => w.parent)).toEqual(["c"]);
    expect(up[0]!.settled.map((l) => l.target)).toEqual(["a"]);
    expect(
      wakesDue(markWoken(settledA, up[0]!, ctx({ threads }), 20_000), ctx({ threads, now: 30_000 })),
    ).toEqual([]);
  });

  test("a parent that is gone is dropped, and its links are marked", () => {
    const b = book([done("a", "b")]);
    const gone = ctx({ threads: {} });
    expect(wakesDue(b, gone)).toEqual([]);
    expect(orphanedLinks(b, gone).map((l) => l.id)).toEqual(["a/b"]);
    // Nothing is orphaned while the parent exists.
    expect(orphanedLinks(b, ctx())).toEqual([]);
  });

  test("one notice per pending request", () => {
    const asking = updateLink(book([link("a", "b", "working")]), "a/b", { askedRequest: "7" });
    const waiting = (request: string) =>
      ctx({ states: { "a/b": "needs_user" }, pending: { "a/b": request } });
    // The request reported before is not reported again.
    expect(wakesDue(asking, waiting("7"))).toEqual([]);
    // A new request on the same child is.
    const next = wakesDue(asking, waiting("8"));
    expect(next[0]!.asking.map((l) => l.id)).toEqual(["a/b"]);
    const after = markWoken(asking, next[0]!, waiting("8"), 10_000);
    expect(after.links[0]!.askedRequest).toBe("8");
    expect(wakesDue(after, waiting("8"))).toEqual([]);
    // A question the parent is waiting on is not reported either.
    expect(wakesDue(asking, { ...waiting("8"), waiting: { a: ["a/b"] } })).toEqual([]);
  });

  test("the message says what finished, what waits, and not to poll", () => {
    const b = book([done("a", "b"), link("a", "c", "working")]);
    const wake = wakesDue(b, ctx({ states: { "a/c": "needs_user" }, pending: { "a/c": "3" } }))[0]!;
    const text = wakeMessage(wake, 10_000);
    expect(text.split("\n")[0]).toBe("1 child finished · 1 needs you");
    // It marks itself, so a wake turn replayed from history is still a wake (state/thread.ts).
    expect(isWakeMessage(text)).toBe(true);
    expect(text).toContain("link_id a/b");
    expect(text).toContain("done");
    expect(text).toContain("session b");
    expect(text).toContain("2 files changed");
    expect(text).toContain("b says done");
    expect(text).toContain("list_pending_requests");
    expect(text).toContain("read_session");
    expect(text).toContain("do not poll with sleeps");
    // Each answer is bounded, whatever the child said.
    const long = { ...done("a", "b"), answer: "x".repeat(5_000) };
    const cappedText = wakeMessage({ ...wake, settled: [long], asking: [] }, 10_000);
    expect(cappedText).toContain("…");
    expect(cappedText.length).toBeLessThan(WAKE_ANSWER_CHARS + 1_000);
  });

  test("a follow-up to the same thread retires the earlier answer's wake", () => {
    // The shape seen live: A messaged B (link a/1), B settled, A messaged B again (a/2) and read that
    // answer through wait_session. The first answer was still unreported, because B was working again
    // when it settled, and it must not land as a wake 11 minutes later.
    const first: Link = { ...done("a", "b", 1_000), id: "a/1", createdAt: 1_000 };
    const again: Link = { ...link("a", "b", "working"), id: "a/2", createdAt: 4_000 };
    const both = book([first, again]);
    expect(supersededLinks(both, ctx())).toEqual([first]);
    expect(wakesDue(both, ctx())).toEqual([]);
    expect(nextWakeAt(both, ctx())).toBeNull();
    // The newer turn's answer is the one the parent hears, once it settles.
    const newer: Link = { ...done("a", "b", 9_500), id: "a/2", createdAt: 4_000 };
    const settles = book([first, newer]);
    const wakes = wakesDue(settles, ctx({ now: 20_000 }));
    expect(wakes[0]!.settled.map((l) => l.id)).toEqual(["a/2"]);
    // A link the parent was already told about leaves nothing to supersede.
    const told = book([{ ...first, notifiedAt: 2_000 }, newer]);
    expect(supersededLinks(told, ctx())).toEqual([]);
    // Another parent's link to the same thread is its own answer, not a newer one.
    const other = book([first, { ...again, id: "z/1", orchestrator: "z" }]);
    expect(
      supersededLinks(
        other,
        ctx({ threads: { a: { title: "A", running: false }, z: { title: "Z", running: false } } }),
      ),
    ).toEqual([]);
  });

  test("a wake is a message, not a new link", () => {
    const b = book([done("a", "b")]);
    const wake = wakesDue(b, ctx())[0]!;
    const after = markWoken(b, wake, ctx(), 10_000);
    // No link appears, and no other parent suddenly has something to hear.
    expect(after.links).toHaveLength(1);
    expect(wakesDue(after, ctx({ now: 11_000 }))).toEqual([]);
    expect(isActive(after.links[0]!.state)).toBe(false);
  });

  // Top started Middle, and Middle started Leaf: the live shape this had to get right.
  const chainCtx = (over: Partial<WakeContext> = {}) =>
    ctx({
      threads: {
        top: { title: "Top", running: false },
        middle: { title: "Middle", running: false },
        leaf: { title: "Leaf", running: false },
      },
      ...over,
    });
  const said = (text: string) => ({ text, truncated: false, filesChanged: 0 });

  test("a middle thread is not finished for its parent while its own child works", () => {
    // Middle's first turn ended with "STARTED LEAF"; Leaf still works.
    const started = book([
      { ...done("top", "middle", 9_000), answer: "STARTED LEAF" },
      link("middle", "leaf", "working"),
    ]);
    expect(quietTree(started, chainCtx(), "middle")).toBe(false);
    expect(wakesDue(started, chainCtx())).toEqual([]);

    // Leaf finishes: Middle has a child result to be told about, so it is not quiet yet either.
    const leafDone = updateLink(started, "middle/leaf", { state: "done", settledAt: 9_500, answer: "leaf" });
    const forMiddle = wakesDue(leafDone, chainCtx({ now: 9_600 }));
    expect(forMiddle.map((w) => w.parent)).toEqual(["middle"]);
    expect(wakesDue(leafDone, chainCtx({ now: 9_600 })).every((w) => w.parent !== "top")).toBe(true);

    // Middle is woken: its own turn runs, and Top still waits.
    const told = markWoken(leafDone, forMiddle[0]!, chainCtx({ now: 9_600 }), 9_600);
    const running = chainCtx({
      now: 9_700,
      threads: { ...chainCtx().threads, middle: { title: "Middle", running: true } },
    });
    expect(quietTree(told, running, "middle")).toBe(false);
    expect(wakesDue(told, running)).toEqual([]);

    // That wake turn ends with the real answer: the settled link Top holds takes it.
    const answered = restateLinks(
      told,
      "middle",
      said("leaf is done; here is the real answer"),
      10_000,
      true,
    );
    expect(answered.links.find((l) => l.id === "top/middle")!.answer).toBe(
      "leaf is done; here is the real answer",
    );
    expect(answered.links.find((l) => l.id === "top/middle")!.settledAt).toBe(10_000);
    const up = wakesDue(answered, chainCtx({ now: 11_000 }));
    expect(up.map((w) => w.parent)).toEqual(["top"]);
    expect(wakeMessage(up[0]!, 11_000)).toContain("here is the real answer");
    // Once only.
    const after = markWoken(answered, up[0]!, chainCtx({ now: 11_000 }), 11_000);
    expect(wakesDue(after, chainCtx({ now: 20_000 }))).toEqual([]);
  });

  test("two leaves, or a failed one, hold the middle back until they are all read", () => {
    const leaves = book([
      done("top", "middle", 9_000),
      { ...done("middle", "leaf", 9_400) },
      { ...done("middle", "other", 9_600, "failed"), answer: "it broke" },
    ]);
    const threads = { ...chainCtx().threads, leaf: { title: "Leaf", running: false } };
    // A failed child is a result like any other: Middle has to read it before Top hears.
    expect(quietTree(leaves, ctx({ threads, now: 9_900 }), "middle")).toBe(false);
    expect(pendingWakeUnder(leaves, ctx({ threads, now: 9_900 }), "middle")).toBe(true);
    expect(wakesDue(leaves, ctx({ threads, now: 9_900 })).map((w) => w.parent)).toEqual(["middle"]);
    // Middle is told about both at once, and then Top hears once.
    const told = wakesDue(leaves, ctx({ threads, now: 9_900 }))[0]!;
    expect(told.settled.map((l) => l.target).sort()).toEqual(["leaf", "other"]);
    const marked = markWoken(leaves, told, ctx({ threads, now: 9_900 }), 9_900);
    // Being told starts Middle's own turn (that is how the wake is delivered): Top still waits.
    const waking = ctx({
      threads: { ...threads, middle: { title: "Middle", running: true } },
      now: 9_950,
    });
    expect(wakesDue(marked, waking).every((w) => w.parent !== "top")).toBe(true);
    const answered = restateLinks(marked, "middle", said("both leaves are read"), 10_000, true);
    expect(wakesDue(answered, ctx({ threads, now: 10_100 })).map((w) => w.parent)).toEqual(["top"]);
  });

  test("a middle that never wakes does not leave its parent waiting: an interrupted child is no result", () => {
    // Leaf was stopped, not finished: nothing will ever report it, so Middle is quiet and Top hears.
    const stopped = book([
      { ...done("top", "middle", 9_000), answer: "STARTED LEAF" },
      link("middle", "leaf", "interrupted"),
    ]);
    expect(pendingWakeUnder(stopped, chainCtx(), "middle")).toBe(false);
    expect(quietTree(stopped, chainCtx(), "middle")).toBe(true);
    const up = wakesDue(stopped, chainCtx());
    expect(up.map((w) => w.parent)).toEqual(["top"]);
    expect(up[0]!.settled.map((l) => l.target)).toEqual(["middle"]);
  });

  test("a reported link is re-armed once for the turn its own child's wake started", () => {
    const reported = book([{ ...done("top", "middle", 9_000), answer: "STARTED LEAF", notifiedAt: 9_100 }]);
    // A user's own turn in Middle does not tell Top again.
    const userTurn = restateLinks(reported, "middle", said("more work by hand"), 10_000, false);
    expect(userTurn.links[0]!.notifiedAt).toBe(9_100);
    expect(userTurn.links[0]!.answer).toBe("STARTED LEAF");
    // A turn started by the wake from Middle's own child does, once.
    const woken = restateLinks(reported, "middle", said("the real answer"), 10_000, true);
    expect(woken.links[0]!.notifiedAt).toBeUndefined();
    expect(woken.links[0]!.answerChangedAt).toBe(10_000);
    expect(quietTree(woken, chainCtx({ now: 10_100 }), "middle")).toBe(true);
    const up = wakesDue(woken, chainCtx({ now: 10_200 }));
    expect(up.map((w) => w.parent)).toEqual(["top"]);
    const after = markWoken(woken, up[0]!, chainCtx({ now: 10_200 }), 10_200);
    // Another wake turn changes nothing more: no loop, no third message.
    const again = restateLinks(after, "middle", said("and again"), 11_000, true);
    expect(again.links[0]!.notifiedAt).toBe(10_200);
    expect(wakesDue(again, chainCtx({ now: 12_000 }))).toEqual([]);
  });

  test("a finished link never loses its answer to a turn that said nothing", () => {
    const good = book([{ ...done("top", "middle", 9_000), answer: "MIDDLE DONE" }]);
    // A turn whose rows carried no reply (they arrived later, or the harness said nothing) must
    // not blank the answer the parent is about to read.
    const silent = restateLinks(good, "middle", said(""), 10_000, false);
    expect(silent.links[0]!.answer).toBe("MIDDLE DONE");
    const spaces = restateLinks(good, "middle", said("   \n "), 10_000, false);
    expect(spaces.links[0]!.answer).toBe("MIDDLE DONE");
    // A turn that answered does take over.
    const again = restateLinks(good, "middle", said("the real answer"), 10_000, false);
    expect(again.links[0]!.answer).toBe("the real answer");
    // A reported link is re-armed once for a wake turn that answered, and not for one that did not.
    const reported = book([{ ...done("top", "middle", 9_000), answer: "STARTED LEAF", notifiedAt: 9_100 }]);
    const silentWake = restateLinks(reported, "middle", said(""), 10_000, true);
    expect(silentWake.links[0]!.notifiedAt).toBe(9_100);
    expect(silentWake.links[0]!.answer).toBe("STARTED LEAF");
    const spoke = restateLinks(reported, "middle", said("MIDDLE DONE"), 10_000, true);
    expect(spoke.links[0]!.notifiedAt).toBeUndefined();
  });

  test("a link that settled without an answer takes the thread's last reply", () => {
    // The shapes the app's transcript holds: the reply can sit before the turn's own rows.
    const items = [
      { kind: "user", text: "From thread “Middle” (agent): …" },
      { kind: "tool", text: "" },
      { kind: "assistant", text: "LEAF OK" },
      { kind: "turnEnd", text: "" },
    ];
    expect(latestReply(items)).toBe("LEAF OK");
    expect(latestReply([{ kind: "assistant", text: "   " }, { kind: "turnEnd" }])).toBe("");
    expect(latestReply([])).toBe("");
    // An empty row a whitespace chunk left after the reply (a "\n" between two calls) loses.
    expect(
      latestReply([
        { kind: "assistant", text: "LEAF OK" },
        { kind: "tool", text: "" },
        { kind: "assistant", text: "  \n " },
      ]),
    ).toBe("LEAF OK");

    const empty = book([{ ...done("middle", "leaf", 9_000), answer: "" }]);
    const filled = fillAnswers(empty, { leaf: latestReply(items) });
    expect(filled.links[0]!.answer).toBe("LEAF OK");
    // The wake message then carries that answer, not an empty line.
    const threads = { middle: { title: "Middle", running: false } };
    const up = wakesDue(filled, ctx({ threads }));
    expect(up.map((w) => w.parent)).toEqual(["middle"]);
    expect(wakeMessage(up[0]!, 10_000)).toContain("LEAF OK");

    // A link that has an answer keeps it, and an active one is never filled.
    const kept = book([
      { ...done("middle", "leaf", 9_000), answer: "LEAF OK" },
      link("middle", "other", "working"),
    ]);
    expect(fillAnswers(kept, { leaf: "other reply", other: "other reply" })).toBe(kept);
  });

  test("a link still working is left alone by a turn of its target", () => {
    const working = book([link("top", "middle", "working")]);
    const after = restateLinks(working, "middle", said("still going"), 10_000, false);
    expect(after.links[0]).toEqual(working.links[0]);
  });
});

describe("a group of threads", () => {
  test("its header counts what is working and what needs you", () => {
    const counts = linkCounts(["working", "queued", "needs_user", "done", "failed"]);
    expect(counts).toEqual({ working: 2, needs: 1, failed: 1 });
    expect(linkGroupStatus(counts)).toBe("2 working · 1 needs you");
    expect(linkGroupStatus(linkCounts(["done", "done"]))).toBe("done");
    expect(linkGroupStatus(linkCounts(["failed", "failed"]))).toBe("2 failed");
    expect(linkGroupStatus(linkCounts(["needs_user", "needs_user"]))).toBe("2 need you");
  });

  test("the strip shows what works now, and what the transcript has no row for", () => {
    const working = link("a", "b", "working");
    const started = { ...link("a", "c", "done"), started: true };
    const messaged = link("a", "d", "done");
    const links = [working, started, messaged];
    // `c` has a row in the transcript; `b` works, so the strip is its live place; `d` has none.
    expect(stripLinks(links, new Set(["c"])).map((l) => l.target)).toEqual(["b", "d"]);
    expect(stripLinks(links, new Set(["b", "c"])).map((l) => l.target)).toEqual(["b", "d"]);
    expect(stripLinks(links, new Set(["b", "c", "d"]))).toEqual([working]);
  });
});

describe("answering with the slots and the child's activity", () => {
  test("a busy refusal says how many threads are left", () => {
    const full = book(Array.from({ length: MAX_ACTIVE_LINKS }, (_, i) => link("a", `t${i}`)));
    const refused = checkStart(thread("a"), full, 2)!;
    expect(refused.code).toBe("busy");
    expect(refused.linksLeft).toBe(0);
    expect(refused.message).toContain(
      `at most ${MAX_ACTIVE_LINKS} threads at once; wait_session on one first`,
    );
    const json = failureJson(refused);
    expect(json.error).toMatchObject({ code: "busy", links_left: 0 });
    // One slot open: a single start fits, two do not.
    const room = book(Array.from({ length: MAX_ACTIVE_LINKS - 1 }, (_, i) => link("a", `t${i}`)));
    expect(checkStart(thread("a"), room, 1)).toBeNull();
    expect(checkStart(thread("a"), room, 2)?.linksLeft).toBe(1);
    // A thread limit of two says two, and its tree says four.
    const two = book([link("a", "b"), link("a", "c")]);
    const small = checkStart(thread("a"), two, 1, 2)!;
    expect(small.message).toContain("at most 2 threads at once");
    const tiny = book([link("a", "b"), link("b", "c"), link("b", "d"), link("b", "e")]);
    expect(checkStart(thread("a"), tiny, 1, 2)?.message).toContain("tree holds at most 4");
  });

  test("a timeout's link says how long it has been idle and what it did last", () => {
    const items = [
      { kind: "user", at: 4_000 },
      { kind: "tool", title: "bash", startedAt: 5_000, endedAt: 6_000 },
      { kind: "tool", title: "read", toolName: "read_session", startedAt: 7_000, endedAt: 8_000 },
      { kind: "turnEnd", at: 8_500 },
    ];
    expect(activityOf(items)).toEqual({ lastAt: 8_500, lastTool: "read_session", turns: 1 });
    const entry = linkResult(link("a", "b", "working"), "working", undefined, {
      now: 12_000,
      activity: activityOf(items),
    });
    expect(entry).toMatchObject({ idle_ms: 3_500, last_tool: "read_session", turns: 1 });
  });

  test("a cut answer says so and names the thread whose read_session has the rest", () => {
    const cut = { ...link("a", "b", "done"), answer: "x", truncated: true };
    const entry = linkResult(cut, "done");
    expect(entry).toMatchObject({ answer_truncated: true, session_id: "b", link_id: "a/b" });
    // An answer that fits says nothing about truncation.
    expect(linkResult({ ...cut, truncated: false }, "done")).not.toHaveProperty("answer_truncated");
  });
});

describe("paging a rendered answer", () => {
  test("a page walk over multibyte text loses and repeats nothing", () => {
    const text = `${'å🦀\n\t\u0001"\\'.repeat(4_000)}\nend`;
    let offset = 0;
    let recovered = "";
    for (let pages = 0; ; pages += 1) {
      const page = pageOf(text, offset);
      expect(isFailure(page)).toBe(false);
      if (isFailure(page)) return;
      expect(new TextEncoder().encode(page.text).length).toBeLessThanOrEqual(16 * 1024);
      recovered += page.text;
      if (page.nextOffset === null) break;
      expect(page.nextOffset).toBeGreaterThan(offset);
      offset = page.nextOffset;
      expect(pages).toBeLessThan(100);
    }
    expect(recovered).toBe(text);
  });

  test("a rendered read pages whole characters, and a bad offset is refused", () => {
    const answer = {
      session_id: "b",
      title: "Thread 🦀",
      status: "idle",
      project: "/repo",
      turns: Array.from({ length: 6 }, (_, i) => ({
        user: `ask ${i} å`.repeat(400),
        assistant: "🦀 done\n".repeat(900),
        files_changed: i,
      })),
    };
    const rendered = renderRead(answer);
    expect(rendered).toContain("Thread: Thread 🦀");
    expect(rendered).toContain("files changed: 5");
    let offset = 0;
    let recovered = "";
    for (;;) {
      const page = pageOf(rendered, offset);
      if (isFailure(page)) throw new Error(page.message);
      recovered += page.text;
      if (page.nextOffset === null) break;
      offset = page.nextOffset;
    }
    expect(recovered).toBe(rendered);
    // An offset inside a character is refused rather than moved, as zoom_context does.
    const second = new TextEncoder().encode("🦀")[1]!;
    const inside = pageOf("🦀x", 1);
    expect(inside).toMatchObject({ code: "invalid" });
    expect(second & 0xc0).toBe(0x80);
    expect(isFailure(pageOf(rendered, rendered.length * 4))).toBe(true);
    expect(pageOf(rendered, new TextEncoder().encode(rendered).length)).toMatchObject({
      text: "",
      nextOffset: null,
    });
  });
});
