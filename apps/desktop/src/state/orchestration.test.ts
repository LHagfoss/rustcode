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
  activityOf,
  capped,
  checkManage,
  checkMessage,
  checkQueryData,
  checkStart,
  emptyBook,
  failureJson,
  forget,
  interruptActive,
  isActive,
  isFailure,
  type Link,
  type LinkBook,
  linkResult,
  listSessions,
  MAX_DATA_ROWS,
  markWoken,
  nextWakeAt,
  orphanedLinks,
  parseCall,
  type RuleThread,
  sameProject,
  updateLink,
  usageScope,
  WAKE_ANSWER_CHARS,
  WAKE_COALESCE_MS,
  type WakeContext,
  wakeMessage,
  wakesDue,
} from "./orchestration";

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
    });
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
      isFailure(parseCall({ tool: "message_session", args: { session_id: "b", message: "x".repeat(9000) } })),
    ).toBe(true);
    const wait = parseCall({ tool: "wait_session", args: { link_id: "a/1", timeout_ms: 9e9 } });
    expect(!isFailure(wait) && wait.tool === "wait_session" && wait.timeoutMs).toBe(3_600_000);
  });

  test("same project, no chains, four links, one orchestrator per target", () => {
    const a = thread("a");
    expect(checkMessage(a, thread("b", "/other"), emptyBook())?.code).toBe("not_allowed");
    expect(checkMessage(a, a, emptyBook())?.code).toBe("not_allowed");
    expect(checkMessage(a, undefined, emptyBook())?.code).toBe("not_found");
    // A worktree thread belongs to its repository's project.
    const wt = thread("w", "/wt/x", { worktree: { path: "/wt/x", branch: "b", repo: "/repo" } });
    expect(checkMessage(a, wt, emptyBook())).toBeNull();
    expect(checkMessage(a, thread("b"), book([link("z", "a")]))?.code).toBe("not_allowed");
    expect(checkMessage(a, thread("b"), book([link("z", "b")]))?.code).toBe("target_linked");
    const four = ["c", "d", "e", "f"].map((t) => link("a", t));
    expect(checkMessage(a, thread("b"), book(four))?.code).toBe("busy");
    expect(checkStart(a, book(four.slice(0, 2)), 3)?.code).toBe("busy");
    expect(checkStart(a, book(four.slice(0, 2)), 2)).toBeNull();
    expect(checkStart(a, book([link("z", "a")]), 1)?.code).toBe("not_allowed");
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
    parents: { a: { title: "Parent a", running: false } },
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

  test("a child works for another thread, so it is not woken", () => {
    const chain = book([link("c", "a", "working"), done("a", "b")]);
    expect(wakesDue(chain, ctx({ parents: { a: { title: "A", running: false } } }))).toEqual([]);
  });

  test("a parent that is gone is dropped, and its links are marked", () => {
    const b = book([done("a", "b")]);
    const gone = ctx({ parents: {} });
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

  test("a wake is a message, not a new link", () => {
    const b = book([done("a", "b")]);
    const wake = wakesDue(b, ctx())[0]!;
    const after = markWoken(b, wake, ctx(), 10_000);
    // No link appears, and no other parent suddenly has something to hear.
    expect(after.links).toHaveLength(1);
    expect(wakesDue(after, ctx({ now: 11_000 }))).toEqual([]);
    expect(isActive(after.links[0]!.state)).toBe(false);
  });
});

describe("answering with the slots and the child's activity", () => {
  test("a busy refusal says how many links are left", () => {
    const full = book([link("a", "b"), link("a", "c"), link("a", "d"), link("a", "e")]);
    const refused = checkStart(thread("a"), full, 2)!;
    expect(refused.code).toBe("busy");
    expect(refused.linksLeft).toBe(0);
    expect(refused.message).toContain("at most 4 at once; wait_session on one first");
    const json = failureJson(refused);
    expect(json.error).toMatchObject({ code: "busy", links_left: 0 });
    // One slot open: a single start fits, two do not.
    const three = book([link("a", "b"), link("a", "c"), link("a", "d")]);
    expect(checkStart(thread("a"), three, 1)).toBeNull();
    expect(checkStart(thread("a"), three, 2)?.linksLeft).toBe(1);
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
});
