import { describe, expect, test } from "bun:test";
import type { Json } from "../acp/jsonrpc";
import {
  checkpointsByTurn,
  parseTurnDiff,
  parseWorktree,
  removeRefusal,
  suggestBranch,
  validBranch,
} from "../acp/v2";
import {
  activeDelegates,
  activeDescendants,
  activityOf,
  askingIds,
  busiestThreadDelegates,
  CAPABILITY_VERSION,
  capped,
  checkManage,
  checkMessage,
  checkQueryData,
  checkStart,
  contextNote,
  DEFAULT_ACTIVITY_ITEMS,
  DEFAULT_DELEGATE_LIMITS,
  DEFAULT_DELEGATES_PER_THREAD,
  DEFAULT_DELEGATES_TOTAL,
  DEFAULT_MERGE_MS,
  DELEGATE_REPORT,
  delegated,
  delivered,
  depthOf,
  doneLinks,
  effectivePerThreadLimit,
  emptyBook,
  failQueuedLinks,
  failureJson,
  fillAnswers,
  foldWake,
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
  linkThreads,
  listSessions,
  MACHINE_LOST_TURN,
  MAX_ACTIVITY_ITEMS,
  MAX_DATA_ROWS,
  MAX_DEPTH,
  MAX_MESSAGE_BYTES,
  markDelivered,
  mergeBooks,
  nextWakeAt,
  orphanedLinks,
  pageOf,
  parseBook,
  parseCall,
  pendingWakeUnder,
  queueWake,
  quietTree,
  type RuleThread,
  reachesThread,
  renderRead,
  reopenLinks,
  restateLinks,
  STALL_NOTICE_MS,
  sameProject,
  sameReach,
  stripLinks,
  supersededLinks,
  unheard,
  updateLink,
  usageScope,
  WAKE_ANSWER_CHARS,
  WAKE_COALESCE_MS,
  type Wake,
  type WakeContext,
  waitEnded,
  waitStarted,
  wakeFor,
  wakeIds,
  wakeMessage,
  wakeStillOwed,
  wakeSummary,
  wakesDue,
} from "./orchestration";
import { isWakeMessage, type QueuedPrompt, wakeLine, withWakeMark } from "./thread";

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
    // The filters are optional, and what is there is read as a trimmed string.
    expect(parseCall({ tool: "list_harnesses", args: { harness: " codex ", model: " Opus " } })).toEqual({
      tool: "list_harnesses",
      harness: "codex",
      model: "Opus",
    });
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
    // v11: the project tools and the scheduler's four.
    expect(parseCall({ tool: "read_project", args: { path: " /abs/project " } })).toEqual({
      tool: "read_project",
      path: "/abs/project",
    });
    expect(isFailure(parseCall({ tool: "read_project", args: { path: " " } }))).toBe(true);
    expect(parseCall({ tool: "create_project", args: { path: "/abs/new" } })).toEqual({
      tool: "create_project",
      path: "/abs/new",
      init: undefined,
    });
    expect(parseCall({ tool: "create_project", args: { path: "/abs/new", init: false } })).toMatchObject({
      init: false,
    });
    expect(parseCall({ tool: "clone_project", args: { url: "git@github.com:o/r.git", name: "r" } })).toEqual({
      tool: "clone_project",
      url: "git@github.com:o/r.git",
      parent: undefined,
      name: "r",
    });
    expect(isFailure(parseCall({ tool: "clone_project", args: {} }))).toBe(true);
    expect(parseCall({ tool: "update_project", args: { path: "/abs/p", harness: " codex " } })).toEqual({
      tool: "update_project",
      path: "/abs/p",
      name: undefined,
      harness: "codex",
      model: undefined,
    });
    expect(isFailure(parseCall({ tool: "update_project", args: { path: "/abs/p" } }))).toBe(true);
    expect(parseCall({ tool: "remove_project", args: { path: "/abs/p" } })).toEqual({
      tool: "remove_project",
      path: "/abs/p",
    });
    expect(
      parseCall({
        tool: "schedule_task",
        args: {
          prompt: "Check the build",
          schedule: { kind: "cron", expression: "0 9 * * 1-5", timezone: "Europe/Oslo" },
          project: "/abs/project",
        },
      }),
    ).toEqual({
      tool: "schedule_task",
      prompt: "Check the build",
      schedule: { kind: "cron", expression: "0 9 * * 1-5", timezone: "Europe/Oslo" },
      project: "/abs/project",
      name: undefined,
      modelProfile: undefined,
    });
    expect(
      parseCall({
        tool: "schedule_task",
        args: { prompt: "Once", schedule: { kind: "at", at: "2026-03-04T09:30:00+01:00" } },
      }),
    ).toMatchObject({ schedule: { kind: "at", at: "2026-03-04T09:30:00+01:00" } });
    expect(
      isFailure(parseCall({ tool: "schedule_task", args: { prompt: "x", schedule: { kind: "daily" } } })),
    ).toBe(true);
    expect(
      isFailure(
        parseCall({
          tool: "schedule_task",
          args: { prompt: "x", schedule: { kind: "cron", expression: "0 9 * *" } },
        }),
      ),
    ).toBe(true);
    expect(
      isFailure(
        parseCall({ tool: "schedule_task", args: { prompt: "x", schedule: { kind: "at", at: "later" } } }),
      ),
    ).toBe(true);
    expect(parseCall({ tool: "list_scheduled_tasks", args: {} })).toEqual({ tool: "list_scheduled_tasks" });
    expect(parseCall({ tool: "delete_scheduled_task", args: { id: " task-1 " } })).toEqual({
      tool: "delete_scheduled_task",
      id: "task-1",
    });
    expect(parseCall({ tool: "run_scheduled_task_now", args: { id: "task-1" } })).toEqual({
      tool: "run_scheduled_task_now",
      id: "task-1",
    });
    expect(isFailure(parseCall({ tool: "run_scheduled_task_now", args: {} }))).toBe(true);
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
    // Answering what a thread waits on: an approval's choice, or a question's answer.
    expect(
      parseCall({ tool: "answer_request", args: { session_id: "c1", request_id: "7", choice: "deny" } }),
    ).toEqual({ tool: "answer_request", target: "c1", requestId: "7", choice: "deny" });
    expect(
      parseCall({
        tool: "answer_request",
        args: { session_id: "c1", request_id: " 7 ", answer: "  yes, run it " },
      }),
    ).toEqual({ tool: "answer_request", target: "c1", requestId: "7", answer: "yes, run it" });
    for (const args of [
      {},
      { session_id: "c1" },
      // The engine's name check refuses a number, so the app does too, as it is printed.
      { session_id: "c1", request_id: 7, choice: "allow_once" },
      { session_id: "c1", request_id: "7" },
      { session_id: "c1", request_id: "7", choice: "allow_hour" },
      { session_id: "c1", request_id: "7", choice: "deny", answer: "no" },
      { session_id: "c1", request_id: "7", answer: "  " },
    ] as Record<string, Json>[])
      expect(isFailure(parseCall({ tool: "answer_request", args }))).toBe(true);
    // Organizing several at once: one change set, at least one thread.
    expect(
      parseCall({ tool: "organize_threads", args: { session_ids: ["a", "b", "a"], pinned: true } }),
    ).toEqual({ tool: "organize_threads", targets: ["a", "b"], pinned: true });
    for (const args of [
      {},
      { session_ids: [] },
      { session_ids: ["a"] },
      { session_ids: ["a"], pinned: "yes" },
      { session_ids: [1], unread: true },
    ] as Record<string, Json>[])
      expect(isFailure(parseCall({ tool: "organize_threads", args }))).toBe(true);
    // Attachments are absolute paths in a list, at most ten.
    expect(
      parseCall({
        tool: "message_session",
        args: { session_id: "b", message: "look", attachments: ["/repo/a.png", " /repo/b.log "] },
      }),
    ).toEqual({
      tool: "message_session",
      target: "b",
      text: "look",
      attachments: ["/repo/a.png", "/repo/b.log"],
    });
    expect(
      isFailure(
        parseCall({ tool: "message_session", args: { session_id: "b", message: "x", attachments: "a" } }),
      ),
    ).toBe(true);
    // A delegate's pull request: report it, merge it, clean up after it.
    expect(parseCall({ tool: "thread_status", args: {} })).toEqual({
      tool: "thread_status",
      target: undefined,
    });
    expect(parseCall({ tool: "thread_status", args: { session_id: "d1" } })).toEqual({
      tool: "thread_status",
      target: "d1",
    });
    expect(parseCall({ tool: "merge_when_green", args: { session_id: "d1" } })).toEqual({
      tool: "merge_when_green",
      target: "d1",
      timeoutMs: DEFAULT_MERGE_MS,
    });
    expect(parseCall({ tool: "merge_when_green", args: { session_id: "d1", timeout_ms: 0 } })).toMatchObject({
      timeoutMs: 1,
    });
    expect(isFailure(parseCall({ tool: "merge_when_green", args: {} }))).toBe(true);
    // Settling after cleanup is the default; a caller can keep the thread.
    expect(parseCall({ tool: "cleanup_worktree", args: { session_id: "d1" } })).toEqual({
      tool: "cleanup_worktree",
      target: "d1",
      settle: true,
    });
    expect(parseCall({ tool: "cleanup_worktree", args: { session_id: "d1", settle: false } })).toMatchObject({
      settle: false,
    });
    expect(isFailure(parseCall({ tool: "cleanup_worktree", args: {} }))).toBe(true);
    // Where worktrees stand, and handing one back to the user.
    expect(parseCall({ tool: "worktree_status", args: {} })).toEqual({
      tool: "worktree_status",
      allProjects: false,
    });
    expect(parseCall({ tool: "worktree_status", args: { session_id: "d1", all_projects: true } })).toEqual({
      tool: "worktree_status",
      target: "d1",
      allProjects: true,
    });
    // Handing off defaults to the checkout: the direction the user asks for.
    expect(parseCall({ tool: "handoff_worktree", args: { session_id: "d1" } })).toEqual({
      tool: "handoff_worktree",
      target: "d1",
      to: "checkout",
    });
    expect(parseCall({ tool: "handoff_worktree", args: { session_id: "d1", to: "worktree" } })).toEqual({
      tool: "handoff_worktree",
      target: "d1",
      to: "worktree",
    });
    expect(isFailure(parseCall({ tool: "handoff_worktree", args: {} }))).toBe(true);
    expect(
      isFailure(parseCall({ tool: "handoff_worktree", args: { session_id: "d1", to: "nowhere" } })),
    ).toBe(true);
  });

  test("checkManage keeps a thread to its own project", () => {
    const a = { id: "a", title: "A", folder: "/repo", running: false };
    expect(checkManage(a, undefined)?.code).toBe("not_found");
    expect(checkManage(a, { ...a, id: "b", folder: "/other" })?.code).toBe("not_allowed");
    expect(checkManage(a, { ...a, id: "b" })).toBeNull();
  });
});

describe("orchestration rules", () => {
  test("standalone is read from start_session and fan_out, and only when asked for", () => {
    // A delegate is the default (#3443): the field is absent, not `false`, so the
    // engine's argument hash and the receipt book see the same call it always did.
    expect(parseCall({ tool: "start_session", args: { prompt: "go" } })).toEqual({
      tool: "start_session",
      prompt: "go",
      title: undefined,
      harness: undefined,
      model: undefined,
      effort: undefined,
      account: undefined,
      worktree: undefined,
      standalone: undefined,
      project: undefined,
    });
    expect(parseCall({ tool: "start_session", args: { prompt: "go", standalone: true } })).toMatchObject({
      tool: "start_session",
      prompt: "go",
      standalone: true,
    });
    const fan = parseCall({
      tool: "fan_out",
      args: { prompt: "go", harnesses: [{ harness: "codex" }, { harness: "pi" }], standalone: true },
    });
    expect(fan).toMatchObject({ tool: "fan_out", standalone: true });
    // Anything but a true boolean is not a standalone thread.
    expect(parseCall({ tool: "start_session", args: { prompt: "go", standalone: "yes" } })).toMatchObject({
      tool: "start_session",
      standalone: undefined,
    });
  });

  test("the app advertises the version whose arguments it answers", () => {
    // The engine strips `account` below 5, `attachments` below 10, `machine` below 12,
    // `steer` below 14 and `standalone` below 15, and refuses a call that carries one:
    // the app speaks the newest, so it is offered all five.
    expect(CAPABILITY_VERSION).toBe(15);
  });

  test("a machine is read from any call that takes one, and absent when it is not named", () => {
    // An id or a name `list_machines` gave; whitespace is not a machine.
    expect(parseCall({ tool: "list_sessions", args: { machine: " Studio " } })).toMatchObject({
      machine: "Studio",
    });
    expect(parseCall({ tool: "list_sessions", args: { machine: "  " } })).toEqual({
      tool: "list_sessions",
      query: "",
      allProjects: false,
    });
    expect(parseCall({ tool: "list_machines", args: {} })).toEqual({ tool: "list_machines" });
    expect(parseCall({ tool: "list_projects", args: { machine: "gw-1" } })).toMatchObject({
      machine: "gw-1",
    });
    expect(
      parseCall({ tool: "start_session", args: { prompt: "run it", machine: "Lars's Linux box" } }),
    ).toMatchObject({ machine: "Lars's Linux box", prompt: "run it" });
    expect(parseCall({ tool: "wait_session", args: { link_id: "a/1", machine: "Studio" } })).toMatchObject({
      machine: "Studio",
    });
    // A call that acts on a thread takes it too; the id may carry the machine itself.
    expect(
      parseCall({ tool: "interrupt_session", args: { session_id: "gw-1/t", machine: "Studio" } }),
    ).toEqual({ tool: "interrupt_session", target: "gw-1/t", machine: "Studio" });
    expect(parseCall({ tool: "thread_status", args: { session_id: "t", machine: "Studio" } })).toEqual({
      tool: "thread_status",
      target: "t",
      machine: "Studio",
    });
    // The tools that cannot work across machines take no machine at all.
    expect(parseCall({ tool: "merge_when_green", args: { session_id: "t", machine: "Studio" } })).toEqual({
      tool: "merge_when_green",
      target: "t",
      timeoutMs: DEFAULT_MERGE_MS,
    });
  });

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
    // A fan-out names the project on the machine it runs on, as a start does.
    expect(
      parseCall({
        tool: "fan_out",
        args: {
          prompt: "go",
          harnesses: [{ harness: "a" }, { harness: "b" }],
          project: "/srv/app",
          machine: "Studio",
        },
      }),
    ).toMatchObject({ project: "/srv/app", machine: "Studio" });
    // A named login rides on start_session and each fan_out entry.
    const started = parseCall({
      tool: "start_session",
      args: { prompt: "go", harness: "claude-code", account: "work" },
    });
    expect(!isFailure(started) && started.tool === "start_session" && started.account).toBe("work");
    expect(parseCall({ tool: "start_session", args: { prompt: "go", account: "  " } })).toMatchObject({
      account: undefined,
    });
    expect(
      parseCall({
        tool: "fan_out",
        args: {
          prompt: "go",
          harnesses: [
            { harness: "claude-code", account: "work" },
            { harness: "claude-code", model: "opus" },
          ],
        },
      }),
    ).toMatchObject({ harnesses: [{ account: "work" }, { account: undefined }] });
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
    expect(!isFailure(wait) && wait.tool === "wait_session" && wait.timeoutMs).toBe(1_500_000);
    // The engine's name for the call rides along, so a dropped call can end its wait.
    expect(
      parseCall({ tool: "wait_session", callId: "call-7", args: { link_id: "a/1", timeout_ms: 5 } }),
    ).toMatchObject({ callId: "call-7" });
    expect(parseCall({ tool: "wait_session", args: { link_id: "a/1", timeout_ms: 5 } })).toMatchObject({
      callId: undefined,
    });
    // Each answer once: a repeated link id is asked about once.
    expect(parseCall({ tool: "wait_session", args: { link_ids: ["a/1", "a/1", "a/2"] } })).toMatchObject({
      links: ["a/1", "a/2"],
    });
  });

  test("same project, a thread's delegate limit, one orchestrator per target", () => {
    const a = thread("a");
    expect(checkMessage(a, thread("b", "/other"), emptyBook())).toBeNull();
    expect(checkMessage(a, a, emptyBook())?.code).toBe("not_allowed");
    expect(checkMessage(a, undefined, emptyBook())?.code).toBe("not_found");
    // A worktree thread belongs to its repository's project.
    const wt = thread("w", "/wt/x", { worktree: { path: "/wt/x", branch: "b", repo: "/repo" } });
    expect(checkMessage(a, wt, emptyBook())).toBeNull();
    expect(checkMessage(a, thread("b"), book([link("z", "b")]))?.code).toBe("target_linked");
    const full = ["c", "d", "e", "f"].map((t) => link("a", t));
    expect(checkMessage(a, thread("b"), book(full), false, false, { perThread: 4, total: 16 })?.code).toBe(
      "busy",
    );
    expect(checkStart(a, book(full.slice(0, 2)), 3, { perThread: 4, total: 16 })?.code).toBe("busy");
    expect(checkStart(a, book(full.slice(0, 2)), 2, { perThread: 4, total: 16 })).toBeNull();
    // The default is eight per thread and sixteen in all.
    expect(checkStart(a, book(full.slice(0, 2)), 2)).toBeNull();
    const eight = ["c", "d", "e", "f", "g", "h", "i", "j"].map((t) => link("a", t));
    expect(checkStart(a, book(eight.slice(0, 7)), 1)).toBeNull();
    expect(checkStart(a, book(eight), 1)?.code).toBe("busy");
    expect(checkStart(a, book(eight.slice(0, 2)), 7, { perThread: 2, total: 16 })?.code).toBe("busy");
    expect(checkStart(a, book(eight.slice(0, 1)), 1, { perThread: 2, total: 16 })).toBeNull();
    expect(checkStart(a, book(eight), 1, { perThread: 16, total: 16 })).toBeNull();
    // The machine-wide total refuses even when this thread has room of its own.
    expect(checkStart(a, book(eight), 1, { perThread: 16, total: 8 })?.code).toBe("busy");
  });

  test("a follow-up to a thread the caller already runs takes no slot", () => {
    // The live shape: a thread at the limit messaging a thread it is already running.
    const targets = Array.from({ length: DEFAULT_DELEGATES_PER_THREAD }, (_, i) => `t${i}`);
    const full = book(targets.map((t) => link("a", t)));
    expect(checkStart(thread("a"), full, 1)?.code).toBe("busy");
    expect(checkMessage(thread("a"), thread(targets[0]!), full)).toBeNull();
    // A second link to the same thread is still tracked, and a wait answers each once.
    const again = book([...full.links, link("a", targets[0]!)]);
    expect(checkMessage(thread("a"), thread(targets[0]!), again)).toBeNull();
    expect(again.links.filter((l) => l.target === targets[0])).toHaveLength(2);
    // Slots count distinct threads, not links: two links to one thread take one.
    const pair = book([link("a", "b"), link("a", "b"), link("a", "c")]);
    expect(linksLeft(pair, "a", { perThread: 4, total: 16 })).toBe(2);
    expect(linksLeft(full, "a")).toBe(0);
    // A machine-wide total filled by other threads does not refuse a follow-up either.
    const shared = book([
      ...full.links,
      ...Array.from({ length: DEFAULT_DELEGATES_TOTAL - targets.length }, (_, i) => link("other", `o${i}`)),
    ]);
    expect(activeDelegates(shared)).toBe(DEFAULT_DELEGATES_TOTAL);
    expect(
      checkMessage(thread("a"), thread(targets[0]!), shared, false, false, { perThread: 2, total: 16 }),
    ).toBeNull();
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

  test("the machine-wide total caps every thread's delegates at once", () => {
    // Two threads fill the total between them: one thread's own room is not the whole story.
    const under = (parent: string, n: number) =>
      Array.from({ length: n }, (_, i) => link(parent, `${parent}-${i}`));
    const filled = book([...under("a", 8), ...under("b", 8)]);
    expect(activeDelegates(filled)).toBe(DEFAULT_DELEGATES_TOTAL);
    expect(busiestThreadDelegates(filled)).toBe(8);
    const refused = checkStart(thread("c"), filled, 1)!;
    expect(refused.code).toBe("busy");
    expect(refused.message).toContain(`at most ${DEFAULT_DELEGATES_TOTAL} delegates at once across this app`);
    expect(refused.message).toContain("16 are running");
    expect(refused.linksLeft).toBe(0);
    // A message to another thread opens a link too, so the total holds for it as well.
    expect(checkMessage(thread("c"), thread("j"), filled)?.code).toBe("busy");
    // One less in the total: the last slot is there, and a fan-out of two is not.
    const room = book([...under("a", 8), ...under("b", 7)]);
    expect(checkStart(thread("c"), room, 1)).toBeNull();
    expect(checkStart(thread("c"), room, 2)?.code).toBe("busy");
    // A finished link frees its slot at once.
    const freed = book([...under("a", 8), ...under("b", 7), link("b", "done", "done")]);
    expect(activeDelegates(freed)).toBe(15);
    expect(checkStart(thread("c"), freed, 1, DEFAULT_DELEGATE_LIMITS)).toBeNull();
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

  test("reading needs the setting; messaging uses host approval across projects", () => {
    const a = thread("a");
    const other = thread("b", "/other");
    expect(checkMessage(a, other, emptyBook(), true)?.code).toBe("not_allowed");
    expect(checkMessage(a, other, emptyBook(), true, true)).toBeNull();
    expect(checkMessage(a, other, emptyBook(), false, true)).toBeNull();
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

  test("parses read_activity and clamps its page", () => {
    expect(parseCall({ tool: "read_activity", args: { session_id: "b", order: "newest" } })).toMatchObject({
      order: "newest",
    });
    expect(parseCall({ tool: "read_activity", args: { session_id: "b" } })).toEqual({
      tool: "read_activity",
      target: "b",
      after: undefined,
      limit: DEFAULT_ACTIVITY_ITEMS,
    });
    expect(
      parseCall({ tool: "read_activity", args: { session_id: "b", after: " at|event|3 ", limit: 2 } }),
    ).toEqual({ tool: "read_activity", target: "b", after: "at|event|3", limit: 2 });
    // A limit is clamped, never refused: the store bounds the page either way.
    expect(parseCall({ tool: "read_activity", args: { session_id: "b", limit: 1e9 } })).toMatchObject({
      limit: MAX_ACTIVITY_ITEMS,
    });
    expect(parseCall({ tool: "read_activity", args: { session_id: "b", limit: 0 } })).toMatchObject({
      limit: 1,
    });
    expect(isFailure(parseCall({ tool: "read_activity", args: {} }))).toBe(true);
  });

  test("parses manage_queue, which needs an action and, but for a list, a message", () => {
    expect(parseCall({ tool: "manage_queue", args: { session_id: "b", action: "list" } })).toEqual({
      tool: "manage_queue",
      target: "b",
      action: "list",
    });
    // A list names no message, so a queue_id beside it changes nothing.
    expect(
      parseCall({ tool: "manage_queue", args: { session_id: "b", action: "list", queue_id: "q1" } }),
    ).toEqual({
      tool: "manage_queue",
      target: "b",
      action: "list",
    });
    expect(
      parseCall({ tool: "manage_queue", args: { session_id: "b", action: "cancel", queue_id: " q1 " } }),
    ).toEqual({ tool: "manage_queue", target: "b", action: "cancel", queueId: "q1" });
    expect(
      parseCall({ tool: "manage_queue", args: { session_id: "b", action: "steer", queue_id: "q1" } }),
    ).toEqual({
      tool: "manage_queue",
      target: "b",
      action: "steer",
      queueId: "q1",
    });
    expect(
      parseCall({
        tool: "manage_queue",
        args: { session_id: "b", action: "edit", queue_id: "q1", text: " instead " },
      }),
    ).toEqual({ tool: "manage_queue", target: "b", action: "edit", queueId: "q1", text: " instead " });
    // An edit says what it says instead, as long as a message may be.
    expect(
      isFailure(
        parseCall({ tool: "manage_queue", args: { session_id: "b", action: "edit", queue_id: "q1" } }),
      ),
    ).toBe(true);
    expect(
      isFailure(
        parseCall({
          tool: "manage_queue",
          args: { session_id: "b", action: "edit", queue_id: "q1", text: "   " },
        }),
      ),
    ).toBe(true);
    expect(
      isFailure(
        parseCall({
          tool: "manage_queue",
          args: { session_id: "b", action: "edit", queue_id: "q1", text: "x".repeat(MAX_MESSAGE_BYTES + 1) },
        }),
      ),
    ).toBe(true);
    expect(
      isFailure(
        parseCall({ tool: "manage_queue", args: { session_id: "b", action: "reorder", queue_id: "q1" } }),
      ),
    ).toBe(true);
    expect(isFailure(parseCall({ tool: "manage_queue", args: { session_id: "b", action: "cancel" } }))).toBe(
      true,
    );
    expect(isFailure(parseCall({ tool: "manage_queue", args: { action: "list" } }))).toBe(true);
  });

  test("read_activity refuses an unknown thread and the caller's own", () => {
    const a = thread("a");
    expect(checkMessage(a, undefined, emptyBook(), true)?.code).toBe("not_found");
    expect(checkMessage(a, a, emptyBook(), true)?.code).toBe("not_allowed");
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
    expect(isFailure(refused) ? refused.message : "").toContain("Settings › Harnesses");
  });

  test("SQL needs the setting: it cannot be scoped to one project", () => {
    expect(checkQueryData(true)).toBeNull();
    const refused = checkQueryData(false);
    expect(refused?.code).toBe("not_allowed");
    expect(refused?.message).toContain("Let agents read threads in other projects");
  });
});

describe("a link's wait on the user", () => {
  test("stops the clock once, and only for the thread that waits", () => {
    const links = [link("a", "b", "working"), link("a", "c", "working"), link("a", "d", "done")];
    const waiting = waitStarted(links, "b", 5_000);
    expect(waiting[0]!.waitingSince).toBe(5_000);
    expect(waiting[1]!.waitingSince).toBeUndefined();
    // A second request in the same wait keeps the moment it started.
    expect(waitStarted(waiting, "b", 7_000)).toBe(waiting);
    // A settled link is not waiting on anything.
    expect(waitStarted(links, "d", 5_000)).toBe(links);
  });

  test("folds the wait it took into the time already waited, and clears it", () => {
    const waiting = [{ ...link("a", "b", "working"), waitingSince: 5_000, waitedMs: 1_000 }];
    const answered = waitEnded(waiting, "b", 8_500);
    // Its one second from before, plus the three and a half seconds of this wait.
    expect(answered[0]).toMatchObject({ waitedMs: 4_500 });
    expect(answered[0]!.waitingSince).toBeUndefined();
    // Nothing waiting: the same array, so a book that did not change is not saved.
    expect(waitEnded(answered, "b", 9_000)).toBe(answered);
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

  const repo = "/Users/me/code/rustcode";
  const main = thread("main", repo, { repo });
  const one = thread("one", "/Users/me/.config/rustcode/worktrees/rustcode/fix-a", { repo });
  const two = thread("two", "/Users/me/.config/rustcode/worktrees/rustcode/fix-b", { repo });

  test("a main checkout and its worktrees are one project", () => {
    expect(checkManage(main, one)).toBeNull();
    expect(checkManage(one, main)).toBeNull();
    // Two worktrees of one repository reach each other too.
    expect(checkManage(one, two)).toBeNull();
    expect(checkMessage(one, main, emptyBook())).toBeNull();
    expect(checkMessage(one, two, emptyBook())).toBeNull();
  });

  test("a worktree lists the threads of its repository, not the worktree's own folder", () => {
    const listed = listSessions([main, one, two], one, "").sessions as { session_id: string }[];
    expect(listed.map((s) => s.session_id)).toEqual(["main", "two"]);
  });

  test("a worktree RustCode made names its repository without a repo field", () => {
    const recorded = thread("w", "/Users/me/.config/rustcode/worktrees/rustcode/fix-c", {
      worktree: {
        path: "/Users/me/.config/rustcode/worktrees/rustcode/fix-c",
        branch: "fix/c",
        repo,
      },
    });
    expect(sameReach(main, recorded)).toBe(true);
    expect(checkManage(recorded, main)).toBeNull();
  });

  test("two repositories stay two projects", () => {
    const other = thread("other", "/Users/me/code/other", { repo: "/Users/me/code/other" });
    expect(checkManage(main, other)?.code).toBe("not_allowed");
    expect(checkMessage(main, other, emptyBook())).toBeNull();
    expect(listSessions([main, other], main, "").sessions).toEqual([]);
  });

  test("a folder that is gone falls back to its path", () => {
    // No repository was resolved for it, so only the same folder is one project.
    const gone = thread("gone", "/Users/me/gone");
    expect(checkManage(main, gone)?.code).toBe("not_allowed");
    expect(checkManage(gone, thread("gone-again", "/Users/me/gone"))).toBeNull();
    expect(checkManage(gone, two)?.code).toBe("not_allowed");
  });

  test("a thread reaches the delegate it started, wherever it runs", () => {
    const other = thread("delegate", "/Users/me/code/other", {
      repo: "/Users/me/code/other",
      parent: "a",
    });
    const a = thread("a");
    expect(checkManage(a, other)).toBeNull();
    expect(checkMessage(a, other, emptyBook())).toBeNull();
    expect(listSessions([other], a, "").sessions).toHaveLength(1);
    // Somebody else's delegate in another repository is not on offer.
    expect(checkManage(thread("b"), other)?.code).toBe("not_allowed");
  });

  test("search and worktree listings reach a delegate in another repository", () => {
    const a = thread("a");
    const delegate = thread("delegate", "/Users/me/code/other", {
      repo: "/Users/me/code/other",
      parent: "a",
      worktree: {
        path: "/Users/me/code/other",
        branch: "fix/other",
        repo: "/Users/me/code/other",
      },
    });
    // `search_sessions` and `list_worktrees` both filter with this predicate.
    expect(reachesThread(a, delegate)).toBe(true);
    expect(reachesThread(thread("b"), delegate)).toBe(false);
  });
});

describe("waking a parent", () => {
  const done = (orchestrator: string, target: string, at = 1_000, state: Link["state"] = "done"): Link => ({
    ...link(orchestrator, target, state),
    settledAt: at,
    answer: `${target} says done`,
    filesChanged: 2,
  });

  /** The wake row a busy parent's queue holds, as `sendWake` queues it. */
  const wakeRow = (id: string, wake: Wake, now: number): QueuedPrompt => ({
    id,
    text: wakeMessage(wake, now),
    wake: true,
    wakeLinks: wakeIds(wake),
    wakeAsking: askingIds(wake),
  });

  /** The book after a wake message was read: what it named as finished is heard, so is what it named as asking. */
  const read = (b: LinkBook, wake: Wake, c: WakeContext, now: number): LinkBook =>
    markDelivered(b, { settled: wake.settled.map((l) => l.id), asking: askingIds(wake) }, c, now);

  const ctx = (over: Partial<WakeContext> = {}): WakeContext => ({
    states: {},
    pending: {},
    waiting: {},
    threads: { a: { title: "Parent a", running: false } },
    now: 10_000,
    ...over,
  });

  test("a child that shows no progress is reported once per stall", () => {
    const at = 1_000;
    const child = { ...link("a", "b", "working"), startedAt: at };
    const stallCtx = (now: number, over: Partial<WakeContext> = {}): WakeContext =>
      ctx({
        now,
        threads: {
          a: { title: "Parent a", running: false },
          b: { title: "Thread b", running: true },
        },
        activity: {
          b: { lastAt: at, lastTool: "bash", lastCommand: "sleep 600", turns: 0 },
        },
        ...over,
      });
    const due = at + STALL_NOTICE_MS;
    // Just inside the window: nothing yet, but the next look is at the threshold.
    expect(wakesDue(book([child]), stallCtx(due - 1))).toEqual([]);
    expect(nextWakeAt(book([child]), stallCtx(due - 1))).toBe(due);
    const [wake] = wakesDue(book([child]), stallCtx(due));
    expect(wake!.stalled?.map((n) => n.link.target)).toEqual(["b"]);
    expect(wakeSummary(wake!)).toBe("Delegate stalled: b");
    // A child waiting on the user is not stuck, however long the answer takes.
    expect(
      wakesDue(book([child]), stallCtx(due, { states: { "a/b": "needs_user" } })).flatMap(
        (w) => w.stalled ?? [],
      ),
    ).toEqual([]);
    const text = wakeMessage(wake!, due);
    expect(text).toContain("has made no progress for 10m");
    expect(text).toContain("last tool: bash: sleep 600");
    expect(text).toContain("interrupt_session it or message it");
    // Sent: the same silence does not wake the parent again.
    const heard = updateLink(book([child]), "a/b", { stallNotifiedFor: at });
    expect(wakesDue(heard, stallCtx(due + 60_000))).toEqual([]);
    // New activity arms another notice, only once that has been quiet for ten minutes too.
    const resumed = stallCtx(due + 60_000, {
      activity: { b: { lastAt: due, lastTool: "bash", turns: 0 } },
    });
    expect(wakesDue(heard, resumed)).toEqual([]);
    const again = wakesDue(heard, { ...resumed, now: due + STALL_NOTICE_MS, activity: resumed.activity });
    expect(again[0]!.stalled?.map((n) => n.link.target)).toEqual(["b"]);
  });

  test("a stall notice survives a wake folded around a finished sibling", () => {
    const at = 1_000;
    const child = { ...link("a", "b", "working"), startedAt: at };
    const sibling = { ...done("a", "c"), settledAt: at + STALL_NOTICE_MS };
    const now = at + STALL_NOTICE_MS;
    const c = ctx({
      now,
      threads: {
        a: { title: "Parent a", running: false },
        b: { title: "Thread b", running: true },
      },
      activity: { b: { lastAt: at, lastTool: "bash", turns: 0 } },
    });
    const both = book([child, sibling]);
    const stall = wakesDue(both, c)[0]!;
    // A settled sibling arrives while the stall row waits: the rebuilt row keeps both lines.
    const merged = wakeFor("a", "Parent a", [...wakeIds(stall), "a/c"], both, c)!;
    expect(wakeIds(merged)).toEqual(["a/c", "a/b"]);
    expect(wakeMessage(merged, now)).toContain("no progress");
  });

  test("coalesces parallel children into one message, once", () => {
    const two = book([done("a", "b"), done("a", "c")]);
    const wakes = wakesDue(two, ctx());
    expect(wakes).toHaveLength(1);
    expect(wakes[0]!.settled.map((l) => l.target)).toEqual(["b", "c"]);
    // Delivered: the same links are not reported again, in this run or after a reload.
    const after = read(two, wakes[0]!, ctx(), 10_000);
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

  test("a mark a dropped call leaves behind is taken back, and a newer one stands", () => {
    const heard = (id: string, at: number): Link => ({ ...done("a", id), notifiedAt: at });
    const b = book([heard("b", 1_000), heard("c", 1_000)]);
    // The wait that handed both over was dropped before its answer was read.
    const back = unheard(b, [
      { id: "a/b", notifiedAt: 1_000 },
      { id: "a/c", notifiedAt: 1_000 },
    ]);
    expect(back.links.map((l) => l.notifiedAt)).toEqual([undefined, undefined]);
    // They are due again: the answer nobody read comes as the ordinary wake.
    expect(wakesDue(back, ctx())[0]!.settled.map((l) => l.target)).toEqual(["b", "c"]);
    // A mark another delivery made since stands: nothing arrives twice.
    const newer = book([heard("b", 5_000)]);
    expect(unheard(newer, [{ id: "a/b", notifiedAt: 1_000 }])).toBe(newer);
    // A question mark is taken back the same way.
    const asked = book([{ ...done("a", "b", 1_000, "needs_user"), askedRequest: "7" }]);
    expect(unheard(asked, [{ id: "a/b", askedRequest: "7" }]).links[0]!.askedRequest).toBeUndefined();
    expect(unheard(asked, [{ id: "a/b", askedRequest: "6" }])).toBe(asked);
    // Nothing listed: the same book, unchanged.
    expect(unheard(b, [])).toBe(b);
  });

  test("a queue read back after a restart only runs what the parent has not heard", () => {
    const b = book([done("a", "b"), done("a", "c")]);
    const wake = wakesDue(b, ctx())[0]!;
    const row = { links: wakeIds(wake), asking: askingIds(wake) };
    // Nothing was delivered before the app died: the row still has something to say.
    expect(wakeStillOwed(b, row, ctx())).toBe(true);
    // Its turn started (the marks are written) and the queue was not written back: nothing is left
    // to run, so a restart cannot report the same finishes twice.
    const heard = read(b, wake, ctx(), 10_000);
    expect(wakeStillOwed(heard, row, ctx())).toBe(false);
    // A row that names no links is not something to start a turn for.
    expect(wakeStillOwed(b, { links: [], asking: [] }, ctx())).toBe(false);
  });

  test("a question is owed until the request the row names is answered", () => {
    const asking = book([done("a", "b", 1_000, "needs_user")]);
    const pending = ctx({ pending: { "a/b": "42" } });
    const row = { links: ["a/b"], asking: ["a/b"] };
    expect(wakeStillOwed(asking, row, pending)).toBe(true);
    // Told already: the same request is not something to say again after a restart.
    const told = book([{ ...done("a", "b", 1_000, "needs_user"), askedRequest: "42" }]);
    expect(wakeStillOwed(told, row, pending)).toBe(false);
    // The child finished since: its finish comes as the ordinary wake, not from this row.
    const finished = book([done("a", "b")]);
    expect(wakeStillOwed(finished, row, ctx())).toBe(false);
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
    const heard = read(chain, wakes[0]!, ctx({ threads }), 10_000);
    expect(heard.links.find((l) => l.id === "c/a")!.state).toBe("working");
    expect(heard.links.find((l) => l.id === "a/b")!.notifiedAt).toBe(10_000);
    // a's own turn ends: its parent hears about a, once.
    const settledA = updateLink(heard, "c/a", { state: "done", settledAt: 9_500 });
    const up = wakesDue(settledA, ctx({ threads }));
    expect(up.map((w) => w.parent)).toEqual(["c"]);
    expect(up[0]!.settled.map((l) => l.target)).toEqual(["a"]);
    expect(wakesDue(read(settledA, up[0]!, ctx({ threads }), 20_000), ctx({ threads, now: 30_000 }))).toEqual(
      [],
    );
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
    const after = read(asking, next[0]!, waiting("8"), 10_000);
    expect(after.links[0]!.askedRequest).toBe("8");
    expect(wakesDue(after, waiting("8"))).toEqual([]);
    // A question the parent is waiting on is not reported either.
    expect(wakesDue(asking, { ...waiting("8"), waiting: { a: ["a/b"] } })).toEqual([]);
  });

  test("the message says what finished, what waits, and not to poll", () => {
    const b = book([done("a", "b"), link("a", "c", "working")]);
    const wake = wakesDue(b, ctx({ states: { "a/c": "needs_user" }, pending: { "a/c": "3" } }))[0]!;
    const text = wakeMessage(wake, 10_000);
    expect(text.split("\n")[0]).toBe("Delegate finished: b · Delegate asks: c");
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
    // A delegate's line names its harness and how long it worked; a link without timing omits it
    // rather than showing 0.
    const timed: Link = { ...done("a", "b"), harness: "pi", harnessName: "Pi", workedMs: 229_000 };
    const rich = wakeMessage({ parent: "a", parentTitle: "Parent a", settled: [timed], asking: [] }, 10_000);
    expect(rich).toContain("- link_id a/b · done · “b” · Pi · worked 3m 49s · session b");
    expect(text).not.toContain("worked");
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
    const after = read(b, wake, ctx(), 10_000);
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
    const told = read(leafDone, forMiddle[0]!, chainCtx({ now: 9_600 }), 9_600);
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
    const after = read(answered, up[0]!, chainCtx({ now: 11_000 }), 11_000);
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
    const marked = read(leaves, told, ctx({ threads, now: 9_900 }), 9_900);
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
    const after = read(woken, up[0]!, chainCtx({ now: 10_200 }), 10_200);
    // Another wake turn changes nothing more: no loop, no third message.
    const again = restateLinks(after, "middle", said("and again"), 11_000, true);
    expect(again.links[0]!.notifiedAt).toBe(10_200);
    expect(wakesDue(again, chainCtx({ now: 12_000 }))).toEqual([]);
  });

  test("a re-armed link forgets the time of the turn it already reported", () => {
    const reported = book([
      {
        ...done("top", "middle", 9_000),
        notifiedAt: 9_100,
        startedAt: 8_000,
        workedMs: 1_000,
      },
    ]);
    const woken = restateLinks(reported, "middle", said("the real answer"), 10_000, true);
    // The next notice reports a 30s answer, not the 12m that included the turn before it.
    expect(woken.links[0]!.startedAt).toBeUndefined();
    expect(woken.links[0]!.workedMs).toBeUndefined();
    expect(woken.links[0]!.notifiedAt).toBeUndefined();
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

  test("a queued wake takes a newer one in, so one turn reports both", () => {
    const first = book([done("a", "b")]);
    const queued = wakesDue(first, ctx())[0]!;
    const ids = wakeIds(queued);
    expect(ids).toEqual(["a/b"]);
    // A second child finishes while the parent's turn still runs: the wake waits in its queue.
    const both = book([done("a", "b", 1_000), done("a", "c", 9_500)]);
    const second = wakesDue(both, ctx())[0]!;
    expect(wakeIds(second)).toEqual(["a/b", "a/c"]);
    // The older wait still holds only b: the row adds what it names once.
    const wake = wakeFor("a", "Parent a", [...ids, ...wakeIds(second)], both, ctx())!;
    expect(wakeIds(wake)).toEqual(["a/b", "a/c"]);
    const text = wakeMessage(wake, 10_000);
    // One summary for the two of them, not a line that says "Delegate finished" twice.
    expect(text.split("\n")[0]).toBe("2 delegates finished");
    expect(text).toContain("link_id a/b");
    expect(text).toContain("link_id a/c");
    expect(text).toContain("b says done");
    expect(text).toContain("c says done");
    // The folded wake is still the marked wake message a replayed turn reads as a wake.
    expect(isWakeMessage(text)).toBe(true);
  });

  test("the same link reported twice counts once", () => {
    // A reported link re-armed by its target's wake turn (restateLinks) is the same child, once.
    const b = book([{ ...done("a", "b"), answerChangedAt: 9_000 }]);
    const wake = wakeFor("a", "Parent a", ["a/b", "a/b"], b, ctx())!;
    expect(wake.settled.map((l) => l.id)).toEqual(["a/b"]);
    expect(wakeSummary(wake)).toBe("Delegate finished: b");
  });

  test("a child that asked and then finished reads once, as finished", () => {
    const asked = book([{ ...link("a", "b", "working"), askedRequest: "7" }]);
    const asking = wakesDue(asked, ctx({ states: { "a/b": "needs_user" }, pending: { "a/b": "7" } }));
    expect(asking).toEqual([]);
    const newAsk = ctx({ states: { "a/b": "needs_user" }, pending: { "a/b": "8" } });
    const queued = wakesDue(asked, newAsk)[0]!;
    expect(wakeIds(queued)).toEqual(["a/b"]);
    // It answered the question and finished before the parent's turn ended.
    const settled = book([{ ...done("a", "b", 9_500), askedRequest: "8" }]);
    const wake = wakeFor("a", "Parent a", wakeIds(queued), settled, ctx({ now: 10_000 }))!;
    expect(wake.asking).toEqual([]);
    expect(wake.settled.map((l) => l.id)).toEqual(["a/b"]);
    expect(wakeSummary(wake)).toBe("Delegate finished: b");
  });

  test("a queued wake is one row, not a queue of them", () => {
    // The parent's turn still runs: b's result waits behind the message the user queued first.
    const queued = book([done("a", "b")]);
    const waiting = wakeRow("q-wake", wakesDue(queued, ctx())[0]!, 10_000);
    const user: QueuedPrompt = { id: "q-user", text: "and then run the checks" };
    const both = book([done("a", "b"), done("a", "c", 9_500)]);
    const queue = queueWake([user, waiting], wakesDue(both, ctx())[0]!, both, ctx(), 10_000, "q-new");
    // Same row, same place in the queue: the parent runs one wake turn, not two.
    expect(queue.map((q) => q.id)).toEqual(["q-user", "q-wake"]);
    const folded = queue[1]!;
    expect(folded.wake).toBe(true);
    expect(folded.wakeLinks).toEqual(["a/b", "a/c"]);
    expect(wakeLine(folded.text)).toBe("2 delegates finished");
    expect(folded.text).toContain("b says done");
    expect(folded.text).toContain("c says done");
    expect(isWakeMessage(folded.text)).toBe(true);
    // The user's message is untouched, and the queue is not reordered around the wake.
    expect(queue[0]).toBe(user);
    // A third result folds into the same row again: the queue never grows a second wake.
    const three = book([done("a", "b"), done("a", "c", 9_500), done("a", "d", 9_900)]);
    const again = queueWake(queue, wakesDue(three, ctx())[0]!, three, ctx(), 10_000, "q-newer");
    expect(again).toHaveLength(2);
    expect(wakeLine(again[1]!.text)).toBe("3 delegates finished");
    // Nothing left to report (both were stopped since): the row it has stands rather than emptying.
    const stopped = book([link("a", "b", "cancelled"), link("a", "c", "cancelled")]);
    const nothing = { parent: "a", parentTitle: "Parent a", settled: [], asking: [] };
    expect(foldWake(folded, nothing, stopped, ctx(), 10_000)).toBe(folded);
  });

  test("a rebuild keeps what wakesDue gives up on, and never restates it", () => {
    // A is blocked on b in wait_session: that call returns b's answer, so the wake leaves it out.
    const b = done("a", "b");
    expect(wakeFor("a", "Parent a", ["a/b"], book([b]), ctx({ waiting: { a: ["a/b"] } }))).toBeNull();
    // The parent messaged that thread again: the older answer is out of date, told or not.
    const newer = { ...link("a", "b", "working"), id: "a/2", createdAt: 9_000 };
    const first: Link = { ...done("a", "b"), id: "a/1", createdAt: 1_000, notifiedAt: 2_000 };
    const both = book([first, newer]);
    expect(wakeFor("a", "Parent a", ["a/1"], both, ctx())).toBeNull();
    // Its own subtree still works: a link nobody was told about yet is not finished for the parent.
    const started = book([
      { ...done("a", "middle", 9_000), notifiedAt: undefined },
      link("middle", "leaf", "working"),
    ]);
    expect(wakeFor("a", "Parent a", ["a/middle"], started, ctx({ now: 9_500 }))).toBeNull();
    // But a notice this row already promised keeps its line: nothing else would deliver it.
    const promised = book([
      { ...done("a", "middle", 9_000), notifiedAt: 9_100 },
      link("middle", "leaf", "working"),
    ]);
    const kept = wakeFor("a", "Parent a", ["a/middle"], promised, ctx({ now: 9_500 }))!;
    expect(kept.settled.map((l) => l.id)).toEqual(["a/middle"]);
    expect(wakeSummary(kept)).toBe("Delegate finished: middle");
  });

  test("a report the book has pruned is never overwritten", () => {
    // The row reports a/old, which `prune` has since dropped from the book.
    const carried = wakeRow(
      "q-wake",
      { parent: "a", parentTitle: "Parent a", settled: [], asking: [] },
      10_000,
    );
    const row: QueuedPrompt = {
      ...carried,
      text: withWakeMark("1 child finished\n\nFinished:\n- link_id a/old · done"),
      wakeLinks: ["a/old"],
    };
    const now = book([done("a", "b")]);
    const wake = wakesDue(now, ctx())[0]!;
    // It cannot be rebuilt faithfully, so it is left whole: no text may lose the old report.
    expect(foldWake(row, wake, now, ctx(), 10_000)).toBeNull();
    // The new wake is queued beside it instead, so that result is not lost either.
    const queue = queueWake([row], wake, now, ctx(), 10_000, "q-new");
    expect(queue.map((q) => q.id)).toEqual(["q-wake", "q-new"]);
    expect(queue[0]).toBe(row);
    expect(queue[0]!.text).toContain("link_id a/old");
    expect(queue[1]!.wakeLinks).toEqual(["a/b"]);
    expect(wakeLine(queue[1]!.text)).toBe("Delegate finished: b");
    // A row whose links are all known folds again, so the extra row is not permanent.
    const both = book([done("a", "b"), done("a", "c", 9_500)]);
    const folded = queueWake(queue, wakesDue(both, ctx())[0]!, both, ctx(), 10_000, "q-newer");
    expect(folded.map((q) => q.id)).toEqual(["q-wake", "q-new"]);
    expect(folded[0]).toBe(row);
    expect(folded[1]!.wakeLinks).toEqual(["a/b", "a/c"]);
  });

  test("a stopped successor does not retire the answer nobody read", () => {
    const first: Link = { ...done("a", "b", 1_000), id: "a/1", createdAt: 1_000 };
    const stopped: Link = { ...link("a", "b", "cancelled"), id: "a/2", createdAt: 4_000 };
    // The follow-up never answered: the first answer is still the last real one.
    expect(supersededLinks(book([first, stopped]), ctx())).toEqual([]);
    expect(wakesDue(book([first, stopped]), ctx())).toHaveLength(1);
    const promised = book([{ ...first, notifiedAt: 2_000 }, stopped]);
    expect(wakeFor("a", "Parent a", ["a/1"], promised, ctx())?.settled.map((l) => l.id)).toEqual(["a/1"]);
    // A successor that works still replaces it: the newer turn's answer is the one needed.
    const working = book([first, { ...link("a", "b", "working"), id: "a/2", createdAt: 4_000 }]);
    expect(supersededLinks(working, ctx()).map((l) => l.id)).toEqual(["a/1"]);
  });

  test("a replaced result is not owed, and is owed again once its successor is stopped", () => {
    const first: Link = { ...done("a", "middle", 9_000), createdAt: 1_000 };
    const live: Link = { ...link("a", "middle", "working"), id: "a/2", createdAt: 2_000 };
    // The newer answer is the one owed, so nothing is pending for the old one — and nothing is
    // written down for it either, which is what lets it come back:
    expect(pendingWakeUnder(book([first, live]), ctx(), "a")).toBe(false);
    // The successor was stopped by the user, or interrupted by a restart, without answering.
    const stopped = book([first, { ...live, state: "interrupted" }]);
    expect(pendingWakeUnder(stopped, ctx(), "a")).toBe(true);
    expect(wakesDue(stopped, ctx({ now: 10_000 }))[0]!.settled.map((l) => l.id)).toEqual(["a/middle"]);
  });

  test("a wake row that names no links is never rewritten", () => {
    const b = book([done("a", "b")]);
    const wake = wakesDue(b, ctx())[0]!;
    // A row written before it kept its links (or by a preview): what it reports cannot be read back.
    const old: QueuedPrompt = { id: "q-old", text: withWakeMark("1 child finished"), wake: true };
    expect(foldWake(old, wake, b, ctx(), 10_000)).toBeNull();
    const queue = queueWake([old], wake, b, ctx(), 10_000, "q-new");
    expect(queue.map((q) => q.id)).toEqual(["q-old", "q-new"]);
    expect(queue[0]).toBe(old);
    expect(queue[1]!.wakeLinks).toEqual(["a/b"]);
  });

  test("a queued row reports its links once: its turn marks them, and nothing else is due", () => {
    const b = book([done("a", "b")]);
    const wake = wakesDue(b, ctx())[0]!;
    const queue = queueWake([], wake, b, ctx(), 10_000, "q-new");
    const carried = { a: queue.flatMap((q) => q.wakeLinks ?? []) };
    expect(carried.a).toEqual(["a/b"]);
    // While the row waits, its links are the row's: no second wake, no timer for one.
    expect(wakesDue(b, ctx({ queued: carried }))).toEqual([]);
    expect(nextWakeAt(b, ctx({ queued: carried }))).toBeNull();
    // Its turn starts: the mark is what keeps it quiet afterwards.
    const marked = read(b, wake, ctx(), 10_000);
    expect(wakesDue(marked, ctx({ now: 20_000 }))).toEqual([]);
    // A window that lost its queue (a restart) leaves the book unmarked, so the wake is due again.
    expect(wakesDue(b, ctx({ now: 20_000 }))).toHaveLength(1);
  });

  test("delivery marks what the row said, and only that", () => {
    const b = book([done("a", "b"), { ...link("a", "c", "working") }]);
    const asking = ctx({ states: { "a/c": "needs_user" }, pending: { "a/c": "9" }, now: 10_000 });
    const reported = { settled: ["a/b"], asking: ["a/c"] };
    const marked = markDelivered(b, reported, asking, 10_000);
    expect(marked.links[0]!.notifiedAt).toBe(10_000);
    // A question is marked with the request it was about, not as a finished answer.
    expect(marked.links[1]!.askedRequest).toBe("9");
    expect(marked.links[1]!.notifiedAt).toBeUndefined();
    // Delivering the same row again marks nothing new, so nothing is reported twice.
    expect(markDelivered(marked, reported, asking, 11_000)).toBe(marked);
    // An id the row never named is not marked at all, whatever its state.
    expect(markDelivered(b, { settled: [], asking: [] }, asking, 10_000)).toBe(b);
    // A row that named a finish about a child still running marks nothing: it settles on its own.
    const running = book([link("a", "d", "working")]);
    expect(markDelivered(running, { settled: ["a/d"], asking: [] }, ctx(), 10_000)).toBe(running);
  });

  test("a row that said a child waits keeps its finish owed once the child answers", () => {
    // The row was queued as "Delegate asks": the child asked, and answered before the row ran.
    const askingCtx = ctx({ states: { "a/c": "needs_user" }, pending: { "a/c": "9" }, now: 10_000 });
    const asked = book([link("a", "c", "working")]);
    const notice = wakesDue(asked, askingCtx)[0]!;
    expect(wakeIds(notice)).toEqual(["a/c"]);
    expect(wakeSummary(notice)).toBe("Delegate asks: c");
    const row = wakeRow("q-wake", notice, 10_000);
    expect(row.wakeLinks).toEqual(["a/c"]);
    expect(row.wakeAsking).toEqual(["a/c"]);
    // It finished before the row's turn came: delivery promised a question, so nothing is marked.
    const finished = book([done("a", "c", 9_500)]);
    const delivered = markDelivered(finished, { settled: [], asking: ["a/c"] }, ctx(), 11_000);
    expect(delivered).toBe(finished);
    // The finish is owed, and comes as its own wake; reading that is what marks it.
    const finish = wakesDue(delivered, ctx({ now: 12_000 }))[0]!;
    expect(finish.settled.map((l) => l.id)).toEqual(["a/c"]);
    expect(wakeMessage(finish, 12_000)).toContain("c says done");
    const after = read(delivered, finish, ctx(), 12_000);
    expect(after.links[0]!.notifiedAt).toBe(12_000);
    expect(wakesDue(after, ctx({ now: 20_000 }))).toEqual([]);
  });

  test("a row that said a child finished does not silence its question", () => {
    // The reverse: the row said "1 child finished", and the child asks something since.
    const asked = book([link("a", "b", "working")]);
    const shown = ctx({ states: { "a/b": "needs_user" }, pending: { "a/b": "4" }, now: 10_000 });
    const delivered = markDelivered(asked, { settled: ["a/b"], asking: [] }, shown, 10_000);
    expect(delivered).toBe(asked);
    expect(wakesDue(delivered, shown)[0]!.asking.map((l) => l.id)).toEqual(["a/b"]);
  });

  test("every link a wake reports is on a row by the time it can be marked", () => {
    // The mark comes after the queue is written (host.ts `sendWake`), so each reported link has a row
    // that names it: folded into a waiting wake, queued alone, or queued beside a pruned one.
    const two = book([done("a", "b"), done("a", "c", 9_500)]);
    const wake = wakesDue(two, ctx())[0]!;
    const foldable = wakeRow("q-wake", wakesDue(book([done("a", "b")]), ctx())[0]!, 10_000);
    // A row whose own report was pruned away cannot be rebuilt: it stays and the new wake is added.
    const pruned: QueuedPrompt = { ...foldable, wakeLinks: ["a/gone"] };
    for (const queue of [[], [foldable], [pruned]] as QueuedPrompt[][]) {
      const rows = queueWake(queue, wake, two, ctx(), 10_000, "q-new");
      const carried = new Set(rows.filter((q) => q.wake).flatMap((q) => q.wakeLinks ?? []));
      expect(wakeIds(wake).every((id) => carried.has(id))).toBe(true);
    }
  });

  test("a wake with no row waiting is queued on its own, after the user's messages", () => {
    const user: QueuedPrompt = { id: "q-user", text: "next turn, please" };
    const b = book([done("a", "b")]);
    const queue = queueWake([user], wakesDue(b, ctx())[0]!, b, ctx(), 10_000, "q-new");
    expect(queue.map((q) => q.id)).toEqual(["q-user", "q-new"]);
    expect(queue[1]).toMatchObject({ wake: true, wakeLinks: ["a/b"] });
    expect(queue[1]!.link).toBeUndefined();
    expect(isWakeMessage(queue[1]!.text)).toBe(true);
  });

  test("a link stopped since has nothing left to say, and the rest stays", () => {
    const b = book([done("a", "b"), link("a", "c", "working")]);
    const queued = wakesDue(b, ctx())[0]!;
    const stopped = book([done("a", "b"), link("a", "c", "cancelled")]);
    const wake = wakeFor("a", "Parent a", [...wakeIds(queued), "a/c"], stopped, ctx())!;
    expect(wakeIds(wake)).toEqual(["a/b"]);
    // Nothing left to report at all: no empty wake message.
    const gone = book([link("a", "b", "cancelled")]);
    expect(wakeFor("a", "Parent a", ["a/b"], gone, ctx())).toBeNull();
    // Another parent's link never rides along, whatever ids are handed in.
    const elsewhere = book([{ ...done("z", "q"), id: "a/b" }]);
    expect(wakeFor("a", "Parent a", ["a/b"], elsewhere, ctx())).toBeNull();
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

  test("a link a restart severed still has its strip place while its thread works", () => {
    // The book says interrupted, the thread says running: the row that reads live is the live one.
    const severed = { ...link("a", "b", "interrupted"), started: true };
    expect(stripLinks([severed], new Set(["b"]))).toEqual([]);
    expect(stripLinks([severed], new Set(["b"]), ["working"])).toEqual([severed]);
  });

  test("the done list is one row per thread, newest first, and a count for the rest", () => {
    const links = [
      { ...link("a", "older", "done"), started: true, settledAt: 100 },
      { ...link("a", "newer", "done"), started: true, settledAt: 300 },
      { ...link("a", "newer", "working"), started: true },
      { ...link("a", "refused", "failed"), started: true, settledAt: 200 },
      { ...link("a", "busy", "working"), started: true },
      link("other", "elsewhere", "done"),
    ];
    // One row for `newer`, at its latest moment; the live ones and another parent are not done.
    expect(doneLinks(links, "a").shown.map((l) => [l.target, l.state])).toEqual([
      ["newer", "done"],
      ["refused", "failed"],
      ["older", "done"],
    ]);
    const capped = doneLinks(links, "a", 2);
    expect(capped.shown.map((l) => l.target)).toEqual(["newer", "refused"]);
    expect(capped.more).toBe(1);
    expect(doneLinks(links, "a", 0)).toEqual({ shown: [], more: 3 });
  });
});

describe("one row per delegate thread", () => {
  const messages = (target: string, state: Link["state"], createdAt: number, id: string): Link => ({
    ...link("a", target, state),
    id,
    createdAt,
  });

  test("several links to one session are one row, and the other threads keep theirs", () => {
    const rows = linkThreads([
      messages("b", "done", 100, "a/1"),
      messages("c", "working", 200, "a/2"),
      messages("b", "working", 300, "a/3"),
    ]);
    expect(rows.map((row) => [row.target, row.state, row.link.id, row.links.length])).toEqual([
      ["b", "working", "a/3", 2],
      ["c", "working", "a/2", 1],
    ]);
  });

  test("a live message speaks for its thread, so a settled one never reads as working", () => {
    const settled = messages("b", "done", 100, "a/1");
    const running = messages("b", "working", 200, "a/2");
    const rows = linkThreads([settled, running]);
    expect(rows).toHaveLength(1);
    expect(rows[0]!.state).toBe("working");
    expect(rows[0]!.link.id).toBe("a/2");

    // The shown state can differ from the record: a restart's `interrupted` link whose thread runs.
    const [shown] = linkThreads([{ ...settled, state: "interrupted" }], ["working"]);
    expect(shown!.state).toBe("working");
  });

  test("a session idle after answering takes its newest answer and that link's time", () => {
    const older = messages("b", "done", 100, "a/1");
    const newer = messages("b", "done", 300, "a/2");
    const [row] = linkThreads([newer, older]);
    expect(row!.state).toBe("done");
    expect(row!.link.id).toBe("a/2");
    expect(row!.link.createdAt).toBe(300);
    expect(row!.links).toHaveLength(2);
  });

  test("a session waiting on the user is one row with its latest state", () => {
    const rows = linkThreads([messages("b", "done", 100, "a/1"), messages("b", "needs_user", 200, "a/2")]);
    expect(rows).toHaveLength(1);
    expect(rows[0]!.state).toBe("needs_user");
    expect(rows[0]!.link.id).toBe("a/2");
  });
});

describe("answering with the slots and the child's activity", () => {
  test("a busy refusal says which limit and how many are left", () => {
    const full = book(Array.from({ length: DEFAULT_DELEGATES_PER_THREAD }, (_, i) => link("a", `t${i}`)));
    const refused = checkStart(thread("a"), full, 2)!;
    expect(refused.code).toBe("busy");
    expect(refused.linksLeft).toBe(0);
    expect(refused.message).toContain(`at most ${DEFAULT_DELEGATES_PER_THREAD} delegates per thread at once`);
    expect(refused.message).toContain("this thread has 8");
    const json = failureJson(refused);
    expect(json.error).toMatchObject({ code: "busy", links_left: 0 });
    // One slot open: a single start fits, two do not.
    const room = book(Array.from({ length: DEFAULT_DELEGATES_PER_THREAD - 1 }, (_, i) => link("a", `t${i}`)));
    expect(checkStart(thread("a"), room, 1)).toBeNull();
    expect(checkStart(thread("a"), room, 2)?.linksLeft).toBe(1);
    // A thread limit of two says two; the machine-wide total names its own count.
    const two = book([link("a", "b"), link("a", "c")]);
    const small = checkStart(thread("a"), two, 1, { perThread: 2, total: 16 })!;
    expect(small.message).toContain("at most 2 delegates per thread at once");
    const tiny = book([link("a", "b"), link("a", "c"), link("other", "d"), link("other", "e")]);
    const total = checkStart(thread("a"), tiny, 1, { perThread: 8, total: 4 })!;
    expect(total.message).toContain("at most 4 delegates at once across this app");
    expect(total.message).toContain("4 are running");
    // links_left is the smaller of the two rooms, whichever cap refused, and the
    // refusal says that number rather than the per-thread room it did not use.
    expect(linksLeft(two, "a", { perThread: 4, total: 2 })).toBe(0);
    const capped = checkStart(thread("a"), two, 3, { perThread: 4, total: 2 })!;
    expect(capped.message).toContain("at most 4 delegates per thread at once");
    expect(capped.message).toContain("0 slots are left in all");
    expect(capped.message).not.toContain("2 slots are left");
    expect(capped.linksLeft).toBe(0);
  });

  test("a per-thread setting above the total is capped by it", () => {
    expect(effectivePerThreadLimit({ perThread: 32, total: 16 })).toBe(16);
    expect(effectivePerThreadLimit({ perThread: 8, total: 16 })).toBe(8);
    expect(effectivePerThreadLimit({ perThread: 4, total: 4 })).toBe(4);
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
    // A tool that names its command keeps it, and a long silence says the child looks stalled.
    const running = activityOf([
      { kind: "tool", title: "bash", toolName: "bash", input: { command: "sleep 600" }, startedAt: 12_000 },
    ]);
    expect(running).toMatchObject({ lastTool: "bash", lastCommand: "sleep 600" });
    const stalled = linkResult(link("a", "b", "working"), "working", undefined, {
      now: 12_000 + STALL_NOTICE_MS,
      activity: running,
    });
    expect(stalled).toMatchObject({ stalled: true, idle_ms: STALL_NOTICE_MS });
  });

  test("a cut answer says so and names the thread whose read_session has the rest", () => {
    const cut = { ...link("a", "b", "done"), answer: "x", truncated: true };
    const entry = linkResult(cut, "done");
    expect(entry).toMatchObject({ answer_truncated: true, session_id: "b", link_id: "a/b" });
    // An answer that fits says nothing about truncation.
    expect(linkResult({ ...cut, truncated: false }, "done")).not.toHaveProperty("answer_truncated");
  });
});

describe("a delegate whose machine went away mid-turn", () => {
  test("its link reaches wait_session as done or failed, with that turn's answer", () => {
    // The settled link is what the parent's row and `wait_session` read (state/store.ts
    // `lostTurn` settles it once the machine says how the thread ended).
    const done = linkResult(
      { ...link("a", "b", "working"), state: "done", answer: "All 12 tests pass." },
      "done",
    );
    expect(done).toMatchObject({ status: "done", link_id: "a/b", answer: "All 12 tests pass." });
    expect(done).not.toHaveProperty("note");
    // A machine that never came back is the failure, and the row previews that sentence.
    expect(MACHINE_LOST_TURN).toBe("The machine disconnected during this turn.");
    const failed = linkResult(
      { ...link("a", "b", "working"), state: "failed", error: MACHINE_LOST_TURN },
      "failed",
    );
    expect(failed).toMatchObject({ status: "failed" });
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

describe("a message queued on a thread whose turn failed", () => {
  test("fails with the reason instead of staying Queued, and nothing else changes", () => {
    const queued = link("parent", "child", "queued");
    const other = { ...link("parent", "elsewhere", "queued"), id: "parent/2" };
    const working = { ...link("parent", "child", "working"), id: "parent/3" };
    const { book: next, failed } = failQueuedLinks(
      book([queued, other, working]),
      "child",
      "Codex rejected the request (HTTP 400 Bad Request).",
      1000,
    );
    expect(failed).toEqual([queued.id]);
    const after = next.links.find((l) => l.id === queued.id)!;
    expect(after.state).toBe("failed");
    expect(after.settledAt).toBe(1000);
    expect(after.answer).toContain("Not delivered");
    expect(after.answer).toContain("HTTP 400 Bad Request");
    expect(after.answer).toContain("message the thread again");
    expect(next.links.find((l) => l.id === other.id)!.state).toBe("queued");
    expect(next.links.find((l) => l.id === working.id)!.state).toBe("working");
  });

  test("a thread with nothing queued keeps the same book", () => {
    const start = book([link("parent", "child", "working")]);
    const { book: next, failed } = failQueuedLinks(start, "child", undefined, 1000);
    expect(failed).toEqual([]);
    expect(next).toBe(start);
  });
});

describe("keeping an orchestrator's context small", () => {
  test("asks a new delegate for a short report after the attributed task", () => {
    const text = delegated("Fix the login", "Parent");
    expect(text.startsWith(delivered("Fix the login", "Parent"))).toBe(true);
    expect(text.endsWith(DELEGATE_REPORT)).toBe(true);
  });

  test("notes the context once per quarter it passes", () => {
    expect(contextNote(undefined, 0)).toBeNull();
    expect(contextNote({ used: 10_000, size: 0 }, 0)).toBeNull();
    expect(contextNote({ used: 40_000, size: 200_000 }, 0)).toBeNull();
    const first = contextNote({ used: 54_000, size: 200_000 }, 0);
    expect(first?.step).toBe(1);
    expect(first?.note).toContain("27% full (54k of 200k tokens)");
    expect(contextNote({ used: 90_000, size: 200_000 }, 1)).toBeNull();
    expect(contextNote({ used: 110_000, size: 200_000 }, 1)?.step).toBe(2);
  });

  test("waits twenty minutes when no timeout is given", () => {
    const call = parseCall({ tool: "wait_session", args: { link_id: "a/1" } });
    expect(call).toMatchObject({ tool: "wait_session", timeoutMs: 20 * 60 * 1000 });
  });
});

describe("a thread that carries on after a usage limit", () => {
  const stopped = (id: string, over: Partial<Link> = {}): Link => ({
    ...link("parent", "child", "failed"),
    id,
    createdAt: 1,
    answer: "You've hit your 5-hour limit · resets 2:40am (Europe/Oslo)",
    error: `Internal error: You've hit your 5-hour limit · resets 2:40am (Europe/Oslo): {"errorKind":"rate_limit"}`,
    settledAt: 2,
    notifiedAt: 3,
    workedMs: 500,
    ...over,
  });

  test("the failed link works again and the parent is owed the new answer", () => {
    const after = reopenLinks(book([stopped("parent/1")]), "child", ["parent/1"], 9_000);
    const reopened = after.links[0]!;
    expect(reopened.state).toBe("working");
    expect(reopened.startedAt).toBe(9_000);
    expect(reopened.answer).toBeUndefined();
    expect(reopened.error).toBeUndefined();
    expect(reopened.settledAt).toBeUndefined();
    // The parent heard the stop once: it is told again only when the thread settles for good.
    expect(reopened.notifiedAt).toBe(3);
    expect(reopened.events?.at(-1)?.kind).toBe("working");
  });

  test("a link whose parent was archived or stopped is not reopened", () => {
    const base = book([stopped("parent/1"), stopped("other/1", { orchestrator: "other", createdAt: 2 })]);
    const after = reopenLinks(base, "child", ["parent/1", "other/1"], 9_000, (o) => o === "parent");
    expect(after.links.map((l) => l.state)).toEqual(["failed", "working"]);
  });

  test("a reopened link is silent while it works or fails again heard, and wakes once settled for good", () => {
    const wakes = (links: Link[]) =>
      wakesDue(book(links), {
        states: {},
        pending: {},
        waiting: {},
        threads: {
          parent: { title: "Parent", running: false },
          child: { title: "Child", running: false },
        },
        now: 100_000,
      }).length;
    const reopened = reopenLinks(book([stopped("parent/1")]), "child", ["parent/1"], 9_000).links[0]!;
    expect(wakes([reopened])).toBe(0);
    // It fails on the limit again with a wait armed: settled, but still heard.
    expect(wakes([{ ...reopened, state: "failed", settledAt: 10 }])).toBe(0);
    // It settles for good (`linksFinished` without a wait): the parent is told.
    expect(wakes([{ ...reopened, state: "done", settledAt: 10, notifiedAt: undefined }])).toBe(1);
  });

  test("a stopped link, a link to another thread and a newer link are left alone", () => {
    const base = book([
      stopped("parent/1", { state: "cancelled" }),
      stopped("parent/2", { target: "other" }),
      stopped("parent/3"),
      stopped("parent/4", { createdAt: 5 }),
    ]);
    const after = reopenLinks(base, "child", ["parent/1", "parent/2", "parent/3"], 9_000);
    expect(after.links.map((l) => l.state)).toEqual(["cancelled", "failed", "failed", "failed"]);
    expect(after).toBe(base);
  });
});
