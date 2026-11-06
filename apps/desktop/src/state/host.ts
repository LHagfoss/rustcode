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
import { type Harness, Method, projectName } from "../acp/types";
import { hasShell, shellCall } from "../lib/shell";
import { childTitle, titleFrom } from "../lib/title";
import { selectOption, unavailableReason, usableHarnesses } from "./harness";
import { accountLabelOf, accountNameOf, accountSignedOut, childAccountFor } from "./logins";
import {
  activeDescendants,
  activityOf,
  askingIds,
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
  markDelivered,
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
  queueWake,
  quietTree,
  type RuleThread,
  renderRead,
  restateLinks,
  sameProject,
  shownState,
  type TurnAnswer,
  unheard,
  updateLink,
  usageScope,
  type Wake,
  type WakeContext,
  type WakeReport,
  wakeIds,
  wakeMessage,
  wakeStillOwed,
  wakesDue,
} from "./orchestration";
import { delegationAllowed, effectiveMode, modeLabel, type PermissionMode } from "./permissions";
import { wakeDelivery } from "./queue";
import {
  parseQueueBook,
  pendingRows,
  persistable,
  QUEUES_KEY,
  type QueueBook,
  queueOf,
  withQueue,
} from "./queues";
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
  resumeQueue,
  runPrompt,
  searchThreads,
  setConfigOption,
  setPermissionMode,
  setThreadStatus,
  snoozeThread,
  statusOf,
  steer,
  steeringOf,
  switchThreadHarness,
  togglePin,
  turnState,
  updateThread,
  useApp,
} from "./store";
import {
  appendNotice,
  attachAutoAllowed,
  autoAllowedNoticeNeeded,
  localId,
  type QueuedPrompt,
  type TranscriptItem,
  turnCompletion,
} from "./thread";
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
  // The grant dialog names the caller and the request; a toast under it would repeat them.
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

async function permitted(caller: RuleThread, key: string, question: GrantAsk): Promise<Failure | null> {
  if (hasGrant(book(), caller.id, key)) return null;
  // The thread's access level answers for the user where it already allows the work: the same
  // policy that answers the harness's own approvals, so a Full-access thread is not re-asked here.
  const mode = permissionModeOf(useApp.getState(), caller.id);
  if (delegationAllowed(mode, question.kind)) {
    // Not silent: the call's own row says the thread did it on its own, and why. A notice would
    // both name the caller (the reader is already in that thread) and split the run of calls.
    const why = modeLabel(mode);
    const tool = TOOL_OF_KIND[question.kind];
    let attached = false;
    let needed = true;
    updateThread(caller.id, (t) => {
      const marked = attachAutoAllowed(t, tool, why);
      attached = marked.attached;
      // A start is shown by the card of the thread it started, so it needs no notice of its own.
      needed = autoAllowedNoticeNeeded(marked.state, tool);
      return marked.attached ? marked.state : t;
    });
    if (!attached && needed) {
      updateThread(caller.id, (t) =>
        appendNotice(t, `${ALLOWED_NOTICE[question.kind]} · allowed by ${why}`, "info"),
      );
    }
    // The call's row and its notice above are the record; no toast repeats an allowed call.
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
  const caller = ruleThread(useApp.getState(), typeof params.sessionId === "string" ? params.sessionId : "");
  if (!caller) return failureJson(failure("not_found", "the calling thread is not listed"));
  const call = parseCall(params);
  if (isFailure(call)) return failureJson(call);
  // A call that can create a thread or deliver a message may carry a retry key:
  // its answer is kept, so the same call retried after a dropped answer is
  // answered from there instead of acting twice (`docs/orchestrator.md`).
  const key = retryKey(params, call);
  if (!key) return perform(caller, call);
  return receipted(caller, call, key, () => perform(caller, call));
}

/** The tools a retry key applies to: the ones that can start or deliver twice. */
const KEYED_TOOLS: ReadonlySet<string> = new Set(["message_session", "start_session", "fan_out"]);

/** The retry key a request carries, when it is one the app keeps a receipt for. */
function retryKey(params: JsonObject, call: { tool: string }): { id: string; argsHash: string } | null {
  if (!KEYED_TOOLS.has(call.tool)) return null;
  const id = typeof params.clientRequestId === "string" ? params.clientRequestId : "";
  const argsHash = typeof params.argsHash === "string" ? params.argsHash : "";
  // The engine sends both; one without the other is a call with no receipt.
  return id && argsHash ? { id, argsHash } : null;
}

/** Calls with a retry key still running in this window, so a duplicate waits for the first. */
const inFlight = new Map<string, Promise<JsonObject>>();

/**
 * One call with a retry key, made safe to retry: the receipt the data store
 * keeps under the caller's key is checked before the app acts and written
 * right after it succeeds, so the same key and arguments return the recorded
 * answer (`replayed: true`) instead of starting or delivering twice, for a
 * week and across restarts. The same key with another action or other
 * arguments is refused. A duplicate still running here waits for the first
 * rather than asking the user again.
 */
async function receipted(
  caller: RuleThread,
  call: { tool: string },
  key: { id: string; argsHash: string },
  run: () => Promise<JsonObject>,
): Promise<JsonObject> {
  const scope = `${caller.id}\u0000${key.id}`;
  const running = inFlight.get(scope);
  if (running) return replayed(await running);
  // Reserved before the first await, so two duplicates cannot both act.
  const work = (async () => {
    const earlier = await receiptLookup(caller.id, call.tool, key);
    if (earlier.state === "replayed") return replayed(earlier.result);
    if (earlier.state === "conflict") {
      return failureJson(failure("invalid", "client_request_id reused with different arguments"));
    }
    const result = await run();
    // A refusal or a failure did nothing to record; a success is the receipt.
    if (!result.error) await receiptSave(caller.id, call.tool, key, result);
    return result;
  })();
  inFlight.set(scope, work);
  try {
    return await work;
  } finally {
    inFlight.delete(scope);
  }
}

/** A recorded answer, marked so an agent knows it is not new work. */
const replayed = (result: JsonObject): JsonObject => (result.error ? result : { ...result, replayed: true });

type Earlier = { state: "none" } | { state: "conflict" } | { state: "replayed"; result: JsonObject };

/**
 * What the store remembers under a caller's retry key. A store that cannot
 * answer counts as nothing remembered: a receipt is a safety net, and losing
 * it must not stop the call the user allowed.
 */
async function receiptLookup(
  caller: string,
  action: string,
  key: { id: string; argsHash: string },
): Promise<Earlier> {
  try {
    const answer = await request<JsonObject>(
      Method.dataReceiptGet,
      { caller, clientRequestId: key.id, action, argsHash: key.argsHash },
      20_000,
    );
    if (answer.state === "conflict") return { state: "conflict" };
    if (answer.state === "replayed") {
      return { state: "replayed", result: (answer.result ?? {}) as JsonObject };
    }
    return { state: "none" };
  } catch {
    return { state: "none" };
  }
}

/** Writes the receipt of a call that succeeded; a store that cannot keep it does not fail the call. */
async function receiptSave(
  caller: string,
  action: string,
  key: { id: string; argsHash: string },
  result: JsonObject,
): Promise<void> {
  try {
    await request(
      Method.dataReceiptPut,
      { caller, clientRequestId: key.id, action, argsHash: key.argsHash, result },
      20_000,
    );
  } catch {
    // The call happened either way; a lost receipt only costs a safe retry.
  }
}

/** A read that asked for every project and may not have it: say so, rather than answer half. */
function reachRefused(): JsonObject {
  return failureJson(
    failure(
      "not_allowed",
      "reading across projects is off; turn on “Let agents read threads in other projects” in Settings › Agents",
    ),
  );
}

/** Every thread the app lists, as the rules see it. */
function allThreads(): RuleThread[] {
  const st = useApp.getState();
  return st.sessions.flatMap((x) => ruleThread(st, x.id) ?? []);
}

/** The answer to one call, once its retry key (if any) has been checked. */
async function perform(
  caller: RuleThread,
  call: Exclude<ReturnType<typeof parseCall>, Failure>,
): Promise<JsonObject> {
  const acrossProjects = () => useApp.getState().readAcrossProjects;
  const threads = allThreads;
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
      return listProjects(useApp.getState());
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
    case "read_activity": {
      // A read of the store, like thread_usage: the project rule applies (any
      // project while the setting allows it), and no approval is asked.
      const target = ruleThread(useApp.getState(), call.target);
      const refused = checkMessage(caller, target, book(), true, acrossProjects());
      if (refused || !target) return failureJson(refused ?? failure("not_found", "no such thread"));
      try {
        return await request<JsonObject>(
          Method.dataActivity,
          {
            sessionId: target.id,
            ...(call.after ? { after: call.after } : {}),
            limit: call.limit,
          },
          20_000,
        );
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
  const signedOut = useApp.getState().signedOutLogins;
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
        // The logins start_session and fan_out may name; the default login has no id.
        ...(h.accounts?.length
          ? {
              accounts: h.accounts.map((a) => ({
                id: a.id,
                name: a.name,
                auth: a.auth,
                // The plan read saw this login signed out: naming it is refused.
                ...(accountSignedOut(h.id, a.id, signedOut) ? { signed_out: true } : {}),
              })),
            }
          : {}),
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
  const openError = await openThread(target.id);
  if (openError) return failureJson(failure("target_unavailable", openError));
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
function harnessFor(name: string | undefined): Harness | Failure {
  const snapshot = useApp.getState().harnesses;
  const id = name ?? snapshot?.default.harness ?? "rustcode";
  const h = snapshot?.harnesses.find((x) => x.id === id || x.name.toLowerCase() === id.toLowerCase());
  if (!h) return failure("invalid", `no harness “${id}”; the ones that can run now are ${usableIds()}`);
  const reason = unavailableReason(h);
  if (reason)
    return failure("not_allowed", `${h.name} cannot run a thread: ${reason}. Usable now: ${usableIds()}`);
  return h;
}

/**
 * The account a delegated thread runs on: the named login the agent asked for
 * (`start_session`'s or a `fan_out` entry's `account`), checked against that
 * harness's accounts, else the parent's own on the same harness, else the one
 * a new thread would start on (`state/logins.ts`, `docs/harnesses.md`).
 */
function accountFor(caller: RuleThread, harness: Harness, asked?: string): string | null | Failure {
  const s = useApp.getState();
  const pick = childAccountFor(
    harness,
    asked,
    {
      harness: caller.harness,
      account: s.threads[caller.id]?.harness?.account ?? s.sessions.find((x) => x.id === caller.id)?.account,
    },
    s.signedOutLogins,
  );
  if (!("refused" in pick)) return pick.account;
  const refusal = pick.refused;
  switch (refusal.kind) {
    case "ambiguous":
      return failure(
        "invalid",
        `${harness.name} has more than one account matching “${refusal.asked}” (${refusal.matches.join(", ")}); name one of those ids`,
      );
    case "signedOut":
      return failure(
        "not_allowed",
        `${harness.name} account “${refusal.name}” (${refusal.account}) is signed out; sign it in in Settings › Agents or pick another login (${refusal.available.join(", ")})`,
      );
    case "unknown":
      return failure(
        "invalid",
        refusal.available.length
          ? `${harness.name} has no account “${refusal.asked}”; its named accounts are ${refusal.available.join(", ")}`
          : `${harness.name} has no named accounts; it runs on its default login, so leave account out`,
      );
  }
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

/** "Claude Code · Work · opus · high" for the approval card: what will actually run. */
const choiceLabel = (name: string, account?: string, model?: string, effort?: string) =>
  [name, account, model, effort].filter(Boolean).join(" · ");

/** The model, effort and account the new thread reports it runs on, for the caller's answer. */
function ranWith(id: string): JsonObject {
  const options = useApp.getState().threads[id]?.configOptions ?? [];
  const model = selectOption(options, "model")?.current;
  const effort = selectOption(options, "effort")?.current;
  const account = useApp.getState().threads[id]?.harness?.account;
  return {
    ...(model ? { model } : {}),
    ...(effort ? { effort } : {}),
    ...(account ? { account } : {}),
  };
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
  // Resolved before the user is asked, so the card names the login and the thread gets exactly it.
  const account = accountFor(caller, harness, call.account);
  if (isFailure(account)) return failureJson(account);
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
    harnesses: [choiceLabel(harness.name, accountLabelOf(harness, account), model, call.effort)],
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
      account,
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
    account: account ?? undefined,
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
  const chosen: { harness: Harness; model?: string; effort?: string; account: string | null }[] = [];
  const targets = new Set<string>();
  for (const entry of call.harnesses) {
    const h = harnessFor(entry.harness);
    if (isFailure(h)) return failureJson(h);
    const model = await modelFor(h.id, entry.model);
    if (isFailure(model)) return failureJson(model);
    const account = accountFor(caller, h, entry.account);
    if (isFailure(account)) return failureJson(account);
    // Two entries that resolve to the same target would run one task twice, and the engine
    // has only compared the arguments as they came in ('Work' and 'work' are one login).
    const key = [h.id, model ?? "", entry.effort ?? "", account ?? ""].join("\u0000");
    if (targets.has(key)) {
      return failureJson(
        failure(
          "invalid",
          `harnesses resolves to the same target twice: ${choiceLabel(h.name, accountLabelOf(h, account), model, entry.effort)}; give each entry a different harness, model, effort or account`,
        ),
      );
    }
    targets.add(key);
    chosen.push({ harness: h, model, effort: entry.effort, account });
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
    harnesses: chosen.map((h) =>
      choiceLabel(h.harness.name, accountLabelOf(h.harness, h.account), h.model, h.effort),
    ),
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
    // The account names the thread too when it has one, so two logins of one harness read apart.
    const threadTitle = [title, h.harness.name, accountNameOf(h.harness, h.account)]
      .filter(Boolean)
      .join(" · ");
    try {
      const id = await newThread(project, h.harness.id, {
        model: h.model,
        effort: h.effort,
        account: h.account,
        title: threadTitle,
        task: call.prompt,
        parent: caller.id,
        access,
      });
      const link = addLink({
        orchestrator: caller.id,
        target: id,
        orchestratorTitle: caller.title,
        targetTitle: threadTitle,
        message: call.prompt,
        state: "working",
        started: true,
        harness: h.harness.id,
        account: h.account ?? undefined,
        fanOut: fanId,
      });
      linkIds.push(link);
      out.push({
        session_id: id,
        link_id: link,
        harness: h.harness.id,
        access: accessOf(id),
        ...ranWith(id),
      });
      deliver(id, delivered(call.prompt, caller.title), link);
    } catch (error) {
      // The reason, not the name of the error class: the harness says why it could not start.
      out.push({ harness: h.harness.id, ...failureJson(failure("failed", errorText(error))) });
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

// ---------------------------------------------------------------- the queue across restarts

/**
 * A thread's queue is the app's memory, so a restart used to lose it: the messages a stopped turn
 * held, the follow-ups behind a running one, and a child's wake waiting for the parent's turn to
 * end. It is kept in app storage per thread id while the app runs and read back when a thread is
 * opened again (state/queues.ts), so a quit, a crash or an update does not take an instruction or a
 * child's result with it.
 *
 * Nothing is written for a thread the app has not finished loading: opening a thread starts it
 * empty, and those empty frames are not the user deleting their queue.
 */

/** The book as it is in memory: read once, on the first change after start-up. */
let queueBook: QueueBook | undefined;
/** The book as storage last took it: a change that serialises the same is not written again. */
let keptQueues: string | undefined;
/** Putting a saved queue back changes the store, which notifies again: the nested pass is skipped. */
let restoring = false;
/** Whether this window is watching the store for queue changes: installed once, whatever reconnects. */
let watchingQueues = false;
/** At most this many threads are opened at start-up for the queue each kept. */
const RESTORE_MAX = 8;

const storedQueues = (): string | null => {
  try {
    return localStorage.getItem(QUEUES_KEY);
  } catch {
    // No storage (a private window): the queue is this window's only, as before.
    return null;
  }
};

const keepQueues = (text: string): void => {
  try {
    localStorage.setItem(QUEUES_KEY, text);
  } catch {
    // A web view whose storage is full keeps the queue in memory instead.
  }
};

/**
 * Takes the queues the last run left, and watches this one: called once at start-up, after the
 * thread list is in (state/store.ts `connect`). A thread that kept a queue is opened, so its rows
 * are put back as it loads and, when the thread is idle, start running again — an instruction the
 * user queued before the restart is not left sitting in storage until they think to look.
 */
export function restoreQueuedThreads(): void {
  if (!queueBook) queueBook = parseQueueBook(storedQueues());
  if (!watchingQueues) {
    watchingQueues = true;
    // The state is read from the store, never diffed against a `previous` one: the store folds a run
    // of engine lines into one notification, so the pair it is given can already hold the change.
    useApp.subscribe((state) => queuesChanged(state));
  }
  const s = useApp.getState();
  const known = new Set(s.sessions.map((session) => session.id));
  const waiting = Object.keys(queueBook.threads).filter(
    (id) => known.has(id) && !s.threads[id]?.loaded && pendingRows(queueBook!, id).length > 0,
  );
  for (const id of waiting.slice(0, RESTORE_MAX)) void openThread(id);
}

/** Between tests: the queues are read again from storage, as a window that just started would. */
export function resetQueues(): void {
  queueBook = undefined;
  keptQueues = undefined;
  restoring = false;
  restoredQueues.clear();
  watchedQueues.clear();
  steeredLinks.clear();
}

/** The threads whose saved queue this window has already put back. */
const restoredQueues = new Set<string>();
/** The queue array each thread had when it was last written, to notice a change without a diff. */
const watchedQueues = new Map<string, readonly QueuedPrompt[]>();

/**
 * One state change: a thread that has just finished loading gets its queue back, and a queue that
 * changed is written down. A thread that is loading is forgotten again, so a thread opened twice in
 * one run (a repair) reads its queue back the second time too.
 *
 * Only a loaded thread's queue is written — the empty frames a thread passes through while it loads
 * are not rows the user removed — and a row that left a loaded thread's queue was sent, so its id is
 * recorded and a restart cannot send it again.
 */
function queuesChanged(state: AppState): void {
  if (restoring) return;
  const loaded = queueBook;
  if (!loaded) return;
  const now = Date.now();
  let next = loaded;
  const drain: string[] = [];
  for (const [id, thread] of Object.entries(state.threads)) {
    if (!thread.loaded || thread.loading) {
      restoredQueues.delete(id);
      watchedQueues.delete(id);
      continue;
    }
    if (!restoredQueues.has(id)) {
      // First sight of it since it loaded: what it kept is put back beside anything queued meanwhile.
      restoredQueues.add(id);
      const rows = restorable(id, thread.queue);
      if (rows.length) {
        restoring = true;
        try {
          updateThread(id, (t) => ({ ...t, queue: [...t.queue, ...rows] }));
        } finally {
          restoring = false;
        }
        drain.push(id);
      }
      watchedQueues.set(id, useApp.getState().threads[id]?.queue ?? thread.queue);
      continue;
    }
    if (watchedQueues.get(id) === thread.queue) continue;
    watchedQueues.set(id, thread.queue);
    next = withQueue(next, id, thread.queue, now);
  }
  if (next !== loaded) {
    queueBook = next;
    const text = JSON.stringify(persistable(next));
    if (text !== keptQueues) {
      keptQueues = text;
      keepQueues(text);
    }
  }
  // Started after this pass, not inside it: a turn changes the store, and that change is a new pass.
  for (const id of drain) queueMicrotask(() => drainQueue(id));
}

/** The head of a restored queue starts a turn, once the thread is idle. */
function drainQueue(id: string): void {
  const thread = useApp.getState().threads[id];
  if (thread && !thread.running && !thread.queueHeld) void resumeQueue(id);
}

/**
 * The rows a thread's saved queue still owes, less anything it already has: the same id is never
 * twice, a wake whose links another waiting row already reports is the copy that was lost, and a
 * wake whose children the parent has heard about is nothing to run (`wakeStillOwed`).
 */
function restorable(id: string, queue: readonly QueuedPrompt[]): QueuedPrompt[] {
  const saved = queueBook;
  if (!saved) return [];
  const here = new Set(queue.map((item) => item.id));
  const carried = new Set(queue.flatMap((item) => (item.wake ? (item.wakeLinks ?? []) : [])));
  const ctx = wakeContext(Date.now());
  const out: QueuedPrompt[] = [];
  for (const item of queueOf(pendingRows(saved, id))) {
    if (here.has(item.id)) continue;
    const links = item.wakeLinks ?? [];
    if (item.wake) {
      if (links.length && links.every((link) => carried.has(link))) continue;
      if (!wakeStillOwed(book(), { links, asking: item.wakeAsking ?? [] }, ctx)) continue;
    }
    out.push(item);
    for (const link of links) carried.add(link);
  }
  return out;
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
  // What each parent's queued wake rows already report: their turn delivers it, marked when it starts.
  const queued: Record<string, string[]> = {};
  for (const [id, thread] of Object.entries(s.threads)) {
    for (const item of thread.queue) {
      if (!item.wake || !item.wakeLinks?.length) continue;
      const list = queued[id] ?? [];
      queued[id] = list;
      list.push(...item.wakeLinks);
    }
  }
  return { states, pending, waiting: said, queued, threads, now, answers };
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
 * needs. Called when a link settles, when a child asks the user, when a wait
 * returns, and once at start-up after the book is read from the data store
 * (state/store.ts `connect`); nothing here polls.
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
  // A parent that is gone for good: its children's results are never delivered. Only when the app has
  // a thread list to judge by: an engine that could not list threads at start-up must not retire every
  // unread result in the book.
  const orphaned = useApp.getState().sessions.length ? orphanedLinks(filled, ctx) : [];
  if (orphaned.length) {
    setBook((b) => orphaned.reduce((acc, l) => updateLink(acc, l.id, { notifiedAt: now }), b));
  }
  // A settled link a newer one to the same thread replaced is not reported: that answer is out of date
  // (wakesDue's own rule). It is deliberately NOT marked heard. The successor may still be stopped or
  // interrupted by a restart without ever answering, and then this is the last real answer the parent
  // has; a permanent mark here would lose it for good.
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
 * (never interrupting its tools), started at once when the thread is idle. The wake turn carries no
 * link, so waking a thread that is itself a child does not settle its own link in its parent's book
 * while that turn runs: a wake is a message, not a delegation.
 *
 * A parent the app cannot see running (its history is still replaying, or it is
 * not open yet) is never sent a prompt: a second prompt into a turn can interrupt
 * the tools it is running (a parent's own `wait_session`). The wake waits, unmarked,
 * until the app knows where the turn is.
 *
 * A busy parent gets one wake turn for everything that finished, however many ticks
 * report it: a wake already waiting in its queue takes the new one in (the links it
 * carried are kept on the row), rather than queueing another turn that repeats
 * "1 child finished". A wake for an idle parent still starts at once.
 *
 * Nothing is marked here. A queued row marks its links when its turn starts (`wakeDelivered`,
 * called by `runPrompt`): the queue is this window's memory, and a mark written at queue time would
 * survive a restart that the row does not, so the wake would never be delivered. For an idle parent
 * the mark happens in `runPrompt` too, just before the prompt goes out — early enough that the wake
 * turn's own activity cannot report the same result twice.
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
  // A steer for one of these questions is on its way to the running turn: this tick's copy would
  // repeat it, and the steer's own answer decides whether the row is still owed.
  if (wake.asking.length && wake.asking.every((link) => steeredLinks.has(link.id))) return;
  const wakeLinks = wakeIds(wake);
  const wakeAsking = askingIds(wake);
  if (parent.running) {
    // A child that needs the user is read by the running turn when the harness takes a live steer
    // that leaves its tools running (queue.ts `wakeDelivery`); otherwise it waits in the queue.
    if (wakeDelivery(steeringOf(parent), turnState(parent), wake.asking.length) === "steer") {
      void steerWake(wake, ctx, now);
      return;
    }
    queueWakeRow(wake, ctx, now);
    return;
  }
  void runPrompt(wake.parent, wakeMessage(wake, now), undefined, {
    wake: true,
    wakeLinks,
    wakeAsking,
  });
}

/** A wake as a row in its parent's queue: folded into the wake already waiting, or a row of its own. */
function queueWakeRow(wake: Wake, ctx: WakeContext, now: number): void {
  updateThread(wake.parent, (t) => ({
    ...t,
    queue: queueWake(t.queue, wake, book(), ctx, now, localId("queued")),
  }));
}

/** The links a wake being steered names, so a second tick in that moment cannot steer them again. */
const steeredLinks = new Set<string>();

/**
 * A child's request, read by the parent's running turn: the wake goes in as a steer, so the parent
 * hears it now and no tool call is interrupted, no turn is stopped and no second turn is queued. Its
 * links are marked heard once the engine has taken it — the same marks a queued row writes when its
 * turn starts — so nothing reports them again. A steer the engine refuses (the turn ended, or began
 * waiting on the user, meanwhile) leaves the wake in the queue, which delivers it as before.
 */
async function steerWake(wake: Wake, ctx: WakeContext, now: number): Promise<void> {
  const links = wakeIds(wake);
  for (const link of links) steeredLinks.add(link);
  try {
    if (await steer(wake.parent, wakeMessage(wake, now))) {
      wakeDelivered(links, askingIds(wake));
      return;
    }
  } finally {
    for (const link of links) steeredLinks.delete(link);
  }
  queueWakeRow(wake, ctx, now);
}

/**
 * A queued wake row's turn is starting: what its words promised is heard from here
 * (`markDelivered`), not from `sendWake`. A link it named as asking that finished meanwhile is left
 * owed, so its finish comes as its own wake. Idempotent, so a row that starts twice cannot mark twice.
 */
export function wakeDelivered(links: readonly string[] | undefined, asking: readonly string[]): void {
  if (!links?.length) return;
  const s = useApp.getState();
  const want = new Set(links);
  const states: Record<string, LinkState> = {};
  const pending: Record<string, string> = {};
  for (const link of s.links.links) {
    if (!want.has(link.id)) continue;
    const target = s.threads[link.target];
    states[link.id] = shownState(link, !!target?.permissions.length);
    const request = target?.permissions[0];
    if (request) pending[link.id] = String(request.rpcId);
  }
  const asked = new Set(asking);
  const reported: WakeReport = { settled: links.filter((id) => !asked.has(id)), asking: asked };
  const ctx: WakeContext = { states, pending, waiting: {}, threads: {}, now: Date.now() };
  setBook((b) => markDelivered(b, reported, ctx, ctx.now));
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
}

/** The links of a thread, both ways, for chips, banners and badges. */
export function linksOf(b: LinkBook, id: string) {
  return {
    driving: b.links.filter((l) => l.orchestrator === id).slice(-MAX_ACTIVE_LINKS * 2),
    drivenBy: [...b.links].reverse().find((l) => l.target === id && isActive(l.state)),
  };
}

export const projectLabel = (path: string) => projectName(path);
