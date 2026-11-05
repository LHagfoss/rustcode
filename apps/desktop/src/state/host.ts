// The app's side of threads starting and messaging threads: it answers the
// engine's `_rustcode/orchestration` requests by checking the rules
// (state/orchestration.ts), asking the user, creating threads and
// delivering messages through the ordinary turn queue, and reporting each
// link turn's answer. After ThreadMessaging.swift (M1), plus v2's
// `read_session`, `start_session` and `fan_out` (docs/orchestrator.md).
//
// The store imports this module and this module imports the store; nothing
// here touches the store at load time.

import { type Json, type JsonObject, RpcError } from "../acp/jsonrpc";
import { parsePullError, parsePullList, pullErrorTitle } from "../acp/rustcode";
import { Method, projectName } from "../acp/types";
import { hasShell, shellCall } from "../lib/shell";
import { childTitle, titleFrom } from "../lib/title";
import { selectOption, unavailableReason, usableHarnesses } from "./harness";
import { childAccountOn } from "./logins";
import {
  activeDescendants,
  activityOf,
  capped,
  checkManage,
  checkMessage,
  checkQueryData,
  checkStart,
  delivered,
  emptyBook,
  type Failure,
  type FanOut,
  failure,
  failureJson,
  fillAnswers,
  type GrantAsk,
  type GrantChoice,
  grant,
  type HeardMark,
  hasGrant,
  interruptActive,
  isActive,
  isFailure,
  type Link,
  type LinkBook,
  type LinkState,
  latestReply,
  linkResult,
  linksLeft,
  listSessions,
  MAX_ACTIVE_LINKS,
  MAX_PROJECTS,
  MAX_PULL_REQUESTS,
  markWoken,
  mergeBooks,
  nextId,
  nextWakeAt,
  orphanedLinks,
  type PullStateChoice,
  pageOf,
  parseBook,
  parseCall,
  projectOf,
  prune,
  quietTree,
  type RuleThread,
  renderRead,
  restateLinks,
  sameProject,
  shownState,
  supersededLinks,
  type TurnAnswer,
  unheard,
  updateLink,
  usageScope,
  type Wake,
  type WakeContext,
  wakeMessage,
  wakesDue,
} from "./orchestration";
import { delegationAllowed, effectiveMode, modeLabel, type PermissionMode } from "./permissions";
import {
  type AppState,
  cancel,
  createSession,
  ensureHarnessModels,
  errorText,
  markUnread,
  openThread,
  permissionModeOf,
  renameThread,
  request,
  runPrompt,
  searchThreads,
  setConfigOption,
  setPermissionMode,
  setThreadStatus,
  snoozeThread,
  statusOf,
  switchThreadHarness,
  toast,
  togglePin,
  updateThread,
  useApp,
} from "./store";
import { appendNotice, attachAutoAllowed, localId, type TranscriptItem, turnCompletion } from "./thread";
import { orchestrationTool } from "./toolNames";

const BOOK_KEY = "rustcode.threadLinks";
/** How long a change waits before it is written to the store: a run of tool calls is one save. */
const SAVE_DEBOUNCE_MS = 250;

/**
 * The book for the first paint, read synchronously from local storage. The
 * data store is the copy that survives a reinstall, a second window and a
 * wiped webview; `syncLinkBook` takes its answer as soon as the engine is up.
 * Links left active by a restart did not finish here and cannot resume by
 * themselves, so they are interrupted once, on load.
 */
export function loadBook(): LinkBook {
  try {
    const raw = localStorage.getItem(BOOK_KEY);
    if (!raw) return emptyBook();
    const parsed = JSON.parse(raw) as Partial<LinkBook>;
    return interruptActive({ ...emptyBook(), ...parsed });
  } catch {
    return emptyBook();
  }
}

/** Whether the engine keeps the book: undefined until it answers, false on an older engine. */
let engineLinks: boolean | undefined;
let saveTimer: ReturnType<typeof setTimeout> | undefined;
/** The book as the store last took it: a change that serialises the same is not sent. */
let savedText: string | undefined;

const text = (book: LinkBook) => JSON.stringify(book);

/** Stores a book, without a save: the first paint and the fallback for an old engine. */
function keepLocally(book: LinkBook) {
  try {
    localStorage.setItem(BOOK_KEY, text(book));
  } catch {
    // Not persisted in a private window.
  }
}

/** Writes the book to the store once the changes settle; only when it changed. */
function scheduleSave() {
  if (engineLinks !== true) return;
  clearTimeout(saveTimer);
  saveTimer = setTimeout(() => void flushSave(), SAVE_DEBOUNCE_MS);
}

/** Sends the book now, if it differs from what the store already has. */
async function flushSave(): Promise<void> {
  clearTimeout(saveTimer);
  saveTimer = undefined;
  if (engineLinks !== true) return;
  const current = book();
  const body = text(current);
  if (body === savedText) return;
  savedText = body;
  try {
    await request(Method.dataLinksSave, { book: JSON.parse(body) as Json }, 15_000);
  } catch (error) {
    // A store that cannot answer must not stop delegation: the local copy is still there.
    savedText = undefined;
    console.error("the link book was not saved", error);
  }
}

/**
 * Takes the store's copy of the book once the engine is up. With nothing
 * saved there, this window's local book is moved in once (the migration from
 * the release that kept it only in the webview). An engine without the method
 * answers `unknown method`: local storage stays the only copy, as before.
 */
export async function syncLinkBook(): Promise<void> {
  let answer: JsonObject;
  try {
    answer = await request<JsonObject>(Method.dataLinksLoad, {}, 10_000);
  } catch (error) {
    if (error instanceof RpcError && error.isMethodNotFound) engineLinks = false;
    console.error("the link book was not read from the data store", error);
    return;
  }
  engineLinks = true;
  const shared = parseBook(answer.book);
  if (!shared) {
    engineLinks = false;
    return;
  }
  if (answer.stored !== true) {
    // Nothing saved yet: this window's book is the one to move in, once.
    // `savedText` stays empty so the flush cannot decide it is already there.
    savedText = undefined;
    const mine = book();
    if (mine.links.length || Object.keys(mine.grants).length || mine.fanOuts.length) await flushSave();
    return;
  }
  // Both copies are from before this connect: nothing left active resumes.
  const merged = interruptActive(mergeBooks(interruptActive(shared), book()));
  useApp.setState({ links: prune(merged) });
  keepLocally(prune(merged));
  savedText = text(interruptActive(shared));
  // A link this window started while the store was answering is new to it.
  if (text(prune(merged)) !== text(prune(interruptActive(shared)))) scheduleSave();
}

function setBook(fn: (book: LinkBook) => LinkBook) {
  const next = prune(fn(useApp.getState().links));
  useApp.setState({ links: next });
  keepLocally(next);
  scheduleSave();
}

const book = () => useApp.getState().links;
/** How many distinct threads one thread may run at once (Settings › Agents). */
const threadLimit = () => useApp.getState().threadLimit;

// A tab or window closing mid-turn still leaves the store with what happened.
if (typeof window !== "undefined") {
  window.addEventListener("beforeunload", () => void flushSave());
}

/**
 * The links each parent is blocked on in `wait_session` right now: a wake for
 * one of them would be a duplicate, so it is held back while the wait runs.
 */
const waiting = new Map<string, Set<string>>();

/** How often a wait looks again, unless it is ended early (a dropped call, a newer wait). */
const WAIT_CHECK_MS = 300;

/** One `wait_session` in flight: the links it holds, and how it ends early. */
interface Wait {
  caller: string;
  links: Set<string>;
  /** Why it ended without an answer, when it did: its call was dropped, or a newer wait took it over. */
  ended?: "dropped" | "taken";
  /** Resolves the pause between checks, so an ended wait stops where it is. */
  nudge: () => void;
  /** The marks it set when handing links over, so a dropped call can take them back. */
  handed: HeardMark[];
}

/** Which wait holds each `caller/link`, and the waits the engine can name by call id. */
const holds = new Map<string, Wait>();
const calls = new Map<string, Wait>();
/**
 * What the last few calls handed over. An answer a wait has already sent is
 * dropped by the engine when the harness stopped waiting at that very moment:
 * the marks it set go back, and the ordinary wake delivers what nobody read.
 */
const handedBy = new Map<string, HeardMark[]>();
const HANDED_KEPT = 16;

const holdKey = (caller: string, link: string) => `${caller}\n${link}`;

/** The timer that re-checks a coalescing window; one at a time. */
let wakeTimer: ReturnType<typeof setTimeout> | undefined;

/**
 * How many times a wake waits for its parent's thread to become visible to the app (a second each),
 * before leaving it unmarked for the next reason to look. A prompt into a turn the app cannot see can
 * interrupt the tools that turn is running.
 */
const WAKE_WAIT_TRIES = 10;
/** How often each parent's wake has waited so far, by thread id. */
const wakeWaits = new Map<string, number>();

/** Between tests: no wait is in flight and no wake timer is armed by the case before. */
export function resetWaits(): void {
  waiting.clear();
  wakeWaits.clear();
  holds.clear();
  calls.clear();
  handedBy.clear();
  clearTimeout(wakeTimer);
  wakeTimer = undefined;
}

function ruleThread(s: AppState, id: string): RuleThread | undefined {
  const summary = s.sessions.find((x) => x.id === id);
  const thread = s.threads[id];
  if (!summary && !thread) return undefined;
  return {
    id,
    title: summary?.title ?? thread?.title ?? "Untitled thread",
    folder: thread?.cwd ?? summary?.cwd ?? "",
    worktree: thread?.worktree ?? summary?.worktree,
    updatedAt: summary?.updatedAt,
    running: thread?.running ?? false,
    harness: thread?.harness?.id ?? summary?.harness,
    status: statusOf(s, id),
  };
}

// ---------------------------------------------------------------- asking the user

let resolveAsk: ((choice: GrantChoice) => void) | null = null;

/** Ask once; a second request while one waits is refused, and the agent can try again. */
function ask(question: GrantAsk): Promise<GrantChoice> | Failure {
  if (useApp.getState().grant) return failure("busy", "another thread's request is waiting for the user");
  toast(`“${question.callerTitle}” asks to ${verb(question)}`, "info");
  return new Promise((resolve) => {
    resolveAsk = resolve;
    useApp.setState({ grant: { ...question, id: localId("grant") } });
  });
}

export function verb(q: GrantAsk): string {
  switch (q.kind) {
    case "message":
      return `message “${q.target}”`;
    case "read":
      return `read “${q.target}”`;
    case "start":
      return "start a thread";
    case "fan_out":
      return `run one task on ${q.harnesses?.length ?? 2} harnesses`;
    case "organize":
      return `change “${q.target}”`;
    case "interrupt":
      return `stop “${q.target}”`;
    case "configure":
      return `change the model of “${q.target}”`;
    case "transfer":
      return `move “${q.target}” to another harness`;
  }
}

/** The user's answer from the grant prompt. */
export function resolveGrant(choice: GrantChoice) {
  const resolve = resolveAsk;
  resolveAsk = null;
  useApp.setState({ grant: null });
  resolve?.(choice);
}

/** The orchestration tool each grant kind answers for, on the caller's own transcript row. */
const TOOL_OF_KIND: Record<GrantAsk["kind"], string> = {
  read: "read_session",
  message: "message_session",
  start: "start_session",
  fan_out: "fan_out",
  organize: "update_session",
  interrupt: "interrupt_session",
  configure: "configure_session",
  transfer: "transfer_session",
};

/** What the app did on the thread's behalf, when its call's row is not in flight to say it. */
const ALLOWED_NOTICE: Record<GrantAsk["kind"], string> = {
  read: "Read a thread",
  message: "Messaged a thread",
  start: "Started a thread",
  fan_out: "Fanned out across harnesses",
  organize: "Changed a thread",
  interrupt: "Stopped a thread",
  configure: "Changed a thread's model",
  transfer: "Moved a thread to another harness",
};

const toasted = new Set<string>();

async function permitted(caller: RuleThread, key: string, question: GrantAsk): Promise<Failure | null> {
  if (hasGrant(book(), caller.id, key)) return null;
  // The thread's access level answers for the user where it already allows the work: the same
  // policy that answers the harness's own approvals, so a Full-access thread is not re-asked here.
  const mode = permissionModeOf(useApp.getState(), caller.id);
  if (delegationAllowed(mode, question.kind)) {
    // Not silent: the call's own row says the thread did it on its own, and why. A notice would
    // both name the caller (the reader is already in that thread) and split the run of calls.
    const why = modeLabel(mode);
    let attached = false;
    updateThread(caller.id, (t) => {
      const marked = attachAutoAllowed(t, TOOL_OF_KIND[question.kind], why);
      attached = marked.attached;
      return marked.attached ? marked.state : t;
    });
    if (!attached) {
      updateThread(caller.id, (t) =>
        appendNotice(t, `${ALLOWED_NOTICE[question.kind]} · allowed by ${why}`, "info"),
      );
    }
    // The transcript keeps every one; the toast comes once per thread and kind, so an orchestrator
    // reading threads in a loop does not flood the screen.
    const once = `${caller.id}:${question.kind}`;
    if (!toasted.has(once)) {
      toasted.add(once);
      toast(`“${caller.title}” ${verb(question)}, allowed by ${why}`, "info");
    }
    return null;
  }
  const asked = ask(question);
  if (isFailure(asked)) return asked;
  const choice = await asked;
  if (choice === "deny") return failure("denied", `the user did not allow this thread to ${verb(question)}`);
  if (choice === "always") setBook((b) => grant(b, caller.id, key));
  return null;
}

// ---------------------------------------------------------------- the calls

/** Answer one `_rustcode/orchestration` request: a result, or `{ error }` the agent reads. */
export async function answerOrchestration(params: JsonObject): Promise<JsonObject> {
  try {
    return await answer(params);
  } catch (error) {
    return failureJson(failure("failed", errorText(error)));
  }
}

async function answer(params: JsonObject): Promise<JsonObject> {
  const s = useApp.getState();
  const callerId = typeof params.sessionId === "string" ? params.sessionId : "";
  const caller = ruleThread(s, callerId);
  if (!caller) return failureJson(failure("not_found", "the calling thread is not listed"));
  const call = parseCall(params);
  if (isFailure(call)) return failureJson(call);
  const acrossProjects = () => useApp.getState().readAcrossProjects;
  /** A read that asked for every project and may not have it: say so, rather than answer half. */
  const reachRefused = () =>
    failureJson(
      failure(
        "not_allowed",
        "reading across projects is off; turn on “Let agents read threads in other projects” in Settings › Agents",
      ),
    );
  const threads = () => {
    const st = useApp.getState();
    return st.sessions.flatMap((x) => ruleThread(st, x.id) ?? []);
  };
  switch (call.tool) {
    case "list_sessions":
      if (call.allProjects && !acrossProjects()) return reachRefused();
      return listSessions(threads(), caller, call.query, call.allProjects);
    case "read_session":
      return read(caller, call.target, call.lastTurns, call.offset);
    case "message_session":
      return message(caller, call.target, call.text);
    case "wait_session":
      return wait(caller.id, call.links, call.timeoutMs, call.callId);
    case "start_session":
      return start(caller, call);
    case "fan_out":
      return fanOut(caller, call);
    case "list_harnesses":
      return listHarnesses(caller);
    case "search_sessions":
      if (call.allProjects && !acrossProjects()) return reachRefused();
      return searchSessions(threads(), caller, call.query, call.allProjects);
    case "list_worktrees":
      if (call.allProjects && !acrossProjects()) return reachRefused();
      return listWorktrees(threads(), caller, call.allProjects);
    case "list_projects":
      return listProjects(s);
    case "list_pull_requests":
      return listPullRequests(caller, call.state);
    case "update_session":
      return updateSession(caller, call);
    case "interrupt_session":
      return interruptSession(caller, call.target);
    case "configure_session":
      return configureSession(caller, call);
    case "transfer_session":
      return transferSession(caller, call.target, call.harness);
    case "list_pending_requests":
      return listPendingRequests(caller);
    case "thread_usage": {
      const scope = usageScope(caller, acrossProjects(), call.project);
      if (isFailure(scope)) return failureJson(scope);
      try {
        const answer = await request<JsonObject>(
          Method.dataUsage,
          {
            ...(call.since ? { since: call.since } : {}),
            groupBy: call.groupBy,
            ...(scope.project ? { project: scope.project } : {}),
          },
          20_000,
        );
        return { ...answer, scope: scope.scope };
      } catch (error) {
        return failureJson(failure("failed", `the data store did not answer: ${errorText(error)}`));
      }
    }
    case "query_data": {
      const refused = checkQueryData(acrossProjects());
      if (refused) return failureJson(refused);
      try {
        return await request<JsonObject>(Method.dataQuery, { sql: call.sql, limit: call.limit }, 20_000);
      } catch (error) {
        return failureJson(failure("failed", `the data store did not answer: ${errorText(error)}`));
      }
    }
  }
}

// ---------------------------------------------------------------- reads: no approval

/** Models listed per harness: an agent's context pays for each one. */
const LISTED_MODELS = 25;

async function listHarnesses(caller: RuleThread): Promise<JsonObject> {
  const snapshot = useApp.getState().harnesses;
  const harnesses = snapshot?.harnesses ?? [];
  await Promise.all(harnesses.filter((h) => !unavailableReason(h)).map((h) => ensureHarnessModels(h.id)));
  const fresh = useApp.getState().harnesses?.harnesses ?? harnesses;
  return {
    current: { harness: caller.harness ?? "rustcode", ...ranWith(caller.id) },
    harnesses: fresh.map((h) => {
      const reason = unavailableReason(h);
      const models = h.models.slice(0, LISTED_MODELS).map((m) => ({
        id: m.id,
        name: m.name,
        ...(m.efforts.length ? { efforts: m.efforts } : {}),
      }));
      return {
        id: h.id,
        name: h.name,
        available: !reason,
        ...(reason ? { reason } : { models }),
        ...(!reason && h.models.length > LISTED_MODELS
          ? { omitted_models: h.models.length - LISTED_MODELS }
          : {}),
      };
    }),
  };
}

async function searchSessions(
  all: RuleThread[],
  caller: RuleThread,
  query: string,
  allProjects = false,
): Promise<JsonObject> {
  const said = await searchThreads(query);
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  const matches = all
    .filter((t) => allProjects || sameProject(projectOf(t), projectOf(caller)))
    .filter((t) => t.id in said || words.every((w) => t.title.toLowerCase().includes(w)))
    .sort((a, b) => (b.updatedAt ?? 0) - (a.updatedAt ?? 0))
    .slice(0, 30);
  return {
    sessions: matches.map((t) => ({
      session_id: t.id,
      title: t.title,
      status: t.status ?? (t.running ? "running" : "idle"),
      ...(allProjects ? { project: projectOf(t) } : {}),
      ...(said[t.id] ? { snippet: said[t.id] } : {}),
    })),
  };
}

function listWorktrees(all: RuleThread[], caller: RuleThread, allProjects = false): JsonObject {
  return {
    worktrees: all.flatMap((t) =>
      t.worktree && (allProjects || sameProject(projectOf(t), projectOf(caller)))
        ? [
            {
              session_id: t.id,
              title: t.title,
              ...(allProjects ? { project: projectOf(t) } : {}),
              branch: t.worktree.branch,
              path: t.worktree.path,
            },
          ]
        : [],
    ),
  };
}

/** `list_projects`: every project the user has a thread in, most recently updated first. */
function listProjects(s: AppState): JsonObject {
  const byPath = new Map<string, { path: string; threads: number; working: boolean; updatedAt: number }>();
  for (const x of s.sessions) {
    const path = projectOf({ folder: x.cwd, worktree: x.worktree });
    if (!path) continue;
    const entry = byPath.get(path) ?? { path, threads: 0, working: false, updatedAt: 0 };
    entry.threads += 1;
    entry.working ||= !!s.threads[x.id]?.running || x.status === "running";
    entry.updatedAt = Math.max(entry.updatedAt, x.updatedAt ?? 0);
    byPath.set(path, entry);
  }
  const projects = [...byPath.values()]
    .sort((a, b) => b.updatedAt - a.updatedAt)
    .slice(0, MAX_PROJECTS)
    .map((p) => ({ path: p.path, name: projectName(p.path), threads: p.threads, working: p.working }));
  return { projects };
}

/** Why a pull request read failed, in the shape the agent reads: `{ error: { code, message } }`. */
function pullFailure(error: { code: string; message: string }): Failure {
  return failure(
    error.code === "failed" ? "failed" : "target_unavailable",
    `${pullErrorTitle(error.code)}: ${error.message}`,
  );
}

/** `list_pull_requests`: the caller's project's pull requests, through the panel's own call. */
async function listPullRequests(caller: RuleThread, state: PullStateChoice): Promise<JsonObject> {
  const cwd = projectOf(caller);
  if (!cwd) return failureJson(failure("failed", "this thread has no project folder"));
  let result: Json;
  try {
    result = await request(Method.pulls, { cwd, state, limit: MAX_PULL_REQUESTS }, 60_000);
  } catch (error) {
    return failureJson(failure("target_unavailable", `pull requests could not be read: ${errorText(error)}`));
  }
  const error = parsePullError(result);
  if (error) return failureJson(pullFailure(error));
  return {
    pull_requests: parsePullList(result).pulls.map((p) => ({
      number: p.number,
      title: p.title,
      state: p.state,
      branch: p.headRef,
      ...(p.url ? { url: p.url } : {}),
    })),
  };
}

// ---------------------------------------------------------------- managing threads

type UpdateCall = Extract<ReturnType<typeof parseCall>, { tool: "update_session" }>;
type ConfigureCall = Extract<ReturnType<typeof parseCall>, { tool: "configure_session" }>;

const STATUS_OF_CHOICE = { active: "idle", review: "review", settled: "done" } as const;

/** The thread a management tool acts on: the caller's own when none is named. */
function manageTarget(caller: RuleThread, id: string | undefined): RuleThread | Failure {
  const target = id ? ruleThread(useApp.getState(), id) : caller;
  return checkManage(caller, target) ?? (target as RuleThread);
}

async function updateSession(caller: RuleThread, call: UpdateCall): Promise<JsonObject> {
  const target = manageTarget(caller, call.target);
  if (isFailure(target)) return failureJson(target);
  if (target.id !== caller.id) {
    const changes = [
      call.title && `rename it to “${call.title}”`,
      call.pinned !== undefined && (call.pinned ? "pin it" : "unpin it"),
      call.unread && "mark it unread",
      call.status && `set its status to ${call.status}`,
      call.snoozedUntil ? "snooze it" : call.snoozedUntil === null && "bring it back from snooze",
    ].filter(Boolean);
    const no = await permitted(caller, `organize:${target.id}`, {
      kind: "organize",
      caller: caller.id,
      callerTitle: caller.title,
      target: target.title,
      detail: `It would ${changes.join(", ")}.`,
    });
    if (no) return failureJson(no);
  }
  if (call.title && !(await renameThread(target.id, call.title))) {
    return failureJson(failure("failed", "the thread could not be renamed"));
  }
  if (call.pinned !== undefined && useApp.getState().pinned.includes(target.id) !== call.pinned) {
    togglePin(target.id);
  }
  if (call.unread) markUnread(target.id);
  if (call.status) await setThreadStatus(target.id, STATUS_OF_CHOICE[call.status]);
  if (call.snoozedUntil !== undefined) snoozeThread(target.id, call.snoozedUntil);
  const after = ruleThread(useApp.getState(), target.id);
  const until = useApp.getState().snoozed[target.id];
  return {
    session_id: target.id,
    title: after?.title ?? target.title,
    pinned: useApp.getState().pinned.includes(target.id),
    status: after?.status ?? "idle",
    snoozed_until: until ? new Date(until).toISOString() : null,
  };
}

async function interruptSession(caller: RuleThread, id: string): Promise<JsonObject> {
  const target = manageTarget(caller, id);
  if (isFailure(target)) return failureJson(target);
  if (target.id === caller.id) return failureJson(failure("not_allowed", "a thread cannot stop itself"));
  if (!target.running) return { session_id: target.id, interrupted: false, status: target.status ?? "idle" };
  const no = await permitted(caller, `interrupt:${target.id}`, {
    kind: "interrupt",
    caller: caller.id,
    callerTitle: caller.title,
    target: target.title,
    detail:
      "Its current turn stops, and the threads it is working through stop with it. The thread stays and can be messaged again.",
  });
  if (no) return failureJson(no);
  // What goes down with it, counted before the cascade starts.
  const stopped = activeDescendants(book(), target.id).length;
  await cancel(target.id);
  return { session_id: target.id, interrupted: true, stopped };
}

async function configureSession(caller: RuleThread, call: ConfigureCall): Promise<JsonObject> {
  const target = manageTarget(caller, call.target);
  if (isFailure(target)) return failureJson(target);
  await openThread(target.id);
  const options = () => useApp.getState().threads[target.id]?.configOptions ?? [];
  const changes: [string, string][] = [];
  if (call.model) changes.push(["model", call.model]);
  if (call.effort) changes.push(["effort", call.effort]);
  for (const [id, value] of changes) {
    const option = selectOption(options(), id);
    if (!option) return failureJson(failure("not_allowed", `${target.title} has no ${id} setting`));
    const known = option.choices.map((c) => c.value);
    if (!known.includes(value)) {
      return failureJson(
        failure("invalid", `no ${id} “${value}”; choose from ${known.slice(0, 25).join(", ")}`),
      );
    }
  }
  if (changes.length && target.id !== caller.id) {
    const no = await permitted(caller, `configure:${target.id}`, {
      kind: "configure",
      caller: caller.id,
      callerTitle: caller.title,
      target: target.title,
      detail: `Its next turn would run on ${changes.map(([, value]) => value).join(" · ")}.`,
    });
    if (no) return failureJson(no);
  }
  for (const [id, value] of changes) await setConfigOption(target.id, id, value);
  return {
    session_id: target.id,
    harness: target.harness ?? "rustcode",
    access: permissionModeOf(useApp.getState(), target.id),
    ...ranWith(target.id),
  };
}

async function transferSession(caller: RuleThread, id: string, harness: string): Promise<JsonObject> {
  const target = manageTarget(caller, id);
  if (isFailure(target)) return failureJson(target);
  if (target.id === caller.id) return failureJson(failure("not_allowed", "a thread cannot move itself"));
  if (target.running)
    return failureJson(failure("busy", `${target.title} is working; stop it or wait for it first`));
  const listed = useApp.getState().harnesses?.harnesses ?? [];
  const to = listed.find((h) => h.id === harness);
  if (!to) {
    return failureJson(
      failure("invalid", `no harness “${harness}”; choose from ${listed.map((h) => h.id).join(", ")}`),
    );
  }
  const reason = unavailableReason(to);
  if (reason) return failureJson(failure("not_allowed", `${to.name} is not available: ${reason}`));
  if (target.harness === harness) return { session_id: target.id, harness, moved: false };
  const no = await permitted(caller, `transfer:${target.id}`, {
    kind: "transfer",
    caller: caller.id,
    callerTitle: caller.title,
    target: target.title,
    detail: `It would continue on ${to.name}, which gets the conversation so far with its next message.`,
  });
  if (no) return failureJson(no);
  await openThread(target.id);
  if (!(await switchThreadHarness(target.id, harness))) {
    return failureJson(failure("failed", "the thread could not be moved"));
  }
  return { session_id: target.id, harness, moved: true, ...ranWith(target.id) };
}

/** What this project's open threads wait on. Only what they ask: answering stays with the user. */
function listPendingRequests(caller: RuleThread): JsonObject {
  const s = useApp.getState();
  const requests = Object.values(s.threads)
    .filter((t) => t.permissions.length)
    .map((t) => ({ thread: t, rule: ruleThread(s, t.id) }))
    .filter(({ rule }) => rule && sameProject(projectOf(rule), projectOf(caller)))
    .flatMap(({ thread, rule }) =>
      thread.permissions.map((p) => ({
        session_id: thread.id,
        title: rule?.title ?? "",
        kind: p.question ? "question" : "approval",
        asks: p.question?.text ?? p.title,
      })),
    );
  return { requests };
}

async function read(
  caller: RuleThread,
  targetId: string,
  lastTurns: number,
  offset: number,
): Promise<JsonObject> {
  const target = ruleThread(useApp.getState(), targetId);
  const refused = checkMessage(caller, target, book(), true, useApp.getState().readAcrossProjects);
  if (refused || !target) return failureJson(refused ?? failure("not_found", "no such thread"));
  // Its project only matters to the user when it is not the caller's.
  const elsewhere = sameProject(projectOf(target), projectOf(caller))
    ? ""
    : ` It is in ${projectOf(target)}, another project.`;
  const no = await permitted(caller, target.id, {
    kind: "read",
    caller: caller.id,
    callerTitle: caller.title,
    target: target.title,
    detail: `Its last ${lastTurns} turn${lastTurns === 1 ? "" : "s"}: your messages and its replies, no tool output.${elsewhere}`,
  });
  if (no) return failureJson(no);
  await openThread(target.id);
  const items = useApp.getState().threads[target.id]?.items ?? [];
  const turns = turnsOf(items).slice(-lastTurns);
  // One rendered answer, then a page of it: pages join with nothing lost or repeated.
  const rendered = renderRead({
    session_id: target.id,
    title: target.title,
    status: target.status ?? "idle",
    project: projectOf(target),
    turns: turns.map((t) => ({
      user: String(t.user ?? ""),
      assistant: String(t.assistant ?? ""),
      files_changed: Number(t.files_changed ?? 0),
    })),
  });
  const page = pageOf(rendered, offset);
  if (isFailure(page)) return failureJson(page);
  return {
    session_id: target.id,
    title: target.title,
    status: target.status ?? "idle",
    project: projectOf(target),
    offset: page.offset,
    text: page.text,
    ...(page.nextOffset === null ? { complete: true } : { next_offset: page.nextOffset }),
  };
}

/** A thread's turns as `read_session` gives them: the message, the last reply and how many files changed. */
export function turnsOf(items: TranscriptItem[]): JsonObject[] {
  const out: JsonObject[] = [];
  let current: { user: string; assistant: string; start: number } | null = null;
  const close = (end: number) => {
    if (!current) return;
    const files = turnCompletion(items, end)?.files.length ?? 0;
    out.push({ user: current.user, assistant: current.assistant, files_changed: files });
  };
  items.forEach((item, i) => {
    if (item.kind === "user") {
      close(i);
      current = { user: item.text, assistant: "", start: i };
    } else if (item.kind === "assistant" && current) {
      current.assistant = item.text;
    }
  });
  close(items.length);
  return out;
}

async function message(caller: RuleThread, targetId: string, text: string): Promise<JsonObject> {
  const target = ruleThread(useApp.getState(), targetId);
  const refused = checkMessage(caller, target, book(), false, false, threadLimit());
  if (refused || !target) return failureJson(refused ?? failure("not_found", "no such thread"));
  const no = await permitted(caller, target.id, {
    kind: "message",
    caller: caller.id,
    callerTitle: caller.title,
    target: target.title,
    detail: text,
  });
  if (no) return failureJson(no);
  // The user took their time: look again.
  const again = checkMessage(
    caller,
    ruleThread(useApp.getState(), targetId),
    book(),
    false,
    false,
    threadLimit(),
  );
  if (again) return failureJson(again);
  await openThread(target.id);
  const thread = useApp.getState().threads[target.id];
  if (!thread || thread.error) {
    return failureJson(failure("target_unavailable", "that thread could not be opened"));
  }
  const busy = thread.running;
  const id = addLink({
    orchestrator: caller.id,
    target: target.id,
    orchestratorTitle: caller.title,
    targetTitle: target.title,
    message: text,
    state: busy ? "queued" : "working",
  });
  deliver(target.id, delivered(text, caller.title), id);
  return { link_id: id, receipt: busy ? "queued" : "started", session_id: target.id, title: target.title };
}

function addLink(link: Omit<Link, "id" | "createdAt">): string {
  let id = "";
  setBook((b) => {
    const next = nextId(b, link.orchestrator);
    id = next.id;
    return { ...next.book, links: [...next.book.links, { ...link, id, createdAt: Date.now() }] };
  });
  return id;
}

/** After the turn the target runs, never into it: the same queue a typed message uses. */
function deliver(target: string, text: string, link: string) {
  const thread = useApp.getState().threads[target];
  if (thread?.running) {
    updateThread(target, (t) => ({ ...t, queue: [...t.queue, { id: localId("queued"), text, link }] }));
  } else {
    void runPrompt(target, text, undefined, { link });
  }
}

type StartCall = Extract<ReturnType<typeof parseCall>, { tool: "start_session" }>;
type FanOutCall = Extract<ReturnType<typeof parseCall>, { tool: "fan_out" }>;

/** Checks a harness id against the engine's list; the default when none is named. */
/**
 * The harness a delegated thread runs on: it must be installed, turned on and
 * signed in (the same rule as the picker), else the error names the ones that
 * can run now, so the agent can pick again (issue 2799).
 */
function harnessFor(name: string | undefined): { id: string; name: string } | Failure {
  const snapshot = useApp.getState().harnesses;
  const id = name ?? snapshot?.default.harness ?? "rustcode";
  const h = snapshot?.harnesses.find((x) => x.id === id || x.name.toLowerCase() === id.toLowerCase());
  if (!h) return failure("invalid", `no harness “${id}”; the ones that can run now are ${usableIds()}`);
  const reason = unavailableReason(h);
  if (reason)
    return failure("not_allowed", `${h.name} cannot run a thread: ${reason}. Usable now: ${usableIds()}`);
  return { id: h.id, name: h.name };
}

/**
 * The account a delegated thread runs on: the parent's own when it runs on the
 * same harness, else the one a new thread would start on (a signed-out default
 * login is not one; `docs/harnesses.md`).
 */
function accountFor(caller: RuleThread, harness: string): string | null {
  const s = useApp.getState();
  return childAccountOn(
    s.harnesses?.harnesses.find((h) => h.id === harness),
    {
      harness: caller.harness,
      account: s.threads[caller.id]?.harness?.account ?? s.sessions.find((x) => x.id === caller.id)?.account,
    },
    s.signedOutLogins,
  );
}

const usableIds = () => {
  const snapshot = useApp.getState().harnesses;
  return (
    (snapshot
      ? usableHarnesses(snapshot)
          .map((h) => h.id)
          .join(", ")
      : "") || "rustcode"
  );
};

/**
 * The model a delegated thread asks for, checked against what the harness
 * lists (asked for once if it has not listed them yet). An unknown id would
 * be ignored by the harness and the child would quietly run on its default,
 * so it is refused with the closest matches instead.
 */
async function modelFor(harness: string, model: string | undefined): Promise<string | undefined | Failure> {
  if (!model) return undefined;
  await ensureHarnessModels(harness);
  const h = useApp.getState().harnesses?.harnesses.find((x) => x.id === harness);
  const models = h?.models ?? [];
  if (!models.length) return model;
  const wanted = model.toLowerCase();
  const exact = models.find((m) => m.id.toLowerCase() === wanted || m.name.toLowerCase() === wanted);
  if (exact) return exact.id;
  // "deepseek-v4-pro" for "deepseek/deepseek-v4-pro": a provider prefix left off, when only one fits.
  const suffixed = models.filter((m) => m.id.toLowerCase().endsWith(`/${wanted}`));
  if (suffixed.length === 1) return suffixed[0]!.id;
  const close = models
    .filter(
      (m) =>
        m.id.toLowerCase().includes(wanted) || wanted.includes(m.id.toLowerCase().split("/").pop() ?? ""),
    )
    .slice(0, 5)
    .map((m) => m.id);
  const offer = (close.length ? close : models.slice(0, 8).map((m) => m.id)).join(", ");
  return failure(
    "invalid",
    `${h?.name ?? harness} has no model “${model}”; ${close.length ? "closest" : "some it lists"}: ${offer}`,
  );
}

/** "Claude Code · opus · high" for the approval card: what will actually run. */
const choiceLabel = (name: string, model?: string, effort?: string) =>
  [name, model, effort].filter(Boolean).join(" · ");

/** The model and effort the new thread reports it runs on, for the caller's answer. */
function ranWith(id: string): JsonObject {
  const options = useApp.getState().threads[id]?.configOptions ?? [];
  const model = selectOption(options, "model")?.current;
  const effort = selectOption(options, "effort")?.current;
  return { ...(model ? { model } : {}), ...(effort ? { effort } : {}) };
}

/** The project a new thread starts in: the caller's, or one the user has opened before. */
function projectFor(caller: RuleThread, asked: string | undefined): string | Failure {
  const own = projectOf(caller);
  if (!asked || sameProject(asked, own)) return own;
  const known = useApp.getState().sessions.some((x) => sameProject(x.worktree?.repo || x.cwd, asked));
  if (!known) return failure("not_allowed", "that folder is not a project the user has opened in RustCode");
  return asked.replace(/\/+$/, "");
}

/** `session/new` for a link, in a worktree when asked (or by default in a git project). */
async function newThread(
  project: string,
  harness: string,
  options: {
    model?: string;
    effort?: string;
    account?: string | null;
    title?: string;
    task: string;
    worktree?: boolean;
    /** The thread starting this one, recorded in the child's metadata. */
    parent?: string;
    /** What the new thread may do without asking (`childMode`), fixed before the user is asked. */
    access: PermissionMode;
  },
): Promise<string> {
  // The orchestrator names it when it gave a title, and the task itself does otherwise: a session
  // made without one takes the message it is sent as its title, and the sidebar then shows
  // "From thread “parent” (agent)…" for good.
  const title = childTitle(options.title, options.task);
  const { access, ...rest } = options;
  const id = await createThreadFor(project, harness, { ...rest, title });
  // Before its first turn, so nothing it does runs broader than the thread that started it.
  setPermissionMode(id, access);
  // The orchestrator named it; without this the first message ("From thread …") would.
  if (hasShell()) await shellCall("thread_rename", { id, title }).catch(() => undefined);
  return id;
}

/**
 * What a delegated thread runs at: the lower of its parent's access and Settings ›
 * General › Default access, which is what a new thread would get anyway. The parent's
 * own limit is the one that counts, so an Ask or Auto-edit parent never hands out Full
 * access. Applied at every depth, a grandchild can only be narrower still.
 */
function childMode(caller: RuleThread): PermissionMode {
  const s = useApp.getState();
  return effectiveMode(permissionModeOf(s, caller.id), s.defaultPermissionMode);
}

/** What a thread may do without asking, as the orchestration answers report it. */
const accessOf = (id: string): PermissionMode => permissionModeOf(useApp.getState(), id);

async function createThreadFor(
  project: string,
  harness: string,
  options: {
    model?: string;
    effort?: string;
    account?: string | null;
    title: string;
    worktree?: boolean;
    parent?: string;
  },
): Promise<string> {
  const wantWorktree = (options.worktree ?? true) && !!useApp.getState().features.worktrees;
  try {
    return await createSession({
      cwd: project,
      harness,
      account: options.account ?? undefined,
      model: options.model,
      effort: options.effort,
      title: options.title,
      parent: options.parent,
      worktree: wantWorktree ? { repo: project, title: options.title } : undefined,
    });
  } catch (error) {
    // The default (a worktree) does not apply outside a git repository: start in the folder.
    if (wantWorktree && options.worktree === undefined && /git|repositor/i.test(errorText(error))) {
      return createSession({
        cwd: project,
        harness,
        account: options.account ?? undefined,
        model: options.model,
        effort: options.effort,
        title: options.title,
        parent: options.parent,
      });
    }
    throw error;
  }
}

async function start(caller: RuleThread, call: StartCall): Promise<JsonObject> {
  const refused = checkStart(caller, book(), 1, threadLimit());
  if (refused) return failureJson(refused);
  const project = projectFor(caller, call.project);
  if (isFailure(project)) return failureJson(project);
  const harness = harnessFor(call.harness);
  if (isFailure(harness)) return failureJson(harness);
  const model = await modelFor(harness.id, call.model);
  if (isFailure(model)) return failureJson(model);
  const title = call.title ?? titleFrom(call.prompt, 60);
  const other = !sameProject(project, projectOf(caller));
  // Fixed before the user is asked, so the card says what the thread runs at and the thread gets
  // exactly that, however long the card waits.
  const access = childMode(caller);
  const no = await permitted(caller, other ? `start:${project}` : "start", {
    kind: "start",
    caller: caller.id,
    callerTitle: caller.title,
    target: title,
    detail: call.prompt,
    project: other ? project : undefined,
    harnesses: [choiceLabel(harness.name, model, call.effort)],
    access,
    worktree: call.worktree ?? true,
  });
  if (no) return failureJson(no);
  const again = checkStart(caller, book(), 1, threadLimit());
  if (again) return failureJson(again);
  let id: string;
  try {
    // The task is the title's fallback: whatever the prompt starts with, a child never goes
    // unnamed and so never takes the delivered message as its title.
    id = await newThread(project, harness.id, {
      ...call,
      model,
      title,
      task: call.prompt,
      access,
      parent: caller.id,
      account: accountFor(caller, harness.id),
    });
  } catch (error) {
    return failureJson(failure("failed", `the thread could not start: ${errorText(error)}`));
  }
  const link = addLink({
    orchestrator: caller.id,
    target: id,
    orchestratorTitle: caller.title,
    targetTitle: title,
    message: call.prompt,
    state: "working",
    started: true,
    harness: harness.id,
  });
  deliver(id, delivered(call.prompt, caller.title), link);
  return {
    session_id: id,
    link_id: link,
    receipt: "started",
    title,
    harness: harness.id,
    access: accessOf(id),
    ...ranWith(id),
    links_left: linksLeft(book(), caller.id, threadLimit()),
  };
}

async function fanOut(caller: RuleThread, call: FanOutCall): Promise<JsonObject> {
  const refused = checkStart(caller, book(), call.harnesses.length, threadLimit());
  if (refused) return failureJson(refused);
  const chosen: { id: string; name: string; model?: string; effort?: string }[] = [];
  for (const entry of call.harnesses) {
    const h = harnessFor(entry.harness);
    if (isFailure(h)) return failureJson(h);
    const model = await modelFor(h.id, entry.model);
    if (isFailure(model)) return failureJson(model);
    chosen.push({ ...h, model, effort: entry.effort });
  }
  const project = projectOf(caller);
  const title = call.title ?? titleFrom(call.prompt, 60);
  const access = childMode(caller);
  const no = await permitted(caller, "start", {
    kind: "fan_out",
    caller: caller.id,
    callerTitle: caller.title,
    target: title,
    detail: call.prompt,
    harnesses: chosen.map((h) => choiceLabel(h.name, h.model, h.effort)),
    access,
    worktree: true,
  });
  if (no) return failureJson(no);
  const again = checkStart(caller, book(), chosen.length, threadLimit());
  if (again) return failureJson(again);
  const fanId = localId("fan");
  const out: JsonObject[] = [];
  const linkIds: string[] = [];
  for (const h of chosen) {
    const threadTitle = `${title} · ${h.name}`;
    try {
      const id = await newThread(project, h.id, {
        model: h.model,
        effort: h.effort,
        title: threadTitle,
        task: call.prompt,
        parent: caller.id,
        access,
        account: accountFor(caller, h.id),
      });
      const link = addLink({
        orchestrator: caller.id,
        target: id,
        orchestratorTitle: caller.title,
        targetTitle: threadTitle,
        message: call.prompt,
        state: "working",
        started: true,
        harness: h.id,
        fanOut: fanId,
      });
      linkIds.push(link);
      out.push({ session_id: id, link_id: link, harness: h.id, access: accessOf(id), ...ranWith(id) });
      deliver(id, delivered(call.prompt, caller.title), link);
    } catch (error) {
      // The reason, not the name of the error class: the harness says why it could not start.
      out.push({ harness: h.id, ...failureJson(failure("failed", errorText(error))) });
    }
  }
  if (linkIds.length) {
    const group: FanOut = {
      id: fanId,
      orchestrator: caller.id,
      title,
      prompt: call.prompt,
      links: linkIds,
      createdAt: Date.now(),
    };
    setBook((b) => ({ ...b, fanOuts: [...b.fanOuts, group] }));
  }
  return { links: out, fan_out_id: fanId, links_left: linksLeft(book(), caller.id, threadLimit()) };
}

/** A link's state now: a running turn that waits on an approval or a question needs the user. */
export function stateOf(s: AppState, link: Link) {
  return shownState(link, !!s.threads[link.target]?.permissions.length);
}

/**
 * `wait_session`: blocks until one of the links settles or asks the user, or
 * the timeout passes. Each answer carries what the app already knows about the
 * child (idle time, last tool, turns), so a timeout says whether it is
 * thinking or stuck. While it runs, this parent is registered as waiting, so
 * no wake repeats what the wait itself returns.
 *
 * A wait ends without an answer when nobody can read one any more: its call was
 * dropped by the harness (`callId`, see {@link orchestrationCallDropped}), its
 * own transcript row finished, or a newer wait took its links over. Its links
 * are then free at once, and anything that settles comes as the ordinary wake.
 */
async function wait(
  callerId: string,
  ids: string[],
  timeoutMs: number,
  callId?: string,
): Promise<JsonObject> {
  const mine = [...new Set(ids)].filter(
    (id) => book().links.find((l) => l.id === id)?.orchestrator === callerId,
  );
  if (!mine.length) return failureJson(failure("not_found", "no such link from this thread"));
  const deadline = Date.now() + timeoutMs;
  const until = new Date(deadline).toISOString();
  const me: Wait = { caller: callerId, links: new Set(mine), nudge: () => {}, handed: [] };
  // The newest wait on a link takes it over: two waits holding one link could each hand its
  // answer over, and only one of them is being read.
  for (const id of mine) {
    const held = holds.get(holdKey(callerId, id));
    if (held && held !== me) {
      held.ended = "taken";
      held.nudge();
    }
    holds.set(holdKey(callerId, id), me);
    const set = waiting.get(callerId) ?? new Set<string>();
    set.add(id);
    waiting.set(callerId, set);
  }
  if (callId) calls.set(callId, me);
  // Some harnesses drop a call without the engine hearing of it (a tool timeout
  // inside the harness): the caller's own row for the call then ends while the
  // wait runs. Only a caller with one wait row in flight is certainly looking at
  // its own, and a wrong guess costs no answer: a released link comes as a wake.
  const row = waitRowOf(callerId);
  const endEarly = (why: "dropped" | "taken"): JsonObject => {
    me.ended = why;
    return {
      status: why === "taken" ? "taken_over" : "dropped",
      waited_until: until,
      links: [],
      note:
        why === "taken"
          ? "A newer wait_session took these links over, so this one stopped: read that answer. Anything that settles still comes as a wake; do not poll."
          : "This call was dropped (the harness stopped waiting for it), so the wait stopped and its links are free. Anything that settles comes as a wake; do not poll.",
    };
  };
  /**
   * What the parent is told the link is now. A child that settled but whose own subtree still works
   * has not answered yet: it says it works, with how long it has been quiet, rather than hand back
   * the answer of a turn that only started the work below it.
   */
  const shownOf = (link: Link): LinkState => {
    const state = stateOf(useApp.getState(), link);
    return (state === "done" || state === "failed") && !subtreeQuiet(link.target) ? "working" : state;
  };
  const result = (link: Link, shown = shownOf(link)) => {
    const s = useApp.getState();
    const now = Date.now();
    const activity = activityOf(s.threads[link.target]?.items ?? []);
    // A settled link with no answer still has one: the target's last reply, read now.
    const reply = link.answer?.trim()
      ? link
      : filledWith(link, latestReply(s.threads[link.target]?.items ?? []));
    return linkResult(reply, shown, statusOf(s, link.target), { now, activity });
  };
  // The link's answer is out with this reply: no wake may repeat it. A link whose state goes out
  // with a timeout is handed over the same way, so a child that settles just as the wait gives up
  // does not come back a second time as a wake.
  const hand = (link: Link, shown = shownOf(link)) => {
    if (shown === "done" || shown === "failed" || shown === "needs_user") {
      me.handed.push(markHeard(link, shown, Date.now()));
    }
    return { links: [result(link, shown)] };
  };
  /** The pause before looking again, cut short when the wait ends early. */
  const rest = () => {
    if (me.ended) return Promise.resolve();
    return new Promise<void>((resolve) => {
      let timer: ReturnType<typeof setTimeout> | undefined;
      const done = () => {
        clearTimeout(timer);
        me.nudge = () => {};
        resolve();
      };
      timer = setTimeout(done, WAIT_CHECK_MS);
      me.nudge = done;
    });
  };
  try {
    for (;;) {
      if (me.ended) return endEarly(me.ended);
      if (row && !waitRowRuns(callerId, row)) return endEarly("dropped");
      const s = useApp.getState();
      // The turn that asked is over (the user stopped it, or the harness dropped the call): nobody
      // will read this answer, and holding the links back would keep their wakes from arriving.
      // A thread the app holds no state for keeps waiting: it cannot tell whether a turn runs.
      const caller = s.threads[callerId];
      if (caller && !caller.running) {
        return {
          status: "timeout",
          waited_until: until,
          links: mine.flatMap((id) => {
            const link = book().links.find((l) => l.id === id);
            return link ? [result(link)] : [];
          }),
          note: "The turn that waited has ended, so the wait stopped. Anything that settles still comes as a wake; do not poll.",
        };
      }
      for (const id of mine) {
        const link = book().links.find((l) => l.id === id);
        if (!link) continue;
        if (isActive(link.state) && !s.sessions.some((x) => x.id === link.target)) {
          setBook((b) => updateLink(b, id, { state: "failed", answer: "That thread is gone." }));
          continue;
        }
        const now = stateOf(s, link);
        if (now === "done" || now === "failed") {
          // Settled, but it works on: the answer is the one after its subtree is quiet.
          if (subtreeQuiet(link.target)) return hand(link);
          continue;
        }
        if (!isActive(now) || now === "needs_user") return hand(link);
      }
      if (Date.now() >= deadline) {
        return {
          status: "timeout",
          waited_until: until,
          links: mine.flatMap((id) => {
            const link = book().links.find((l) => l.id === id);
            return link ? [hand(link)] : [];
          }),
          note: "Still running. You are woken when it finishes or asks; do not poll with sleeps.",
        };
      }
      await rest();
    }
  } finally {
    if (callId && calls.get(callId) === me) calls.delete(callId);
    // What this wait handed over, for a call whose answer turns out to be dropped unread.
    if (callId && me.handed.length) {
      handedBy.set(callId, me.handed);
      while (handedBy.size > HANDED_KEPT) handedBy.delete(handedBy.keys().next().value!);
    }
    for (const id of mine) {
      if (holds.get(holdKey(callerId, id)) !== me) continue;
      holds.delete(holdKey(callerId, id));
      const set = waiting.get(callerId);
      set?.delete(id);
      if (set && !set.size) waiting.delete(callerId);
    }
    wakeTick();
  }
}

/**
 * The caller's own row for a call the app can watch: its `wait_session` row
 * still in flight. Only a lone one is certainly this wait's, so a second call in
 * parallel leaves this off rather than watching another call's row.
 */
function waitRowOf(callerId: string): string | undefined {
  const rows = (useApp.getState().threads[callerId]?.items ?? []).filter(
    (item) =>
      item.kind === "tool" &&
      (item.status === "pending" || item.status === "in_progress") &&
      orchestrationTool(item.title) === "wait_session",
  );
  return rows.length === 1 ? rows[0]!.id : undefined;
}

/** Whether that row still runs: one that ended, or that a reload took away, says the call is gone. */
function waitRowRuns(callerId: string, row: string): boolean {
  const item = (useApp.getState().threads[callerId]?.items ?? []).find((i) => i.id === row);
  return !!item && item.kind === "tool" && (item.status === "pending" || item.status === "in_progress");
}

/**
 * The engine says a call was dropped before it answered (a harness's own tool
 * timeout, a cancel, the MCP server closing): its wait ends where it is, without
 * marking anything heard, so a child that settles comes as the ordinary wake and
 * an answer already sent — which the engine drops unread — is handed again.
 * `_rustcode/orchestration/cancelled`.
 */
export function orchestrationCallDropped(params: JsonObject): void {
  const callId = typeof params.callId === "string" ? params.callId : "";
  if (!callId) return;
  const session = typeof params.sessionId === "string" ? params.sessionId : undefined;
  const live = calls.get(callId);
  if (live && (session === undefined || live.caller === session)) {
    live.ended = "dropped";
    live.nudge();
    return;
  }
  const marks = handedBy.get(callId);
  if (!marks?.length) return;
  handedBy.delete(callId);
  setBook((b) => unheard(b, marks));
  wakeTick();
}

// ---------------------------------------------------------------- waking a parent

/** What the wake rules need of the app, read once: no polling, no engine calls. */
function wakeContext(now: number): WakeContext {
  const s = useApp.getState();
  const states: Record<string, LinkState> = {};
  const pending: Record<string, string> = {};
  const threads: Record<string, { title: string; running: boolean }> = {};
  /** Each finished link's target's latest reply, for a link that holds no answer. */
  const answers: Record<string, string> = {};
  const said: Record<string, readonly string[]> = {};
  // A parent and a child are both asked about: the parent hears, and the child's own subtree has
  // to be quiet before it does.
  const remember = (id: string) => {
    if (threads[id]) return;
    const thread = ruleThread(s, id);
    if (thread) threads[id] = { title: thread.title, running: !!s.threads[id]?.running };
  };
  for (const link of s.links.links) {
    const target = s.threads[link.target];
    states[link.id] = shownState(link, !!target?.permissions.length);
    const request = target?.permissions[0];
    if (request) pending[link.id] = String(request.rpcId);
    remember(link.orchestrator);
    remember(link.target);
    if (!answers[link.target]) answers[link.target] = latestReply(target?.items ?? []);
  }
  for (const [id, ids] of waiting) said[id] = [...ids];
  return { states, pending, waiting: said, threads, now, answers };
}

/** A link with the target's last reply when it holds no answer of its own. */
function filledWith(link: Link, reply: string): Link {
  if (!reply.trim()) return link;
  const { text, truncated } = capped(reply);
  return { ...link, answer: text, truncated };
}

/**
 * Whether a thread owes its parent nothing more: no child of it works, none
 * waits to be told. Also what the app calls a link that is still going long
 * after it settled (state/waiting.ts).
 */
export function subtreeQuiet(id: string): boolean {
  return quietTree(book(), wakeContext(Date.now()), id);
}

/**
 * Delivers what each parent should hear — children settling, children waiting
 * on the user — once per child, and schedules the re-check a coalescing window
 * needs. Called when a link settles, when a child asks the user, and when a
 * wait returns; nothing here polls.
 */
export function wakeTick(): void {
  clearTimeout(wakeTimer);
  wakeTimer = undefined;
  const now = Date.now();
  if (!useApp.getState().orchestration) return;
  const ctx = wakeContext(now);
  // A link that settled without an answer (its reply's rows came a moment later) is completed
  // from the thread's own last reply before anyone reads it.
  const filled = fillAnswers(book(), ctx.answers ?? {});
  if (filled !== book()) setBook(() => filled);
  // A parent that is gone for good: its children's results are never delivered.
  const orphaned = orphanedLinks(filled, ctx);
  if (orphaned.length) {
    setBook((b) => orphaned.reduce((acc, l) => updateLink(acc, l.id, { notifiedAt: now }), b));
  }
  // A settled link a newer one to the same thread replaced: the parent messaged that thread since, so
  // this answer is out of date. Mark it heard instead of waking the parent with it (it would arrive
  // long after the answer the parent already read).
  const superseded = supersededLinks(book(), ctx);
  if (superseded.length) {
    setBook((b) => superseded.reduce((acc, l) => updateLink(acc, l.id, { notifiedAt: now }), b));
  }
  for (const wake of wakesDue(book(), ctx)) sendWake(wake, ctx, now);
  const at = nextWakeAt(book(), ctx);
  if (at !== null) scheduleWake(Math.max(250, at - now + 50));
}

/** Re-checks once, later: a coalescing window ending, or a parent still loading. */
function scheduleWake(ms: number): void {
  clearTimeout(wakeTimer);
  wakeTimer = setTimeout(() => wakeTick(), ms);
}

/**
 * One message per parent, in a turn of its own: queued behind a running turn
 * (never interrupting its tools), started at once when the thread is idle. Links
 * are marked before it is sent, so a wake turn's own activity cannot deliver the
 * same result twice. The wake turn carries no link, so waking a thread that is
 * itself a child does not settle its own link in its parent's book while that
 * turn runs: a wake is a message, not a delegation.
 *
 * A parent the app cannot see running (its history is still replaying, or it is
 * not open yet) is never sent a prompt: a second prompt into a turn can interrupt
 * the tools it is running (a parent's own `wait_session`). The wake waits, unmarked,
 * until the app knows where the turn is.
 */
function sendWake(wake: Wake, ctx: WakeContext, now: number): void {
  const state = useApp.getState();
  const parent = state.threads[wake.parent];
  const known = state.sessions.some((s) => s.id === wake.parent);
  if (!parent || parent.loading) {
    // Opening it is the app's job; the wake waits rather than landing in a turn it cannot see.
    if (!parent && known) void openThread(wake.parent);
    const tries = (wakeWaits.get(wake.parent) ?? 0) + 1;
    wakeWaits.set(wake.parent, tries);
    if (tries <= WAKE_WAIT_TRIES) scheduleWake(1_000);
    return;
  }
  wakeWaits.delete(wake.parent);
  const text = wakeMessage(wake, now);
  setBook((b) => markWoken(b, wake, ctx, now));
  if (parent.running) {
    updateThread(wake.parent, (t) => ({
      ...t,
      queue: [...t.queue, { id: localId("queued"), text, wake: true }],
    }));
    return;
  }
  void runPrompt(wake.parent, text, undefined, { wake: true });
}

/**
 * A link the parent was just handed (a `wait_session` answer, or a pending
 * question it was told about): mark it heard, so no wake repeats it. The mark is
 * returned so a call whose answer is dropped unread can take it back.
 */
function markHeard(link: Link, state: LinkState, now: number): HeardMark {
  const request = useApp.getState().threads[link.target]?.permissions[0]?.rpcId;
  const asked = request === undefined ? undefined : String(request);
  const change =
    state === "needs_user" && asked !== undefined ? { askedRequest: asked } : { notifiedAt: now };
  setBook((b) => updateLink(b, link.id, change));
  return { id: link.id, ...change };
}

// ---------------------------------------------------------------- link turns

export function linkStarted(id: string) {
  setBook((b) => updateLink(b, id, { state: "working" }));
}

/**
 * What a turn answered: its last reply with something in it, bounded, and how
 * many files it changed. An assistant row an empty chunk left behind (`"\n"`
 * between two calls) is skipped, so it cannot win over the reply before it.
 */
function answerOf(items: TranscriptItem[], failed?: string): TurnAnswer {
  const { text, truncated } = capped(latestReply(items));
  return {
    text: failed ? (text ? `${text}\n\n${failed}` : failed) : text,
    truncated,
    filesChanged: turnCompletion(items, items.length)?.files.length ?? 0,
  };
}

/**
 * A turn's answer, and the thread's last reply when the turn's own rows hold
 * none. The rows of a reply reach the app a moment after the prompt returns (and
 * a thread whose rows were replayed can leave the turn's window behind the
 * reply), so a link must never settle on an empty answer while the thread knows
 * one: the parent reads that answer, and nothing would ever fill it in.
 */
function answerFor(thread: string, items: TranscriptItem[], failed?: string, fallback = true): TurnAnswer {
  const says = answerOf(items, failed);
  if (!fallback || says.text.trim()) return says;
  const { text, truncated } = capped(latestReply(useApp.getState().threads[thread]?.items ?? []));
  return text ? { ...says, text, truncated } : says;
}

/** The link's turn ended: its answer is that turn's last reply, never an earlier one. */
export function linkFinished(
  id: string,
  thread: string,
  outcome: "completed" | "cancelled" | "failed",
  failed: string | undefined,
  items: TranscriptItem[],
) {
  const now = Date.now();
  // A stopped link is not an answer: it takes only what its own turn said.
  const says = answerFor(thread, items, failed, outcome !== "cancelled");
  setBook((b) =>
    updateLink(b, id, {
      state: outcome === "cancelled" ? "cancelled" : outcome === "failed" ? "failed" : "done",
      answer: says.text,
      truncated: says.truncated,
      filesChanged: says.filesChanged,
      settledAt: now,
    }),
  );
  wakeTick();
}

/**
 * A turn of `thread` ended, whichever way it was started. A parent's wake waits
 * for the thread's own subtree to be quiet, so a link that settled but was not
 * reported yet takes this turn's answer instead of the one that ended first
 * (host.ts `wait` reads the same link). A link already reported is re-armed once
 * when this turn was started by the wake from the thread's own child, so the
 * real answer reaches the parent too.
 */
export function threadTurnEnded(
  thread: string,
  woken: boolean,
  outcome: "completed" | "cancelled" | "failed",
  failed: string | undefined,
  items: TranscriptItem[],
) {
  // A stopped turn is not an answer: the link turns record their own, and nothing else does. Its
  // end still matters to a parent: this thread no longer runs, so what it owes may be due now.
  if (outcome !== "cancelled") {
    const now = Date.now();
    const says = answerFor(thread, items, outcome === "completed" ? undefined : failed);
    setBook((b) => restateLinks(b, thread, says, now, woken));
  }
  wakeTick();
}

/** A queued link message was removed from the target's queue. */
export function queuedLinkDropped(id: string) {
  const link = book().links.find((l) => l.id === id);
  if (link?.state !== "queued") return;
  setBook((b) => updateLink(b, id, { state: "cancelled", settledAt: Date.now() }));
  // Its siblings can now be the last ones working: look again at once.
  wakeTick();
}

/** Stop a link: its queued message goes, or its running turn is cancelled. The user's own turns there go on. */
export async function stopLink(id: string): Promise<boolean> {
  const link = book().links.find((l) => l.id === id);
  if (!link || !isActive(link.state)) return false;
  if (link.state === "queued") {
    updateThread(link.target, (t) => ({ ...t, queue: t.queue.filter((q) => q.link !== id) }));
    setBook((b) => updateLink(b, id, { state: "cancelled" }));
    return true;
  }
  await cancel(link.target);
  return true;
}

/** The threads whose work is being stopped right now: a cascade stops each one once. */
const stopping = new Set<string>();

/**
 * Stops the work `orchestrator` handed to other threads, and what those
 * threads handed on: stopping a thread takes its whole delegation tree with
 * it, because the thread above is no longer there to read the answers. A
 * normal finish does not cascade: a child that settles leaves its own children
 * running, they are woken and answer into the thread that started them.
 *
 * Each thread is stopped once ({@link stopping}), so a stop that also arrives
 * from a child's own cancel cannot loop or count twice. Returns how many links
 * it stopped, for a tool's answer to say.
 */
export async function stopLinksFrom(orchestrator: string): Promise<number> {
  if (stopping.has(orchestrator)) return 0;
  stopping.add(orchestrator);
  try {
    let stopped = 0;
    for (const link of book().links) {
      if (link.orchestrator !== orchestrator || !isActive(link.state)) continue;
      if (await stopLink(link.id)) stopped += 1;
    }
    return stopped;
  } finally {
    stopping.delete(orchestrator);
  }
}

/** Forget a finished link's chip. */
export function dismissLink(id: string) {
  setBook((b) => ({ ...b, links: b.links.filter((l) => l.id !== id || isActive(l.state)) }));
}

/** "Keep this": the fan-out's other threads are done, and the kept one goes to review. */
export async function keepFanOut(fanId: string, keep: string) {
  const group = book().fanOuts.find((f) => f.id === fanId);
  if (!group) return;
  const targets = group.links.flatMap((id) => book().links.find((l) => l.id === id)?.target ?? []);
  for (const target of targets) {
    if (target !== keep) await setThreadStatus(target, "done");
  }
  await setThreadStatus(keep, "review");
  setBook((b) => ({ ...b, fanOuts: b.fanOuts.map((f) => (f.id === fanId ? { ...f, kept: keep } : f)) }));
  toast("Kept. The other threads are marked done.", "success");
}

/** The links of a thread, both ways, for chips, banners and badges. */
export function linksOf(b: LinkBook, id: string) {
  return {
    driving: b.links.filter((l) => l.orchestrator === id).slice(-MAX_ACTIVE_LINKS * 2),
    drivenBy: [...b.links].reverse().find((l) => l.target === id && isActive(l.state)),
  };
}

export const projectLabel = (path: string) => projectName(path);
