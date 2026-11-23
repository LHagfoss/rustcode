import {
  checkedMachine,
  fanOutOnMachine,
  listHarnessesOn,
  listMachines,
  machineFields,
  machineOfThread,
  machineThread,
  machineUnavailable,
  managedOn,
  projectsOnMachine,
  resolveMachine,
  runMachineCommand,
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

import { isLoading } from "./thread";
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
import { cleanupWorktree, handoffWorktree, mergeWhenGreen, worktreeStatus } from "./hostDelegates";
import {
  answerRequest,
  compactSession,
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
  updateLoop,
} from "./hostSchedule";
import { activityFrom, fanOut, start, wait } from "./hostStart";
import {
  accountsNeedApproval,
  activityOf,
  askingIds,
  capped,
  checkMessage,
  contextNote,
  type DelegateLimits,
  emptyBook,
  type Failure,
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
  shownState,
  updateLink,
  type Wake,
  type WakeContext,
  type WakeReport,
  wakeIds,
  wakeMessage,
  wakesDue,
} from "./orchestration";
import { revokeThreadGrant } from "./permissionGrants";
import { delegationAllowed, modeLabel } from "./permissions";
import { wakeDelivery } from "./queue";
import {
  type AppState,
  errorText,
  openThread,
  permissionModeOf,
  request,
  runPrompt,
  statusOf,
  steer,
  steeringOf,
  turnState,
  updateThread,
  useApp,
} from "./store";
import { appendNotice, attachAutoAllowed, autoAllowedNoticeNeeded, isRunning, localId } from "./thread";
import { isRemoteThread, LOCAL_HOST, splitThreadKey, threadKey } from "./threadKeys";

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
      return q.onceOnly ? `create the loop “${q.target}”` : `change the loop “${q.target}”`;
    case "exec":
      return `run a command on ${q.target}`;
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
  exec: "run_machine_command",
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
  schedule: "Changed loops",
  exec: "Ran a machine command",
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
export async function answerOrchestration(
  params: JsonObject,
  callerHost: string = LOCAL_HOST,
): Promise<JsonObject> {
  try {
    return await answer(params, callerHost);
  } catch (error) {
    return failureJson(failure("failed", errorText(error)));
  }
}

async function answer(params: JsonObject, callerHost: string = LOCAL_HOST): Promise<JsonObject> {
  const rawSessionId = typeof params.sessionId === "string" ? params.sessionId : "";
  const callerId =
    callerHost && rawSessionId && !splitThreadKey(rawSessionId).host
      ? threadKey(callerHost, rawSessionId)
      : rawSessionId;
  const caller = ruleThread(useApp.getState(), callerId);
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
  const callerHost = splitThreadKey(caller.id).host;
  const acrossProjects = () => useApp.getState().readAcrossProjects;
  const threads = allThreads;
  switch (call.tool) {
    case "list_sessions": {
      const machine = resolveMachine(call.machine, callerHost);
      if (isFailure(machine)) return failureJson(machine);
      if (machine.host) {
        const down = machineUnavailable(machine);
        if (down) return failureJson(down);
        return sessionsOnMachine(machine, caller, call.query, call.allProjects);
      }
      if (call.allProjects && !acrossProjects()) return reachRefused();
      const listed = listSessions(
        threads().filter((t) => splitThreadKey(t.id).host === LOCAL_HOST),
        caller,
        call.query,
        call.allProjects || callerHost !== LOCAL_HOST,
      );
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
      const on = machineThread(call.machine, call.target, callerHost);
      if (isFailure(on)) return failureJson(on);
      const answer = await read(caller, on.target, call.lastTurns, call.offset);
      return answer.error ? answer : { ...answer, ...machineFields(on.machine) };
    }
    case "message_session": {
      const on = machineThread(call.machine, call.target, callerHost);
      if (isFailure(on)) return failureJson(on);
      const answer = await message(caller, on.target, call.text, call.attachments, call.steer, call.wait);
      return answer.error ? answer : { ...answer, ...machineFields(on.machine) };
    }
    case "manage_queue": {
      // The queue is this app's own record, so a machine that is away still has it.
      const on = threadNamedOn(call.machine, call.target, callerHost);
      if (isFailure(on)) return failureJson(on);
      const answer = await manageQueue(caller, { ...call, target: on.target });
      return answer.error ? answer : { ...answer, ...machineFields(on.machine) };
    }
    case "wait_session": {
      // A named machine checks where each link runs, not whether it can be reached: a wait on
      // a machine that just went away still reports what its links last said, and settles if
      // they do. Naming the wrong machine for a link is a mistake worth saying.
      const named = call.machine?.trim() ? resolveMachine(call.machine, callerHost) : undefined;
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
      const machine = checkedMachine(call.machine, callerHost);
      if (isFailure(machine)) return failureJson(machine);
      if (machine.host) return startOnMachine(machine, caller, call);
      const answer = await start(caller, call);
      return answer.error ? answer : { ...answer, ...machineFields(machine) };
    }
    case "fan_out": {
      const machine = checkedMachine(call.machine, callerHost);
      if (isFailure(machine)) return failureJson(machine);
      if (machine.host) return fanOutOnMachine(machine, caller, call);
      const answer = await fanOut(caller, call);
      return answer.error ? answer : { ...answer, ...machineFields(machine) };
    }
    case "list_harnesses": {
      const machine = checkedMachine(call.machine, callerHost);
      if (isFailure(machine)) return failureJson(machine);
      if (machine.host) return listHarnessesOn(machine, caller, call);
      const answer = await listHarnesses(caller, call);
      return answer.error ? answer : { ...answer, ...machineFields(machine) };
    }
    case "list_machines":
      return listMachines(call.probe, callerHost);
    case "run_machine_command": {
      const machine = checkedMachine(call.machine, callerHost);
      if (isFailure(machine)) return failureJson(machine);
      return runMachineCommand(machine, caller, call);
    }
    case "search_sessions": {
      const machine = checkedMachine(call.machine, callerHost);
      if (isFailure(machine)) return failureJson(machine);
      if (machine.host) return searchSessions(threads(), caller, call.query, true, machine);
      if (call.allProjects && !acrossProjects()) return reachRefused();
      return searchSessions(threads(), caller, call.query, call.allProjects || callerHost !== LOCAL_HOST);
    }
    case "list_worktrees": {
      const machine = checkedMachine(call.machine, callerHost);
      if (isFailure(machine)) return failureJson(machine);
      if (machine.host) return listWorktrees(threads(), caller, true, machine);
      if (call.allProjects && !acrossProjects()) return reachRefused();
      return listWorktrees(threads(), caller, call.allProjects || callerHost !== LOCAL_HOST);
    }
    case "list_projects": {
      const machine = checkedMachine(call.machine, callerHost);
      if (isFailure(machine)) return failureJson(machine);
      return await projectsOnMachine(machine);
    }
    case "read_project": {
      const machine = checkedMachine(call.machine, callerHost);
      if (isFailure(machine)) return failureJson(machine);
      if (machine.host) return readProjectOn(machine, call.path);
      return readProject(caller, call.path);
    }
    case "create_project":
      return createProject(caller, call);
    case "clone_project": {
      const machine = checkedMachine(call.machine, callerHost);
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
    case "update_loop":
      return updateLoop(caller, call);
    case "schedule_task":
      return scheduleTask(caller, call);
    case "list_scheduled_tasks":
      return listScheduledTasks();
    case "delete_scheduled_task":
      return deleteScheduledTask(caller, call.id);
    case "run_scheduled_task_now":
      return runScheduledTaskNow(caller, call.id);
    case "list_pull_requests": {
      const machine = checkedMachine(call.machine, callerHost);
      if (isFailure(machine)) return failureJson(machine);
      return listPullRequests(caller, call.state, machine);
    }
    case "compact_session":
      return compactSession(caller, call.target, call.resume);
    case "update_session": {
      const on = managedOn(call.machine, call.target, callerHost);
      if (isFailure(on)) return failureJson(on);
      const answer = await updateSession(
        caller,
        { ...call, target: on.target },
        on.machine.host !== callerHost,
      );
      return answer.error ? answer : { ...answer, ...machineFields(on.machine) };
    }
    case "interrupt_session": {
      const on = machineThread(call.machine, call.target, callerHost);
      if (isFailure(on)) return failureJson(on);
      const answer = await interruptSession(caller, on.target);
      return answer.error ? answer : { ...answer, ...machineFields(on.machine) };
    }
    case "configure_session": {
      const on = managedOn(call.machine, call.target, callerHost);
      if (isFailure(on)) return failureJson(on);
      const answer = await configureSession(
        caller,
        { ...call, target: on.target },
        on.machine.host !== callerHost,
      );
      return answer.error ? answer : { ...answer, ...machineFields(on.machine) };
    }
    case "transfer_session":
      return transferSession(caller, call.target, call.harness, call.account, call.model);
    case "list_pending_requests": {
      const machine = checkedMachine(call.machine, callerHost);
      if (isFailure(machine)) return failureJson(machine);
      return listPendingRequests(caller, call.machine || callerHost ? machine : undefined);
    }
    case "answer_request": {
      const on = machineThread(call.machine, call.target, callerHost);
      if (isFailure(on)) return failureJson(on);
      const answer = await answerRequest(caller, { ...call, target: on.target });
      return answer.error ? answer : { ...answer, ...machineFields(on.machine) };
    }
    case "organize_threads":
      return organizeThreads(caller, call);
    case "worktree_status": {
      const machine = checkedMachine(
        call.machine ?? (call.target ? splitThreadKey(call.target).host || undefined : undefined),
        callerHost,
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
        callerHost,
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
    case "read_activity": {
      // A read of the store, like thread_usage: the project rule applies (any
      // project while the setting allows it), and no approval is asked. The
      // store read is the one on the computer the thread runs on, so a
      // delegate on a paired machine is read there and not here as an empty
      // list (issue 3525).
      const on = machineThread(call.machine, call.target, callerHost);
      if (isFailure(on)) return failureJson(on);
      const target = ruleThread(useApp.getState(), on.target);
      // Another machine's thread is that machine's to allow, as `read_session` reads it.
      const refused = checkMessage(
        caller,
        target,
        book(),
        true,
        acrossProjects() || isRemoteThread(on.target) || splitThreadKey(on.target).host !== callerHost,
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

// ---------------------------------------------------------------- waking a parent

/** What the wake rules need of the app, read once: no polling, no engine calls. */
export function wakeContext(now: number): WakeContext {
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
export const steeredLinks = new Set<string>();

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

export {
  clearWaitingLinks,
  dismissLink,
  interruptedLinks,
  keepFanOut,
  linkFinished,
  linkStarted,
  linkSteered,
  linksFinished,
  linksOf,
  linksReopened,
  linksResumed,
  linkWaitEnded,
  linkWaitStarted,
  queuedLinkDropped,
  reconcileLinks,
  releaseLimitWaitsFrom,
  requeueUnreadSteers,
  STALE_LINK_MS,
  steeredIntoTurn,
  stopDelegate,
  stopLink,
  stopLinksFrom,
  takeSteeredLinks,
  threadTurnEnded,
  UNSTARTED_LINK_MS,
} from "./hostLinkTurns";
export { queueBook, resetQueues, restoreQueuedThreads, storedQueues } from "./hostQueue";
