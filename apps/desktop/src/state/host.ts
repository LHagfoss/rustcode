import {
  checkedMachine,
  fanOutOnMachine,
  listHarnessesOn,
  listMachines,
  machineFields,
  machineOfThread,
  machineThread,
  managedOn,
  projectsOnMachine,
  resolveMachine,
  scopedTarget,
  sessionsOnMachine,
  startOnMachine,
  threadNamedOn,
  waitMachine,
} from "./hostMachines";

export {
  clonedOn,
  forgetMachineProjectsForTests,
  localMachine,
  type MachineRef,
  machineFields,
  machineProjects,
  machineUnavailable,
  projectOnMachine,
  repositories,
  requestOn,
  resolveMachine,
} from "./hostMachines";

import { isLoaded, isLoading } from "./thread";
// The app's side of threads starting and messaging threads: it answers the
// engine's `_rustcode/orchestration` requests by checking the rules
// (state/orchestration.ts), asking the user, creating threads and
// delivering messages through the ordinary turn queue, and reporting each
// link turn's answer. After ThreadMessaging.swift (M1), plus v2's
// `read_session`, `start_session` and `fan_out` (docs/orchestrator.md).
//
// The store imports this module and this module imports the store; nothing
// here touches the store at load time.

import { type Json, type JsonObject, obj, RpcError } from "../acp/jsonrpc";
import { delegateParent, Method } from "../acp/types";
import { readStorage, writeStorage } from "../lib/storage";
import { sessionsListed } from "./engine";
import { cleanupWorktree, handoffWorktree, mergeWhenGreen, worktreeStatus } from "./hostDelegates";
import {
  answerRequest,
  configureSession,
  interruptSession,
  listPendingRequests,
  manageQueue,
  message,
  organizeThreads,
  read,
  transferSession,
  updateSession,
} from "./hostManage";
import {
  cloneProject,
  cloneProjectOn,
  createProject,
  readProject,
  readProjectOn,
  removeProject,
  updateProject,
} from "./hostProjects";
import { listHarnesses, listWorktrees, searchSessions } from "./hostReads";
import {
  createLoop,
  deleteScheduledTask,
  listPullRequests,
  listScheduledTasks,
  runScheduledTaskNow,
  scheduleTask,
  threadStatus,
} from "./hostSchedule";
import { activityFrom, fanOut, start, wait } from "./hostStart";
import {
  accountsNeedApproval,
  activityOf,
  advanceLink,
  askingIds,
  capped,
  checkMessage,
  checkQueryData,
  contextNote,
  type DelegateLimits,
  emptyBook,
  type Failure,
  failQueuedLinks,
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
  type LinkActivity,
  type LinkBook,
  type LinkState,
  latestReply,
  listSessions,
  MAX_DELEGATES_PER_THREAD,
  markDelivered,
  mergeBooks,
  nextWakeAt,
  orphanedLinks,
  parseBook,
  parseCall,
  prune,
  queueWake,
  quietBelow,
  quietTree,
  type RuleThread,
  reopenLinks,
  restateLinks,
  shownState,
  type TurnAnswer,
  updateLink,
  usageScope,
  type Wake,
  type WakeContext,
  type WakeReport,
  waitEnded,
  waitStarted,
  wakeIds,
  wakeMessage,
  wakeStillOwed,
  wakesDue,
} from "./orchestration";
import { revokeThreadGrant } from "./permissionGrants";
import { delegationAllowed, modeLabel } from "./permissions";
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
  dropInternetWait,
  dropLimitWait,
  errorText,
  openThread,
  permissionModeOf,
  releaseLimitWaitLink,
  request,
  resumeQueue,
  runPrompt,
  sessionById,
  setThreadStatus,
  statusOf,
  steer,
  steeringOf,
  toast,
  turnState,
  updateThread,
  useApp,
} from "./store";
import {
  appendNotice,
  attachAutoAllowed,
  autoAllowedNoticeNeeded,
  isRunning,
  localId,
  type QueuedPrompt,
  type TranscriptItem,
  turnCompletion,
} from "./thread";
import { isRemoteThread, splitThreadKey } from "./threadKeys";

export {
  answerStatusLocally,
  delegateStatusInputs,
  mergeResultNotice,
  readDelegatePull,
} from "./hostDelegates";
export { deliver, manageTarget, targetMachine } from "./hostManage";
export { noProviderPullList } from "./hostSchedule";
export { activityFrom, orchestrationCallDropped, stateOf } from "./hostStart";

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
    const raw = readStorage(BOOK_KEY);
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
  // Not persisted in a private window.
  writeStorage(BOOK_KEY, text(book));
}

/** Writes the book to the store once the changes settle; only when it changed. */
function scheduleSave() {
  if (engineLinks !== true) return;
  clearTimeout(saveTimer);
  saveTimer = setTimeout(() => void flushSave(), SAVE_DEBOUNCE_MS);
}

/**
 * Forgets the engine's copy of the book and drops a save still waiting to go: a test's debounced
 * save otherwise fires after the next test has no engine and prints "the link book was not saved".
 */
export function resetLinkBook(): void {
  clearTimeout(saveTimer);
  saveTimer = undefined;
  engineLinks = undefined;
  savedText = undefined;
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
  // Both copies are from before this connect: nothing left active resumes,
  // except a link the engine itself is running the turn of. It started with no
  // app attached and its turn goes on; the engine settles it in the store.
  const engineLive = new Set(
    Array.isArray(answer.engineLive)
      ? answer.engineLive.filter((id): id is string => typeof id === "string")
      : [],
  );
  const now = Date.now();
  const merged = interruptActive(
    mergeBooks(interruptActive(shared, now, engineLive), book()),
    now,
    engineLive,
  );
  useApp.setState({ links: prune(merged) });
  keepLocally(prune(merged));
  savedText = text(interruptActive(shared, now, engineLive));
  // A link this window started while the store was answering is new to it.
  if (text(prune(merged)) !== text(prune(interruptActive(shared, now, engineLive)))) scheduleSave();
  if (engineLive.size) watchEngineLinks();
}

/** How often the store is read for a link the engine runs, until it settles. */
const ENGINE_LINK_CHECK_MS = 3000;
let engineWatch: ReturnType<typeof setTimeout> | undefined;

/**
 * A link the engine runs the turn of settles in the store, not here: read the
 * store while one is active, and take what settled (a settled record beats one
 * that did not, as `mergeBooks` decides).
 */
function watchEngineLinks() {
  if (engineWatch !== undefined) return;
  const check = async () => {
    engineWatch = undefined;
    if (!book().links.some((l) => l.engine === true && isActive(l.state))) return;
    try {
      const answer = await request<JsonObject>(Method.dataLinksLoad, {}, 10_000);
      const shared = parseBook(answer.book);
      if (shared) {
        const next = prune(mergeBooks(shared, book()));
        if (text(next) !== text(book())) {
          useApp.setState({ links: next });
          keepLocally(next);
        }
      }
    } catch (error) {
      console.error("the link book was not read from the data store", error);
    }
    if (book().links.some((l) => l.engine === true && isActive(l.state))) {
      engineWatch = setTimeout(check, ENGINE_LINK_CHECK_MS);
    }
  };
  engineWatch = setTimeout(check, ENGINE_LINK_CHECK_MS);
}

export function setBook(fn: (book: LinkBook) => LinkBook) {
  const next = prune(fn(useApp.getState().links));
  useApp.setState({ links: next });
  keepLocally(next);
  scheduleSave();
}

export const book = () => useApp.getState().links;

/** Settings › Permissions: forget one "Allow for This Thread" answer, so the thread asks again. */
export function forgetThreadGrant(caller: string, key: string) {
  setBook((b) => revokeThreadGrant(b, caller, key));
}

/** The two delegate limits (Settings › Orchestration): one thread's own, and the machine's total. */
export const delegateLimits = (): DelegateLimits => ({
  perThread: useApp.getState().threadLimit,
  total: useApp.getState().totalLimit,
});

/**
 * The calls that open a link run one at a time: each checks the book, asks the
 * user and only then writes the link, so two calls arriving together would
 * both see the same free slot and overshoot a limit. The queue is a promise
 * chain, so a call starts when the one before it has finished, failed or not.
 */
let linkGate: Promise<unknown> = Promise.resolve();
export function withLinkGate<T>(run: () => Promise<T>): Promise<T> {
  const next = linkGate.then(run, run);
  linkGate = next.then(
    () => undefined,
    () => undefined,
  );
  return next;
}

// A tab or window closing mid-turn still leaves the store with what happened.
if (typeof window !== "undefined") {
  window.addEventListener("beforeunload", () => void flushSave());
}

/**
 * The links each parent is blocked on in `wait_session` right now: a wake for
 * one of them would be a duplicate, so it is held back while the wait runs.
 */
export const waiting = new Map<string, Set<string>>();

/** How often a wait looks again, unless it is ended early (a dropped call, a newer wait). */
export const WAIT_CHECK_MS = 300;

/** One `wait_session` in flight: the links it holds, and how it ends early. */
export interface Wait {
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
export const holds = new Map<string, Wait>();
export const calls = new Map<string, Wait>();
/**
 * What the last few calls handed over. An answer a wait has already sent is
 * dropped by the engine when the harness stopped waiting at that very moment:
 * the marks it set go back, and the ordinary wake delivers what nobody read.
 */
export const handedBy = new Map<string, HeardMark[]>();
export const HANDED_KEPT = 16;

export const holdKey = (caller: string, link: string) => `${caller}\n${link}`;

/**
 * A call the engine can drop that waits on more than a link: `merge_when_green`
 * waits for the user's approval and then for the checks, and a call the harness
 * (or the MCP server, which answers `pending` at its deadline) gave up on must
 * never merge afterwards. Each step checks `dropped`, and `onDrop` ends what is
 * waiting right now: the approval card, the engine's merge loop.
 */
export interface Droppable {
  caller: string;
  dropped: boolean;
  onDrop: Set<() => void>;
}
export const droppable = new Map<string, Droppable>();

export const trackCall = (callId: string, caller: string): Droppable => {
  const call: Droppable = { caller, dropped: false, onDrop: new Set() };
  droppable.set(callId, call);
  return call;
};

export const droppedFailure = (): Failure =>
  failure("cancelled", "the call was dropped before it finished, so nothing was merged");

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

export function ruleThread(s: AppState, id: string): RuleThread | undefined {
  const summary = s.sessions.find((x) => x.id === id);
  const thread = s.threads[id];
  if (!summary && !thread) return undefined;
  return {
    id,
    title: summary?.title ?? thread?.title ?? "Untitled thread",
    folder: thread?.cwd ?? summary?.cwd ?? "",
    worktree: thread?.worktree ?? summary?.worktree,
    repo: summary?.repo,
    // Only a thread an agent started is the orchestration's to reach; a fork or side chat is not.
    parent: delegateParent(summary?.parent)?.id,
    updatedAt: summary?.updatedAt,
    running: isRunning(thread),
    harness: thread?.harness?.id ?? summary?.harness,
    status: statusOf(s, id),
  };
}

// ---------------------------------------------------------------- asking the user

let resolveAsk: ((choice: GrantChoice) => void) | null = null;

/** Ask once; a second request while one waits is refused, and the agent can try again. */
function ask(question: GrantAsk, call?: Droppable): Promise<GrantChoice> | Failure {
  if (useApp.getState().grant) return failure("busy", "another thread's request is waiting for the user");
  // The grant dialog names the caller and the request; a toast under it would repeat them.
  return new Promise((resolve) => {
    const id = localId("grant");
    resolveAsk = resolve;
    useApp.setState({ grant: { ...question, id } });
    // A call that was dropped takes its own card away, and only its own.
    call?.onDrop.add(() => {
      if (useApp.getState().grant?.id === id) resolveGrant("deny");
    });
  });
}

export function verb(q: GrantAsk): string {
  switch (q.kind) {
    case "message":
      return `message “${q.target}”`;
    case "read":
      return `read “${q.target}”`;
    case "start":
      return q.standalone ? "start a new thread of yours" : "start a thread";
    case "fan_out":
      return q.standalone
        ? `open the same task in ${q.harnesses?.length ?? 2} new threads`
        : `run one task on ${q.harnesses?.length ?? 2} harnesses`;
    case "organize":
      return `change “${q.target}”`;
    case "answer":
      return `answer what “${q.target}” asks`;
    case "interrupt":
      return `stop “${q.target}”`;
    case "configure":
      return `change the model of “${q.target}”`;
    case "transfer":
      return `move “${q.target}” to another harness`;
    case "merge":
      return `merge “${q.target}”'s pull request`;
    case "cleanup":
      return `clean up “${q.target}”`;
    case "handoff":
      return `hand off “${q.target}”`;
    case "project":
      return `change the project “${q.target}”`;
    case "schedule":
      return q.onceOnly ? `schedule the job “${q.target}”` : `schedule “${q.target}”`;
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
  answer: "answer_request",
  interrupt: "interrupt_session",
  configure: "configure_session",
  transfer: "transfer_session",
  merge: "merge_when_green",
  cleanup: "cleanup_worktree",
  handoff: "handoff_worktree",
  project: "update_project",
  schedule: "schedule_task",
};

/** What the app did on the thread's behalf, when its call's row is not in flight to say it. */
const ALLOWED_NOTICE: Record<GrantAsk["kind"], string> = {
  read: "Read a thread",
  message: "Messaged a thread",
  start: "Started a thread",
  fan_out: "Fanned out across harnesses",
  organize: "Changed a thread",
  answer: "Answered a thread",
  interrupt: "Stopped a thread",
  configure: "Changed a thread's model",
  transfer: "Moved a thread to another harness",
  merge: "Merged a delegate's pull request",
  cleanup: "Cleaned up a delegate's worktree",
  handoff: "Handed a worktree off",
  project: "Changed a project",
  schedule: "Changed scheduled work",
};

export async function permitted(
  caller: RuleThread,
  key: string,
  question: GrantAsk,
  call?: Droppable,
  accountKeys: string[] = [],
): Promise<Failure | null> {
  if (call?.dropped) return droppedFailure();
  const needsAccountApproval = accountsNeedApproval(
    book(),
    caller.id,
    accountKeys,
    useApp.getState().trustDelegateAccounts,
  );
  if (!question.onceOnly && !needsAccountApproval && hasGrant(book(), caller.id, key)) return null;
  // The thread's access level answers for the user where it already allows the work: the same
  // policy that answers the harness's own approvals, so a Full-access thread is not re-asked here.
  const mode = permissionModeOf(useApp.getState(), caller.id);
  if (!question.onceOnly && !needsAccountApproval && delegationAllowed(mode, question.kind)) {
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
  const asked = ask(question, call);
  if (isFailure(asked)) return asked;
  const choice = await asked;
  // Answered after the call was dropped: the card is gone or was answered too late, and neither
  // allows anything, nor is an "always" kept for a call nobody is waiting on.
  if (call?.dropped) return droppedFailure();
  if (choice === "deny") return failure("denied", `the user did not allow this thread to ${verb(question)}`);
  if (choice === "always" && !question.onceOnly) {
    setBook((b) => [key, ...accountKeys].reduce((next, allowed) => grant(next, caller.id, allowed), b));
    // User authority must reach durable storage before the approved work starts.
    await flushSave();
  }
  return null;
}

// ---------------------------------------------------------------- the calls

/** Answer one `_rustcode/orchestration` request: a result, or `{ error }` the agent reads. */
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
  const answered = await (key
    ? receipted(caller, call, key, () => perform(caller, call))
    : perform(caller, call));
  return withContextNote(caller.id, answered);
}

/** The quarter of its context each caller was last told it had passed (`contextNote`). */
const contextNoteSteps = new Map<string, number>();

/** An answer with the caller's context note added, when it has just passed another quarter. */
function withContextNote(callerId: string, answered: JsonObject): JsonObject {
  if ("error" in answered) return answered;
  const told = contextNote(useApp.getState().threads[callerId]?.usage, contextNoteSteps.get(callerId) ?? 0);
  if (!told) return answered;
  contextNoteSteps.set(callerId, told.step);
  return { ...answered, context_note: told.note };
}

/** The tools a retry key applies to: the ones that can start or deliver twice. */
const KEYED_TOOLS: ReadonlySet<string> = new Set([
  "message_session",
  "start_session",
  "fan_out",
  "create_loop",
]);

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
export function reachRefused(): JsonObject {
  return failureJson(
    failure(
      "not_allowed",
      "reading across projects is off; turn on “Let agents read threads in other projects” in Settings › Orchestration",
    ),
  );
}

/** Every thread the app lists, as the rules see it. */
export function allThreads(): RuleThread[] {
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
    case "list_sessions": {
      const machine = checkedMachine(call.machine);
      if (isFailure(machine)) return failureJson(machine);
      if (!machine.host && call.allProjects && !acrossProjects()) return reachRefused();
      if (machine.host) return sessionsOnMachine(machine, caller, call.query, call.allProjects);
      const listed = listSessions(threads(), caller, call.query, call.allProjects);
      return {
        ...machineFields(machine),
        sessions: (Array.isArray(listed.sessions) ? listed.sessions : []).map((entry) => ({
          ...(obj(entry) ?? {}),
          ...machineFields(machine),
        })),
      };
    }
    case "read_session": {
      // The key names the thread's machine when the call does not: a call on a keyed id
      // from another computer routes there, as every later call on it does.
      const machine = checkedMachine(call.machine ?? (splitThreadKey(call.target).host || undefined));
      if (isFailure(machine)) return failureJson(machine);
      const target = scopedTarget(machine, call.target);
      if (isFailure(target)) return failureJson(target);
      // The id decides where the thread runs: an id that carries its machine answers with it
      // even when the call omitted `machine` (the key every later call routes by).
      const answer = await read(caller, target, call.lastTurns, call.offset);
      return answer.error ? answer : { ...answer, ...machineFields(machineOfThread(target)) };
    }
    case "message_session": {
      const machine = checkedMachine(call.machine ?? (splitThreadKey(call.target).host || undefined));
      if (isFailure(machine)) return failureJson(machine);
      const target = scopedTarget(machine, call.target);
      if (isFailure(target)) return failureJson(target);
      const answer = await message(caller, target, call.text, call.attachments, call.steer, call.wait);
      return answer.error ? answer : { ...answer, ...machineFields(machineOfThread(target)) };
    }
    case "manage_queue": {
      // The queue is this app's own record, so a machine that is away still has it.
      const on = threadNamedOn(call.machine, call.target);
      if (isFailure(on)) return failureJson(on);
      const answer = await manageQueue(caller, { ...call, target: on.target });
      return answer.error ? answer : { ...answer, ...machineFields(on.machine) };
    }
    case "wait_session": {
      // A named machine checks where each link runs, not whether it can be reached: a wait on
      // a machine that just went away still reports what its links last said, and settles if
      // they do. Naming the wrong machine for a link is a mistake worth saying.
      const named = call.machine?.trim() ? resolveMachine(call.machine) : undefined;
      if (named && isFailure(named)) return failureJson(named);
      const links = call.links
        .map((id) => book().links.find((link) => link.id === id))
        .filter((link): link is Link => !!link && link.orchestrator === caller.id);
      if (named) {
        const elsewhere = links.filter((link) => splitThreadKey(link.target).host !== named.host);
        if (elsewhere.length) {
          const said = elsewhere
            .map((link) => `${link.id} waits on ${machineOfThread(link.target).name}`)
            .join(", ");
          return failureJson(failure("invalid", `${said}, not on ${named.name}`));
        }
      }
      const answer = await wait(caller.id, call.links, call.timeoutMs, call.callId, call.all === true);
      if (answer.error) return answer;
      const at = waitMachine(links);
      return at ? { ...answer, ...machineFields(at) } : answer;
    }
    case "start_session": {
      const machine = checkedMachine(call.machine);
      if (isFailure(machine)) return failureJson(machine);
      if (machine.host) return startOnMachine(machine, caller, call);
      const answer = await start(caller, call);
      return answer.error ? answer : { ...answer, ...machineFields(machine) };
    }
    case "fan_out": {
      const machine = checkedMachine(call.machine);
      if (isFailure(machine)) return failureJson(machine);
      if (machine.host) return fanOutOnMachine(machine, caller, call);
      const answer = await fanOut(caller, call);
      return answer.error ? answer : { ...answer, ...machineFields(machine) };
    }
    case "list_harnesses": {
      const machine = checkedMachine(call.machine);
      if (isFailure(machine)) return failureJson(machine);
      if (machine.host) return listHarnessesOn(machine, call);
      const answer = await listHarnesses(caller, call);
      return answer.error ? answer : { ...answer, ...machineFields(machine) };
    }
    case "list_machines":
      return listMachines(call.probe);
    case "search_sessions": {
      const machine = checkedMachine(call.machine);
      if (isFailure(machine)) return failureJson(machine);
      if (machine.host) return searchSessions(threads(), caller, call.query, true, machine);
      if (call.allProjects && !acrossProjects()) return reachRefused();
      return searchSessions(threads(), caller, call.query, call.allProjects);
    }
    case "list_worktrees": {
      const machine = checkedMachine(call.machine);
      if (isFailure(machine)) return failureJson(machine);
      if (machine.host) return listWorktrees(threads(), caller, true, machine);
      if (call.allProjects && !acrossProjects()) return reachRefused();
      return listWorktrees(threads(), caller, call.allProjects);
    }
    case "list_projects": {
      const machine = checkedMachine(call.machine);
      if (isFailure(machine)) return failureJson(machine);
      return await projectsOnMachine(machine);
    }
    case "read_project": {
      const machine = checkedMachine(call.machine);
      if (isFailure(machine)) return failureJson(machine);
      if (machine.host) return readProjectOn(machine, call.path);
      return readProject(caller, call.path);
    }
    case "create_project":
      return createProject(caller, call);
    case "clone_project": {
      const machine = checkedMachine(call.machine);
      if (isFailure(machine)) return failureJson(machine);
      if (machine.host) return cloneProjectOn(machine, caller, call);
      return cloneProject(caller, call);
    }
    case "update_project":
      return updateProject(caller, call);
    case "remove_project":
      return removeProject(caller, call);
    case "create_loop":
      return createLoop(caller, call);
    case "schedule_task":
      return scheduleTask(caller, call);
    case "list_scheduled_tasks":
      return listScheduledTasks();
    case "delete_scheduled_task":
      return deleteScheduledTask(caller, call.id);
    case "run_scheduled_task_now":
      return runScheduledTaskNow(caller, call.id);
    case "list_pull_requests": {
      const machine = checkedMachine(call.machine);
      if (isFailure(machine)) return failureJson(machine);
      return listPullRequests(caller, call.state, machine);
    }
    case "update_session": {
      const on = managedOn(call.machine, call.target);
      if (isFailure(on)) return failureJson(on);
      const answer = await updateSession(caller, { ...call, target: on.target }, !!on.machine.host);
      return answer.error ? answer : { ...answer, ...machineFields(on.machine) };
    }
    case "interrupt_session": {
      const machine = checkedMachine(call.machine ?? (splitThreadKey(call.target).host || undefined));
      if (isFailure(machine)) return failureJson(machine);
      const target = scopedTarget(machine, call.target);
      if (isFailure(target)) return failureJson(target);
      const answer = await interruptSession(caller, target);
      return answer.error ? answer : { ...answer, ...machineFields(machineOfThread(target)) };
    }
    case "configure_session": {
      const on = managedOn(call.machine, call.target);
      if (isFailure(on)) return failureJson(on);
      const answer = await configureSession(caller, { ...call, target: on.target }, !!on.machine.host);
      return answer.error ? answer : { ...answer, ...machineFields(on.machine) };
    }
    case "transfer_session":
      return transferSession(caller, call.target, call.harness, call.account, call.model);
    case "list_pending_requests": {
      const machine = checkedMachine(call.machine);
      if (isFailure(machine)) return failureJson(machine);
      return listPendingRequests(caller, call.machine ? machine : undefined);
    }
    case "answer_request": {
      const on = machineThread(call.machine, call.target);
      if (isFailure(on)) return failureJson(on);
      const answer = await answerRequest(caller, { ...call, target: on.target });
      return answer.error ? answer : { ...answer, ...machineFields(on.machine) };
    }
    case "organize_threads":
      return organizeThreads(caller, call);
    case "worktree_status": {
      const machine = checkedMachine(
        call.machine ?? (call.target ? splitThreadKey(call.target).host || undefined : undefined),
      );
      if (isFailure(machine)) return failureJson(machine);
      const target = call.target ? scopedTarget(machine, call.target) : undefined;
      if (isFailure(target)) return failureJson(target);
      const answer = await worktreeStatus(caller, { ...call, target }, machine);
      return answer.error ? answer : { ...answer, ...machineFields(machine) };
    }
    case "handoff_worktree":
      return handoffWorktree(caller, call);
    case "thread_status": {
      const machine = checkedMachine(
        call.machine ?? (call.target ? splitThreadKey(call.target).host || undefined : undefined),
      );
      if (isFailure(machine)) return failureJson(machine);
      const target = call.target ? scopedTarget(machine, call.target) : undefined;
      if (isFailure(target)) return failureJson(target);
      const answer = await threadStatus(caller, target, machine.host);
      return answer.error
        ? answer
        : { ...answer, ...machineFields(target ? machineOfThread(target) : machine) };
    }
    case "merge_when_green":
      return mergeWhenGreen(caller, call);
    case "cleanup_worktree":
      return cleanupWorktree(caller, call);
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
      // project while the setting allows it), and no approval is asked. The
      // store read is the one on the computer the thread runs on, so a
      // delegate on a paired machine is read there and not here as an empty
      // list (issue 3525).
      const on = machineThread(call.machine, call.target);
      if (isFailure(on)) return failureJson(on);
      const target = ruleThread(useApp.getState(), on.target);
      // Another machine's thread is that machine's to allow, as `read_session` reads it.
      const refused = checkMessage(
        caller,
        target,
        book(),
        true,
        acrossProjects() || isRemoteThread(on.target),
      );
      if (refused || !target) return failureJson(refused ?? failure("not_found", "no such thread"));
      const { answer, error } = await activityFrom(target.id, {
        ...(call.after ? { after: call.after } : {}),
        ...(call.order ? { order: call.order } : {}),
        limit: call.limit,
      });
      if (!answer) {
        const remote = isRemoteThread(target.id);
        return failureJson(
          failure(
            remote ? "unavailable" : "failed",
            remote
              ? `activity is not available from ${machineOfThread(target.id).name} (${error}). Read the thread itself with read_session.`
              : `the data store did not answer: ${error}`,
          ),
        );
      }
      return { ...answer, ...machineFields(machineOfThread(target.id)) };
    }
  }
}

// ---------------------------------------------------------------- reads: no approval
// ---------------------------------------------------------------- projects
// ---------------------------------------------------------------- scheduled work
// ---------------------------------------------------------------- delegate status (issue 3651)
// ---------------------------------------------------------------- managing threads
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
export let queueBook: QueueBook | undefined;
/** The book as storage last took it: a change that serialises the same is not written again. */
let keptQueues: string | undefined;
/** Putting a saved queue back changes the store, which notifies again: the nested pass is skipped. */
let restoring = false;
/** Whether this window is watching the store for queue changes: installed once, whatever reconnects. */
let watchingQueues = false;
/** At most this many threads are opened at start-up for the queue each kept. */
const RESTORE_MAX = 8;

// No storage (a private window): the queue is this window's only, as before. A web view whose
// storage is full keeps the queue in memory instead.
export function storedQueues(): string | null {
  return readStorage(QUEUES_KEY);
}

function keepQueues(text: string): void {
  writeStorage(QUEUES_KEY, text);
}

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
    (id) => known.has(id) && !isLoaded(s.threads[id]) && pendingRows(queueBook!, id).length > 0,
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
  steeredIntoTurn.clear();
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
    if (!isLoaded(thread) || isLoading(thread)) {
      restoredQueues.delete(id);
      watchedQueues.delete(id);
      continue;
    }
    if (!restoredQueues.has(id)) {
      // First sight of it since it loaded: what it kept is put back beside anything queued meanwhile.
      restoredQueues.add(id);
      const rows = restorable(id, thread.queue);
      if (rows.length) {
        // A row whose pictures were too big to keep came back as its text alone: the queue waits
        // for the user rather than sending a message without what it was about.
        const back = new Set(rows.map((item) => item.id));
        const lost = pendingRows(loaded, id).some((row) => row.extrasLost && back.has(row.id));
        restoring = true;
        try {
          updateThread(id, (t) => ({
            ...t,
            queue: [...t.queue, ...rows],
            ...(lost ? { queueHeld: true } : {}),
          }));
        } finally {
          restoring = false;
        }
        if (lost) {
          toast(
            "A queued message lost its attachments in the restart, so its queue is held: check it, then Resume.",
            "error",
          );
        } else drain.push(id);
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
  if (thread && !isRunning(thread) && !thread.queueHeld) void resumeQueue(id);
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
  const activity: Record<string, LinkActivity> = {};
  const said: Record<string, readonly string[]> = {};
  // A parent and a child are both asked about: the parent hears, and the child's own subtree has
  // to be quiet before it does.
  const remember = (id: string) => {
    if (threads[id]) return;
    const thread = ruleThread(s, id);
    if (thread) threads[id] = { title: thread.title, running: isRunning(s.threads[id]) };
  };
  for (const link of s.links.links) {
    const target = s.threads[link.target];
    states[link.id] = shownState(link, !!target?.permissions.length);
    const request = target?.permissions[0];
    if (request) pending[link.id] = String(request.rpcId);
    remember(link.orchestrator);
    remember(link.target);
    if (!answers[link.target]) answers[link.target] = latestReply(target?.items ?? []);
    if (!activity[link.target])
      activity[link.target] = activityOf(target?.items ?? [], target?.turn?.children);
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
  return {
    states,
    pending,
    waiting: said,
    queued,
    threads,
    now,
    answers,
    activity,
    batchMs: s.wakeBatchSeconds * 1000,
  };
}

/** A link with the target's last reply when it holds no answer of its own. */
export function filledWith(link: Link, reply: string): Link {
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

/** {@link quietBelow} over the app's book: what `id` handed out is done, whatever `id` does now. */
export function subtreeQuietBelow(id: string): boolean {
  return quietBelow(book(), wakeContext(Date.now()), id);
}

/**
 * Delivers what each parent should hear — children settling, children waiting
 * on the user — once per child, and schedules the re-check a coalescing window
 * needs. Called when a link settles, when a delegate asks the user, when a wait
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
 * "Delegate finished". A wake for an idle parent still starts at once.
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
  if (!parent || isLoading(parent)) {
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
  // The same silence wakes the parent once: the notice is advisory, so its mark
  // is written as it is sent or queued, unlike a settled answer's.
  if (wake.stalled?.length) {
    setBook((b) =>
      wake.stalled!.reduce(
        (acc, notice) => updateLink(acc, notice.link.id, { stallNotifiedFor: notice.at }),
        b,
      ),
    );
  }
  const wakeLinks = wakeIds(wake);
  const wakeAsking = askingIds(wake);
  if (isRunning(parent)) {
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
export function markHeard(link: Link, state: LinkState, now: number): HeardMark {
  const request = useApp.getState().threads[link.target]?.permissions[0]?.rpcId;
  const asked = request === undefined ? undefined : String(request);
  const change =
    state === "needs_user" && asked !== undefined ? { askedRequest: asked } : { notifiedAt: now };
  setBook((b) => updateLink(b, link.id, change));
  return { id: link.id, ...change };
}

// ---------------------------------------------------------------- link turns

/**
 * The user must answer something in `thread`: every live link to it stops its
 * work clock, so the delegate tree never counts a wait on the user (`state/delegates.ts`).
 */
export function linkWaitStarted(thread: string, at = Date.now()) {
  const links = waitStarted(book().links, thread, at);
  // No link to that thread waits on anything: leave the book, and its save, alone.
  if (links === book().links) return;
  setBook((b) => ({ ...b, links }));
}

/** It was answered (or stopped): the wait it took is remembered and the clock resumes. */
export function linkWaitEnded(thread: string, at = Date.now()) {
  const links = waitEnded(book().links, thread, at);
  if (links === book().links) return;
  setBook((b) => ({ ...b, links }));
}

export function linkStarted(id: string) {
  // The turn starts as this link's turn: its notice reports the time from here.
  setBook((b) => updateLink(b, id, { state: "working", startedAt: Date.now() }));
}

/**
 * The links whose message went into a running turn of a thread (`steer`), by thread, with the text
 * the engine was given: that turn settles them, and the text finds the ones it never read.
 */
const steeredIntoTurn = new Map<string, Map<string, string>>();

/** What a steer says as the thread recorded it (`appendSteer`), else the text it was asked with. */
const steerText = (thread: string, fallback: string) =>
  useApp.getState().threads[thread]?.steers.at(-1)?.text ?? fallback;

/**
 * A link's message `text` was steered into the running turn of `thread`. No turn of its own carries
 * the link (`runPrompt` `options.link`), so the running turn is the one that answers it: the link
 * works from here and is settled when that turn ends (`threadTurnEnded`), however many steers went
 * in and whether or not the thread is open.
 *
 * A turn that ended while the steer was in flight either handed the steer back (a copy waits in the
 * queue: it carries the link, and the link is `queued` behind it so a failed turn fails it) or read
 * it, in which case that turn's own reply is the answer and the link settles on it. The message is
 * never sent a second time.
 */
export function linkSteered(thread: string, link: string | undefined, text: string): void {
  if (!link) return;
  // The clock starts at the steer: a steered link has no turn of its own to start it, and without a
  // start time `reconcileLinks` would count it from when it was made.
  linkStarted(link);
  const said = steerText(thread, text);
  const current = useApp.getState().threads[thread];
  if (!isRunning(current)) {
    const queued = current?.queue.find((q) => !q.link && q.text.trim() === said.trim());
    if (queued) {
      updateThread(thread, (t) => ({
        ...t,
        queue: t.queue.map((q) => (q.id === queued.id ? { ...q, link } : q)),
      }));
      setBook((b) => updateLink(b, link, { state: "queued" }));
    } else {
      linksFinished([link], thread, "completed", undefined, []);
    }
    return;
  }
  const links = steeredIntoTurn.get(thread) ?? new Map<string, string>();
  links.set(link, said);
  steeredIntoTurn.set(thread, links);
}

/**
 * The turn of `thread` ended and the engine handed back steers it never read (`unusedSteers`); they
 * wait in the queue as turns of their own. The links they came from are still owed an answer, so
 * those turns carry them, and the ending turn does not settle them (`threadTurnEnded`).
 */
export function requeueUnreadSteers(thread: string, unused: readonly string[]): void {
  const steered = steeredIntoTurn.get(thread);
  if (!steered?.size || !unused.length) return;
  const carried = new Map<string, string>();
  for (const said of unused) {
    const found = [...steered].find(([link, text]) => !carried.has(link) && text.trim() === said.trim());
    if (!found) continue;
    carried.set(found[0], found[1]);
    steered.delete(found[0]);
  }
  if (!steered.size) steeredIntoTurn.delete(thread);
  if (!carried.size) return;
  // Queued behind the turn that will read it: a failed turn that holds the queue fails the link
  // (`failQueuedOn`) instead of leaving it Working beside a thread that runs nothing.
  setBook((b) => [...carried.keys()].reduce((acc, link) => updateLink(acc, link, { state: "queued" }), b));
  updateThread(thread, (t) => {
    const waiting = new Map(carried);
    return {
      ...t,
      queue: t.queue.map((q) => {
        if (q.link) return q;
        const found = [...waiting].find(([, text]) => text.trim() === q.text.trim());
        if (!found) return q;
        waiting.delete(found[0]);
        return { ...q, link: found[0] };
      }),
    };
  });
}

/** The links steered into `thread`'s turn, taken from it: a turn that is lost carries them on (`lostTurn`). */
export function takeSteeredLinks(thread: string): string[] {
  const links = [...(steeredIntoTurn.get(thread)?.keys() ?? [])];
  steeredIntoTurn.delete(thread);
  return links;
}

/**
 * The links a resumed turn of `thread` answers: the ones a restart severed while their turn ran
 * (`orchestration.ts` `interruptActive`). A link the restart caught still queued is not one of
 * them: its message never reached the thread, and the queue it waited in is read back and run on
 * its own (`state/queues.ts`), which starts and settles that link as a link turn does.
 */
export function interruptedLinks(thread: string): string[] {
  return book()
    .links.filter((l) => l.target === thread && l.state === "interrupted" && l.startedAt !== undefined)
    .map((l) => l.id);
}

/**
 * The links a resumed thread's turn answers (`store.ts` `resumeInterrupted`): a
 * restart severed them (`orchestration.ts` `interruptActive`), and the turn the
 * app resumes that thread with is the turn they were waiting on. They are left
 * as {@link linkStarted} leaves a link — working, with the clock set by this
 * turn — and marked `resumedAt`, so the parent that reads the notice knows the
 * answer came after a restart, and its work time is not the time it spent
 * waiting out the quit. A link that is no longer interrupted (a newer one to
 * the same thread replaced it, or the user stopped it) is left alone.
 */
export function linksResumed(ids: readonly string[]): void {
  if (!ids.length) return;
  const now = Date.now();
  setBook((b) => ({
    ...b,
    links: b.links.map((link) =>
      ids.includes(link.id) && link.state === "interrupted"
        ? advanceLink(
            link,
            {
              state: "working" as const,
              startedAt: now,
              resumedAt: now,
              waitingSince: undefined,
              waitedMs: undefined,
            },
            now,
          )
        : link,
    ),
  }));
}

/**
 * A thread carries on after the app waited out the stop that ended its turn (`store.ts`
 * `continueDueThreads`): the links that stop settled work again and settle with the new turn.
 */
export function linksReopened(thread: string, ids: readonly string[]): void {
  if (!ids.length) return;
  // A parent the user archived is not woken by a delegate that carries on without it.
  const archived = useApp.getState().archived;
  const next = reopenLinks(book(), thread, ids, Date.now(), (orchestrator) => !!archived[orchestrator]);
  if (next === book()) return;
  setBook(() => next);
}

/** The harness a settled link ran on, as the notice names it: its id and display name. */
function linkHarness(thread: string): { harness?: string; harnessName?: string } {
  const s = useApp.getState();
  const id = s.threads[thread]?.harness?.id ?? s.sessions.find((x) => x.id === thread)?.harness;
  if (!id) return {};
  return { harness: id, harnessName: s.harnesses?.harnesses.find((h) => h.id === id)?.name };
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

/**
 * The links' turn ended: their answer is that turn's last reply, never an
 * earlier one. A turn that ran for several links at once (a thread a restart
 * interrupted on more than one link, which one resume turn answers) settles
 * them in one write to the book and one wake tick, so the parents read one
 * notice and nothing is woken twice.
 */
export function linksFinished(
  ids: readonly string[],
  thread: string,
  outcome: "completed" | "cancelled" | "failed",
  failed: string | undefined,
  items: TranscriptItem[],
  /**
   * The thread waits to carry on again (a usage limit, an overload): a link the parent already
   * heard the stop of stays heard, so it is woken once when the thread settles for good.
   */
  holding = false,
) {
  if (!ids.length) return;
  const now = Date.now();
  // A stopped link is not an answer: it takes only what its own turn said.
  const says = answerFor(thread, items, failed, outcome !== "cancelled");
  const state = outcome === "cancelled" ? "cancelled" : outcome === "failed" ? "failed" : "done";
  const harness = linkHarness(thread);
  let settled = false;
  setBook((b) => ({
    ...b,
    links: b.links.map((link) => {
      if (!ids.includes(link.id)) return link;
      // A link already settled (stopped by the user, or reported by a newer turn of the thread)
      // is not settled again: this turn is not the one it was waiting on.
      if (link.state === "done" || link.state === "failed" || link.state === "cancelled") return link;
      settled = true;
      const workedMs = link.startedAt === undefined ? undefined : Math.max(0, now - link.startedAt);
      return advanceLink(
        link,
        {
          state,
          answer: says.text,
          ...(state === "failed" && failed?.trim() ? { error: failed.trim() } : {}),
          truncated: says.truncated,
          filesChanged: says.filesChanged,
          settledAt: now,
          ...(holding || state === "cancelled" ? {} : { notifiedAt: undefined }),
          ...harness,
          ...(workedMs ? { workedMs } : {}),
        },
        now,
      );
    }),
  }));
  if (settled) wakeTick();
}

/** The link's turn ended: its answer is that turn's last reply, never an earlier one. */
export function linkFinished(
  id: string,
  thread: string,
  outcome: "completed" | "cancelled" | "failed",
  failed: string | undefined,
  items: TranscriptItem[],
) {
  linksFinished([id], thread, outcome, failed, items);
}

/**
 * A turn of `thread` ended, whichever way it was started. A parent's wake waits
 * for the thread's own subtree to be quiet, so a link that settled but was not
 * reported yet takes this turn's answer instead of the one that ended first
 * (host.ts `wait` reads the same link). A link already reported is re-armed once
 * when this turn was started by the wake from the thread's own child, so the
 * real answer reaches the parent too.
 *
 * A link a restart severed (`orchestration.ts` `interruptActive`) is not
 * settled here: only the turn the app resumes that thread with answers it
 * (`store.ts` `resumeInterrupted` hands the link to that turn, `host.ts`
 * {@link linksResumed} and {@link linksFinished}). A turn the user starts in
 * the thread afterwards is their own work, not the branch's result.
 */
export function threadTurnEnded(
  thread: string,
  woken: boolean,
  outcome: "completed" | "cancelled" | "failed",
  failed: string | undefined,
  items: TranscriptItem[],
) {
  // The links whose messages were steered into this turn end with it: no prompt carries them.
  const steered = takeSteeredLinks(thread);
  if (steered.length) linksFinished(steered, thread, outcome, failed, items);
  // A stopped turn is not an answer: the link turns record their own, and nothing else does. Its
  // end still matters to a parent: this thread no longer runs, so what it owes may be due now.
  if (outcome !== "cancelled") {
    const now = Date.now();
    const says = answerFor(thread, items, outcome === "completed" ? undefined : failed);
    setBook((b) => restateLinks(b, thread, says, now, woken));
  }
  if (outcome === "failed") failQueuedOn(thread, failed);
  wakeTick();
}

/**
 * A failed turn holds the thread's queue (`thread.ts` `holdsQueue`), so a message another thread
 * queued there would say Queued for good. It fails instead, with the reason, and leaves the queue:
 * its parent reads why and may message the thread again, which reopens it (`messageNow`). The
 * user's own queued messages stay held for Resume.
 */
function failQueuedOn(thread: string, failure: string | undefined) {
  const { book: next, failed } = failQueuedLinks(book(), thread, failure, Date.now());
  if (!failed.length) return;
  const gone = new Set(failed);
  updateThread(thread, (t) => {
    const queue = t.queue.filter((item) => !item.link || !gone.has(item.link));
    return { ...t, queue, queueHeld: queue.length ? t.queueHeld : undefined };
  });
  setBook(() => next);
}

/** A queued link message was removed from the target's queue. */
export function queuedLinkDropped(id: string) {
  const link = book().links.find((l) => l.id === id);
  if (link?.state !== "queued") return;
  setBook((b) => updateLink(b, id, { state: "cancelled", settledAt: Date.now() }));
  // Its siblings can now be the last ones working: look again at once.
  wakeTick();
}

/** Whether `id` runs nothing and waits on nobody, as far as this window can tell. */
function threadIdle(id: string): boolean {
  const state = useApp.getState();
  const status = statusOf(state, id);
  if (status === "running" || status === "needs_input") return false;
  // A thread this window has not opened may be running in the engine: its list row says so.
  const listed = sessionById(state, id)?.status;
  return listed !== "running" && listed !== "needs_input";
}

/**
 * Stop a link: its queued message goes, or its running turn is cancelled. A link whose thread runs
 * nothing has no turn to cancel and would stay Working for good, so stopping it settles it as
 * cancelled and wakes the parent. The user's own turns there go on.
 */
export async function stopLink(id: string): Promise<boolean> {
  const link = book().links.find((l) => l.id === id);
  if (!link || !isActive(link.state)) return false;
  if (link.state === "queued") {
    updateThread(link.target, (t) => ({ ...t, queue: t.queue.filter((q) => q.link !== id) }));
    setBook((b) => updateLink(b, id, { state: "cancelled" }));
    return true;
  }
  // Another computer's thread is cancelled on that computer, whatever this window last heard of it.
  if (!isRemoteThread(link.target) && threadIdle(link.target)) {
    // A wait that would carry the thread on (a limit, the internet) is the work being stopped.
    dropLimitWait(link.target);
    dropInternetWait(link.target);
    linksFinished([id], link.target, "cancelled", undefined, []);
    return true;
  }
  await cancel(link.target);
  return true;
}

/** How long a working link may sit beside an idle thread before it is read as lost, not just starting. */
export const STALE_LINK_MS = 20_000;

/** How long a working link with no start time may sit beside an idle thread: a worktree or a cold harness can take minutes. */
export const UNSTARTED_LINK_MS = 10 * 60_000;

/**
 * Settles the working links whose thread, by its real status, runs nothing: a turn that ended
 * without settling its link (a steer, a window that missed the end) must not read Working for
 * hours. The answer is the thread's last reply, or its failure. Run when the thread list is read
 * (load and refocus) and when a thread reports a status. A link that is only starting (its turn
 * has not begun yet), one whose message waits in the thread's queue, and another computer's
 * threads (their lost turns are followed by `lostTurn`) are left alone.
 */
export function reconcileLinks(now = Date.now()): void {
  const state = useApp.getState();
  if (!sessionsListed(state.engine)) return;
  const stale = book().links.filter((link) => {
    if (link.state !== "working" && link.state !== "needs_user") return false;
    // A link that is only starting (a worktree being made, a cold harness) has an idle thread beside
    // it for a moment. One with a start time gets `STALE_LINK_MS` from it; one that never recorded a
    // start (a steered link saved before `linkSteered` set it, a turn that never began) counts from
    // when it was made and gets the longer `UNSTARTED_LINK_MS`, so a slow start is not read as lost
    // while a forgotten link still does not read Working for hours.
    if (isRemoteThread(link.target)) return false;
    const idleFor = link.startedAt === undefined ? now - link.createdAt : now - link.startedAt;
    if (idleFor < (link.startedAt === undefined ? UNSTARTED_LINK_MS : STALE_LINK_MS)) return false;
    const thread = state.threads[link.target];
    if (isLoading(thread) || thread?.queue.some((q) => q.link === link.id)) return false;
    if (!thread && !sessionById(state, link.target)) return false;
    // A thread held for a usage limit, the internet or background commands carries on by itself.
    if (state.limitWaits[link.target] || state.internetWaits[link.target] || thread?.waiting) return false;
    return threadIdle(link.target);
  });
  for (const link of stale) {
    const error = state.threads[link.target]?.error;
    linksFinished([link.id], link.target, error ? "failed" : "completed", error, []);
  }
}

/** Settling a sender abandons its outstanding links without stopping their targets. */
export function clearWaitingLinks(ids: readonly string[]) {
  const senders = new Set(ids);
  const at = Date.now();
  setBook((book) => ({
    ...book,
    links: book.links.map((link) =>
      senders.has(link.orchestrator) && isActive(link.state)
        ? advanceLink(link, { state: "cancelled", settledAt: at, notifiedAt: at }, at)
        : link,
    ),
  }));
}

/** Stops the work `orchestrator` gave one delegate (its status row's Stop); the thread's own messages stay. */
export async function stopDelegate(orchestrator: string, target: string): Promise<number> {
  let stopped = 0;
  for (const link of book().links) {
    if (link.orchestrator !== orchestrator || link.target !== target || !isActive(link.state)) continue;
    if (await stopLink(link.id)) stopped += 1;
  }
  return stopped;
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
    stopped += releaseLimitWaitsFrom(orchestrator);
    return stopped;
  } finally {
    stopping.delete(orchestrator);
  }
}

/**
 * The delegates of `orchestrator` that are waiting out a usage limit stop with it: their failed
 * links are cancelled and their wait is let go (or only the link, when another parent's link
 * shares it), so the reset never wakes a parent the user stopped or archived. Returns how many
 * links it cancelled.
 */
export function releaseLimitWaitsFrom(orchestrator: string): number {
  const waits = useApp.getState().limitWaits;
  const held = book().links.filter(
    (l) =>
      l.orchestrator === orchestrator &&
      (l.state === "failed" || l.state === "done") &&
      waits[l.target]?.links?.includes(l.id),
  );
  if (!held.length) return 0;
  const now = Date.now();
  const ids = new Set(held.map((l) => l.id));
  setBook((b) => ({
    ...b,
    links: b.links.map((l) =>
      ids.has(l.id) ? advanceLink(l, { state: "cancelled", settledAt: now }, now) : l,
    ),
  }));
  for (const link of held) releaseLimitWaitLink(link.target, link.id);
  return held.length;
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
    driving: b.links.filter((l) => l.orchestrator === id).slice(-MAX_DELEGATES_PER_THREAD * 2),
    drivenBy: [...b.links].reverse().find((l) => l.target === id && isActive(l.state)),
  };
}
