// The app's side of threads starting and messaging threads: it answers the
// engine's `_rustcode/orchestration` requests by checking the rules
// (state/orchestration.ts), asking the user, creating threads and
// delivering messages through the ordinary turn queue, and reporting each
// link turn's answer. After ThreadMessaging.swift (M1), plus v2's
// `read_session`, `start_session` and `fan_out` (docs/orchestrator.md).
//
// The store imports this module and this module imports the store; nothing
// here touches the store at load time.

import { bool, type Json, type JsonObject, obj, RpcError, str } from "../acp/jsonrpc";
import {
  cleanupRefusal,
  type DelegatePull,
  harnessPlanKey,
  parseMergeOutcome,
  parsePullError,
  parsePullList,
  parsePullState,
  pullErrorTitle,
} from "../acp/rustcode";
import {
  delegateParent,
  type Harness,
  type HarnessModel,
  type HarnessSnapshot,
  Method,
  type PermissionOption,
  type PermissionRequest,
  plainPath,
  projectName,
} from "../acp/types";
import {
  type HandoffRefusal,
  handoffRefusal,
  parseHandoff,
  parseWorktreeStates,
  type ThreadStatus,
  V2Method,
  type WorktreeState,
} from "../acp/v2";
import { optionScope } from "../lib/approval";
import { currentPlatform } from "../lib/platform";
import { hasShell, shellCall } from "../lib/shell";
import { readStorage, writeStorage } from "../lib/storage";
import { childTitle, titleFrom } from "../lib/title";
import {
  type AttachedFile,
  attachedExtras,
  attachmentRefusal,
  attachmentRoots,
  MAX_ATTACHMENT_FILES,
} from "./attachments";
import type { ComposeExtras } from "./compose";
import { isStatusQuery, type StatusInput, type StatusPull, statusRows } from "./delegateStatus";
import { LINK_AGENT_STATE } from "./delegates";
import { restoreChoice, selectOption, unavailableReason, usableHarnesses } from "./harness";
import { usageOf } from "./harnessUsage";
import {
  accountLabelOf,
  accountNameOf,
  accountSignedOut,
  childAccountFor,
  newThreadAccountOn,
} from "./logins";
import {
  LOCAL_MACHINE_ID,
  localAppVersion,
  localIdentity,
  type Machine,
  machineName,
  offlineSinceLabel,
  probeMachineRoutes,
  refreshMachines,
  retryMachines,
  selfName,
  useMachines,
} from "./machines";
import {
  type ApprovalChoice,
  accountsNeedApproval,
  activeDescendants,
  activityOf,
  advanceLink,
  ancestorsOf,
  askingIds,
  capped,
  checkManage,
  checkMessage,
  checkQueryData,
  checkStart,
  cleanProjectPath,
  contextNote,
  cutForAll,
  type DelegateLimits,
  delegateAccountKey,
  delegated,
  delivered,
  emptyBook,
  type Failure,
  type FanOut,
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
  linkResult,
  linksLeft,
  listSessions,
  liveState,
  MAX_DELEGATES,
  MAX_DELEGATES_PER_THREAD,
  MAX_PROJECTS,
  MAX_PULL_REQUESTS,
  MAX_QUEUE_ITEMS,
  MAX_WORKTREE_PULLS,
  MAX_WORKTREES,
  markDelivered,
  mergeBooks,
  nextId,
  nextWakeAt,
  noteEvent,
  noteLinkEvent,
  orphanedLinks,
  type PullStateChoice,
  pageOf,
  parseBook,
  parseCall,
  parseDelivered,
  projectOf,
  prune,
  QUEUE_PREVIEW_CHARS,
  queueWake,
  quietBelow,
  quietTree,
  type RuleThread,
  reachesThread,
  reachOf,
  renderRead,
  reopenLinks,
  restateLinks,
  sameProject,
  sameReach,
  shownState,
  type TurnAnswer,
  unheard,
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
import { delegationAllowed, effectiveMode, modeLabel, type PermissionMode } from "./permissions";
import {
  forgetProject,
  projectEntry,
  projectLabel,
  registeredProjects,
  registerProject,
  updateProjectEntry,
} from "./projectBook";
import {
  isDataFolder,
  isRootFolder,
  type Project,
  projectByRepository,
  projectsOn,
  remoteRepository,
} from "./projects";
import { rememberPullSummaries } from "./pullQueries";
import { QUEUE_REASON, wakeDelivery } from "./queue";
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
  answerPermission,
  answerQuestionOptions,
  answerQuestionText,
  cancel,
  clearFinished,
  connectionFor,
  createSession,
  dropInternetWait,
  dropLimitWait,
  effortMemory,
  ensureHarnessModels,
  errorText,
  harnessesOn,
  isKeptUnlisted,
  lastAccountFor,
  loadMachineHarnesses,
  localChoices,
  markUnread,
  openThread,
  permissionModeOf,
  planCacheStale,
  refreshReadPlanLogins,
  releaseLimitWaitLink,
  removeQueued,
  renameThread,
  request,
  resumeQueue,
  runPrompt,
  searchThreads,
  sendDecisionFor,
  sendQueuedNow,
  sessionById,
  setConfigOption,
  setPermissionMode,
  setSnoozed,
  setThreadStatus,
  setThreadsStatus,
  statusOf,
  steer,
  steeringOf,
  switchThreadHarness,
  toast,
  togglePin,
  turnState,
  updateQueued,
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
import { isRemoteThread, LOCAL_HOST, splitThreadKey, threadKey } from "./threadKeys";
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

function setBook(fn: (book: LinkBook) => LinkBook) {
  const next = prune(fn(useApp.getState().links));
  useApp.setState({ links: next });
  keepLocally(next);
  scheduleSave();
}

const book = () => useApp.getState().links;
/** The two delegate limits (Settings › Harnesses): one thread's own, and the machine's total. */
const delegateLimits = (): DelegateLimits => ({
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
function withLinkGate<T>(run: () => Promise<T>): Promise<T> {
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

/**
 * A call the engine can drop that waits on more than a link: `merge_when_green`
 * waits for the user's approval and then for the checks, and a call the harness
 * (or the MCP server, which answers `pending` at its deadline) gave up on must
 * never merge afterwards. Each step checks `dropped`, and `onDrop` ends what is
 * waiting right now: the approval card, the engine's merge loop.
 */
interface Droppable {
  caller: string;
  dropped: boolean;
  onDrop: Set<() => void>;
}
const droppable = new Map<string, Droppable>();

const trackCall = (callId: string, caller: string): Droppable => {
  const call: Droppable = { caller, dropped: false, onDrop: new Set() };
  droppable.set(callId, call);
  return call;
};

const droppedFailure = (): Failure =>
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

function ruleThread(s: AppState, id: string): RuleThread | undefined {
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
    running: thread?.running ?? false,
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
      return `schedule “${q.target}”`;
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

async function permitted(
  caller: RuleThread,
  key: string,
  question: GrantAsk,
  call?: Droppable,
  accountKeys: string[] = [],
): Promise<Failure | null> {
  if (call?.dropped) return droppedFailure();
  const needsAccountApproval = accountsNeedApproval(book(), caller.id, accountKeys);
  if (!needsAccountApproval && hasGrant(book(), caller.id, key)) return null;
  // The thread's access level answers for the user where it already allows the work: the same
  // policy that answers the harness's own approvals, so a Full-access thread is not re-asked here.
  const mode = permissionModeOf(useApp.getState(), caller.id);
  if (!needsAccountApproval && delegationAllowed(mode, question.kind)) {
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
  if (choice === "always") {
    setBook((b) => [key, ...accountKeys].reduce((next, allowed) => grant(next, caller.id, allowed), b));
  }
  return null;
}

// ---------------------------------------------------------------- machines

/**
 * The computer a call acts on: this one, or a machine the app is paired with
 * (`state/machines.ts`). `machine` is an id or name from `list_machines`; a
 * call without one is this computer, never a "main" machine.
 */
export interface MachineRef {
  /** `""` for this computer, else the machine's id (`state/threadKeys.ts`). */
  host: string;
  /** What `list_machines` calls it: `local`, or the machine's id. */
  id: string;
  name: string;
  /** `macos`, `windows`, `linux`, or `unknown`. */
  platform: string;
}

/** This computer, as a call names it (`list_machines` `current: true`). */
export function localMachine(): MachineRef {
  const platform = currentPlatform();
  return {
    host: LOCAL_HOST,
    id: LOCAL_MACHINE_ID,
    name: selfName(),
    platform: platform === "mac" ? "macos" : platform,
  };
}

/** A paired machine, as a call names it: the name this app shows, never the reported one. */
function machineRef(machine: Machine): MachineRef {
  return { host: machine.id, id: machine.id, name: machineName(machine), platform: machine.platform };
}

/**
 * The machine a `machine` argument names: this computer when it is absent,
 * else a paired machine by id or name, any case. A name is what the user says
 * ("Studio"); an id is what a previous answer gave. Several machines with one
 * name are refused rather than guessed between.
 */
export function resolveMachine(asked: string | undefined): MachineRef | Failure {
  const wanted = asked?.trim().toLowerCase();
  if (!wanted) return localMachine();
  const local = localMachine();
  if (wanted === LOCAL_MACHINE_ID || wanted === local.name.toLowerCase()) return local;
  const machines = useMachines.getState().machines;
  const byId = machines.filter((machine) => machine.id.toLowerCase() === wanted);
  // The name shown is a local display name; the reported one still names it,
  // so a pairing saved before a rename answers to either.
  const found = byId.length
    ? byId
    : machines.filter(
        (machine) => machineName(machine).toLowerCase() === wanted || machine.name.toLowerCase() === wanted,
      );
  if (!found.length) {
    const names = [local.name, ...machines.map(machineName)];
    return failure("not_found", `no machine “${asked}”; list_machines has ${names.join(", ")}`);
  }
  if (found.length > 1) {
    return failure(
      "invalid",
      `“${asked}” names ${found.length} machines; pass the id: ${found.map((machine) => machine.id).join(", ")}`,
    );
  }
  return machineRef(found[0]!);
}

/**
 * Whether a call can act on `machine` now. An offline one says so by name and
 * when it went away, never silently falls back to this computer.
 */
export function machineUnavailable(machine: MachineRef): Failure | null {
  if (!machine.host) {
    return connectionFor(LOCAL_HOST)
      ? null
      : failure("unavailable", "this computer's RustCode server is not connected; nothing was sent");
  }
  if (connectionFor(machine.host)) return null;
  const saved = useMachines.getState().machines.find((m) => m.id === machine.host);
  if (saved?.state === "needsPairing") {
    return failure(
      "offline",
      `${machine.name} is no longer paired with this computer; pair it again in Settings › Environments. Nothing was sent`,
    );
  }
  const since = offlineSinceLabel(useMachines.getState().offlineSince[machine.host]);
  // A machine that answered and refused says what it said: "offline" would send the agent to check
  // a network that works.
  if (saved?.state === "unavailable") {
    return failure(
      "offline",
      `${machine.name} answers but is not serving this computer${saved.detail ? `: ${saved.detail}` : ""}; nothing was sent`,
    );
  }
  return failure("offline", `${machine.name} is offline${since ? ` since ${since}` : ""}; nothing was sent`);
}

/**
 * Where a machine stands, as `list_machines` says it: `connected`, `connecting`, `reconnecting`
 * (no route answered; the app is on its retry clock), `unavailable` (a route answered and the
 * machine would not serve: `cause` says why), `revoked` (it refused this computer's pairing), or
 * `offline` (not connected and not trying).
 */
function machineState(saved: Machine | undefined, connected: boolean): string {
  if (connected) return "connected";
  switch (saved?.state) {
    case "needsPairing":
      return "revoked";
    case "connecting":
      return "connecting";
    case "unavailable":
      return "unavailable";
    case "offline":
      return saved.retryInSecs ? "reconnecting" : "offline";
    default:
      return "offline";
  }
}

/** What every machine-aware answer carries, so an agent can tell where a thread runs. */
function machineFields(machine: MachineRef): JsonObject {
  return { machine: machine.name, machine_id: machine.id };
}

/** A host key ("" is this computer), as an answer about a thread there names it. */
function machineOfHost(host: string): MachineRef {
  if (!host) return localMachine();
  const saved = useMachines.getState().machines.find((m) => m.id === host);
  return {
    host,
    id: host,
    name: useApp.getState().machineNames[host] ?? saved?.name ?? host,
    platform: saved?.platform ?? "unknown",
  };
}

/** The machine a thread key names: an answer about a keyed thread says where it runs. */
const machineOfThread = (id: string) => machineOfHost(splitThreadKey(id).host);

/** The one machine a wait's links act on, when they all do. */
function waitMachine(links: Link[]): MachineRef | undefined {
  const hosts = new Set(links.map((link) => splitThreadKey(link.target).host));
  return hosts.size === 1 ? machineOfHost([...hosts][0]!) : undefined;
}

/**
 * A thread id as the named machine knows it: an id that already carries a
 * machine is kept (and must be that machine's), and a bare one is prefixed
 * with the machine the call named.
 */
function scopedTarget(machine: MachineRef, target: string): string | Failure {
  const { host } = splitThreadKey(target);
  if (host) {
    if (machine.host && machine.host !== host) {
      const other = useApp.getState().machineNames[host] ?? host;
      return failure("invalid", `“${target}” runs on ${other}, not ${machine.name}`);
    }
    return target;
  }
  return threadKey(machine.host, target);
}

/** A machine and the connection a call on it needs, or the failure to answer with. */
function checkedMachine(asked: string | undefined): MachineRef | Failure {
  const machine = resolveMachine(asked);
  if (isFailure(machine)) return machine;
  return machineUnavailable(machine) ?? machine;
}

/**
 * The machine and thread key a call on one thread acts on: the machine the
 * call names, else the one the id carries. A connected machine only, so a
 * call never lands on this computer when another was meant.
 */
function machineThread(
  asked: string | undefined,
  target: string,
): { machine: MachineRef; target: string } | Failure {
  // An id that carries its machine names it already: connected is all it needs to be.
  const carried = machineOfThread(target);
  const machine = asked?.trim() ? checkedMachine(asked) : (machineUnavailable(carried) ?? carried);
  if (isFailure(machine)) return machine;
  const key = scopedTarget(machine, target);
  if (isFailure(key)) return key;
  return { machine, target: key };
}

/**
 * The same for a call that only touches this app's own record of the thread
 * (its queue): the machine may be away, and the thread is still named by it.
 */
function threadNamedOn(
  asked: string | undefined,
  target: string,
): { machine: MachineRef; target: string } | Failure {
  const named = asked?.trim() ? resolveMachine(asked) : machineOfThread(target);
  if (isFailure(named)) return named;
  const key = scopedTarget(named, target);
  if (isFailure(key)) return key;
  return { machine: named, target: key };
}

/**
 * The thread a management call acts on, on the machine the call names or its
 * id carries: `undefined` is the caller itself, which only this computer has.
 */
function managedOn(
  asked: string | undefined,
  target: string | undefined,
): { machine: MachineRef; target: string | undefined } | Failure {
  if (!target) {
    const machine = checkedMachine(asked);
    if (isFailure(machine)) return machine;
    if (machine.host) return failure("invalid", `session_id is required for a thread on ${machine.name}`);
    return { machine, target };
  }
  return machineThread(asked, target);
}

/** One request to the engine of `host`: this computer's, or a paired machine's over its connection. */
async function requestOn<T = Json>(
  host: string,
  method: string,
  params: JsonObject,
  timeoutMs: number,
): Promise<T> {
  const conn = connectionFor(host);
  if (!conn) throw new Error(host ? "that machine is not connected" : "the engine is not connected");
  return conn.request<T>(method, params, timeoutMs);
}

/**
 * Each folder's repository (`remoteRepository` of its `origin`), per machine:
 * `null` for a folder with none. Only an answer is kept, so a read that failed
 * is tried again by the next call.
 */
const repositories = new Map<string, string | null>();
/** How long one folder's git read may take while a call matches projects. */
const REPOSITORY_READ_MS = 10_000;

/** The repository a folder on `host` is a checkout of, or `null`; `undefined` when it could not be read. */
async function repositoryOf(host: string, path: string): Promise<string | null | undefined> {
  const key = `${host}\n${path}`;
  if (repositories.has(key)) return repositories.get(key);
  try {
    const facts = await requestOn<JsonObject>(host, Method.projectRead, { path }, REPOSITORY_READ_MS);
    const repository = remoteRepository(str(facts?.remote));
    repositories.set(key, repository);
    return repository;
  } catch {
    return undefined;
  }
}

/**
 * Folders cloned on a paired machine by `clone_project` here: its thread list
 * names a project only once a thread runs there, and a clone is where the
 * next thread starts.
 */
const clonedOn = new Map<string, string[]>();

/** For tests: forget every folder's repository and every clone made on a machine. */
export function forgetMachineProjectsForTests(): void {
  repositories.clear();
  clonedOn.clear();
}

/** A machine's projects: what its threads name, newest first, then what was cloned there from here. */
function machineProjects(host: string): Project[] {
  const listed = projectsOn(useApp.getState().sessions, host);
  const cloned = (clonedOn.get(host) ?? [])
    .filter((path) => !listed.some((project) => project.path === path))
    .map((path) => ({ path, name: projectName(path) }));
  return [...listed, ...cloned];
}

/** Models `list_machines` names per harness: enough to choose a machine by, not the catalogue. */
const MACHINE_LISTED_MODELS = 8;

/**
 * A machine as `list_machines` describes it: what it is, where it stands and
 * why, how busy it is, and what it has to run a thread with and in. An agent
 * choosing where to delegate reads this once instead of calling
 * `list_harnesses` and `list_projects` on every machine.
 */
async function machineEntry(machine: MachineRef): Promise<JsonObject> {
  const saved = useMachines.getState().machines.find((m) => m.id === machine.id);
  const connected = !!connectionFor(machine.host);
  const state = machineState(saved, connected);
  const version = machine.host ? (saved?.version ?? null) : await localAppVersion();
  const st = useApp.getState();
  const threads = st.sessions.filter((x) => (x.host ?? LOCAL_HOST) === machine.host);
  const running = threads.filter((x) => !!st.threads[x.id]?.running || x.status === "running").length;
  const waiting = threads.reduce((n, x) => n + (st.threads[x.id]?.permissions.length ?? 0), 0);
  return {
    id: machine.id,
    // The reported name stays here, so a client that never heard of the local
    // one still says what the machine calls itself; display_name is what the
    // app shows everywhere, and `name` above is that too for a call.
    name: saved?.name ?? machine.name,
    display_name: saved?.displayName ?? (machine.host ? null : localIdentity().displayName),
    icon: saved?.icon ?? (machine.host ? null : localIdentity().icon),
    icon_tint: saved?.iconTint ?? (machine.host ? null : localIdentity().tint),
    ...(saved?.paused ? { paused: true } : {}),
    platform: machine.platform,
    ...(version ? { version } : {}),
    ...(machine.host ? {} : { current: true }),
    state,
    // Why it is not connected, in the words Settings › Machines shows, as a word to branch on
    // (`cause`), and when it tries again.
    ...(!connected && machine.host
      ? {
          reason:
            [saved?.detail, saved?.hint].filter(Boolean).join(" ") ||
            (saved?.paused ? "paused" : "not connected"),
          ...(saved?.reason ? { cause: saved.reason } : {}),
          ...(saved?.retryInSecs != null ? { retry_in_secs: saved.retryInSecs } : {}),
        }
      : {}),
    ...(saved?.route
      ? { route: { kind: saved.route.kind, address: saved.route.address, encrypted: saved.route.encrypted } }
      : {}),
    ...(saved?.lastConnectedAt ? { last_connected: saved.lastConnectedAt } : {}),
    // Each route with what its last test found: reachable, refused or timeout, with the latency.
    ...(saved?.routes.length
      ? {
          routes: saved.routes.map((route) => ({
            kind: route.kind,
            address: route.address,
            encrypted: route.encrypted,
            probe: route.probe
              ? {
                  result: route.probe.outcome,
                  ...(route.probe.latencyMs === null ? {} : { latency_ms: route.probe.latencyMs }),
                  tested_at: route.probe.at,
                }
              : null,
          })),
        }
      : {}),
    running_threads: running,
    pending_requests: waiting,
    harnesses: await machineHarnesses(machine),
    projects: await machineProjectRows(machine, connected),
  };
}

/** How long `list_machines` with `probe` waits for the machines it woke to connect or say why not. */
const RECONNECT_WAIT_MS = 10_000;
const RECONNECT_POLL_MS = 500;

/**
 * Try every paired machine that is not connected again now, and wait (at most
 * `RECONNECT_WAIT_MS`) until each is connected or its attempt ended with a
 * reason. A probe whose route answered has already woken its machine in the
 * shell; this also wakes one whose routes all timed out, since a probe is only
 * a TCP connect.
 */
async function reconnectMachines(): Promise<void> {
  const woken = new Set(await retryMachines());
  // The ones the probe woke are trying already.
  for (const m of useMachines.getState().machines) {
    if (m.state === "connecting" && !connectionFor(m.id)) woken.add(m.id);
  }
  if (!woken.size) return;
  const tried = new Set<string>();
  const deadline = Date.now() + RECONNECT_WAIT_MS;
  while (Date.now() < deadline) {
    await new Promise((resolve) => setTimeout(resolve, RECONNECT_POLL_MS));
    await refreshMachines();
    const machines = useMachines.getState().machines;
    let pending = false;
    for (const id of woken) {
      const m = machines.find((entry) => entry.id === id);
      if (!m || connectionFor(id) || m.state === "needsPairing" || m.state === "stopped") continue;
      if (m.state === "connecting" || m.state === "connected") {
        tried.add(id);
        pending = true;
      } else if (!tried.has(id)) {
        // Still on the backoff it had before the wake reached it.
        pending = true;
      }
    }
    if (!pending) return;
  }
}

/**
 * A machine's projects for `list_machines`: path, name and the repository each
 * is a checkout of. A machine that is not connected says what was read last.
 */
async function machineProjectRows(machine: MachineRef, connected: boolean): Promise<JsonObject[]> {
  const local = machine.host ? [] : listProjects(useApp.getState()).projects;
  const projects = machine.host
    ? machineProjects(machine.host)
    : (Array.isArray(local) ? local : []).flatMap((row) => {
        const project = obj(row);
        return project ? [{ path: String(project.path ?? ""), name: String(project.name ?? "") }] : [];
      });
  const listed = projects.slice(0, MAX_PROJECTS);
  const remotes = await Promise.all(
    listed.map((project) =>
      connected
        ? repositoryOf(machine.host, project.path)
        : repositories.get(`${machine.host}\n${project.path}`),
    ),
  );
  return listed.map((project, index) => ({
    path: project.path,
    name: project.name,
    ...(remotes[index] === undefined ? {} : { remote: remotes[index] }),
  }));
}

/**
 * `list_machines`: this computer and each paired machine, one call. With `probe`
 * every route of every paired machine is tested first, so the answer carries
 * what each found now rather than the last test, and every machine that is not
 * connected is tried again at once; the answer then says whether it came up,
 * or why not.
 */
async function listMachines(probe = false): Promise<JsonObject> {
  const probed = probe ? await probeMachineRoutes() : undefined;
  if (probe) await reconnectMachines();
  const local = localMachine();
  const machines = [local, ...useMachines.getState().machines.map(machineRef)];
  return {
    current: local.id,
    machines: await Promise.all(machines.map(machineEntry)),
    ...(probed === undefined ? {} : { probed }),
  };
}

/**
 * The harnesses one machine can run, as `list_machines` lists them: this app's
 * own snapshot, or that machine's. Each says whether it can run a thread and
 * why not, its first models and its named logins; `list_harnesses` with
 * `machine` has the rest.
 */
async function machineHarnesses(machine: MachineRef): Promise<JsonObject[] | null> {
  const snapshot = machine.host
    ? await loadMachineHarnesses(machine.host)
    : harnessesOn(useApp.getState(), LOCAL_HOST);
  if (!snapshot) return null;
  return snapshot.harnesses.map((h): JsonObject => {
    const reason = unavailableReason(h);
    if (reason) return { id: h.id, name: h.name, available: false, reason };
    const models = h.models.slice(0, MACHINE_LISTED_MODELS).map((m) => m.id);
    return {
      id: h.id,
      name: h.name,
      available: true,
      ...(h.id === snapshot.default.harness ? { default: true } : {}),
      models,
      ...(models.length < h.models.length ? { omitted_models: h.models.length - models.length } : {}),
      ...(h.accounts?.length
        ? {
            accounts: h.accounts.map((a) => ({
              id: a.id,
              name: a.name,
              ...(accountSignedOut(h.id, a.id, useApp.getState().signedOutLogins)
                ? { signed_out: true }
                : {}),
            })),
          }
        : {}),
    };
  });
}

/** Another machine's harnesses, read over its own connection (kept per machine). */
async function remoteSnapshot(machine: MachineRef): Promise<HarnessSnapshot | Failure> {
  const snapshot = await loadMachineHarnesses(machine.host);
  if (snapshot) return snapshot;
  return machineUnavailable(machine) ?? failure("failed", `could not read ${machine.name}'s harnesses`);
}

/** One harness of a named machine, by id else name, refused with what can run there. */
function harnessOn(
  snapshot: HarnessSnapshot,
  asked: string | undefined,
  machine: MachineRef,
): Harness | Failure {
  const wanted = asked?.trim().toLowerCase();
  const all = snapshot.harnesses;
  const usable = (h: Harness) => !unavailableReason(h);
  if (!wanted) {
    const fallback = snapshot.default.harness;
    const h =
      all.find((x) => x.id === fallback && usable(x)) ??
      all.find(usable) ??
      all.find((x) => x.id === fallback);
    if (!h) return failure("not_allowed", `${machine.name} has no harness that can run a thread`);
    const reason = unavailableReason(h);
    if (reason) {
      return failure("not_allowed", `${h.name} cannot run a thread on ${machine.name}: ${reason}`);
    }
    return h;
  }
  const byId = all.filter((h) => h.id.toLowerCase() === wanted);
  const named = byId.length ? byId : all.filter((h) => h.name.toLowerCase() === wanted);
  if (!named.length) {
    return failure(
      "invalid",
      `no harness “${asked}” on ${machine.name}; it has ${all.map((h) => h.id).join(", ")}`,
    );
  }
  if (named.length > 1) {
    return failure(
      "invalid",
      `“${asked}” names ${named.length} harnesses on ${machine.name}; pass the id: ${named.map((h) => h.id).join(", ")}`,
    );
  }
  const h = named[0]!;
  const reason = unavailableReason(h);
  if (reason) return failure("not_allowed", `${h.name} cannot run a thread on ${machine.name}: ${reason}`);
  return h;
}

/**
 * `list_projects` on a machine: the projects it has threads in. This
 * computer's answer also counts a project's unopened threads; another
 * machine's is what its listed threads say, which is what it knows here, plus
 * what `clone_project` cloned there, each with the repository it is a
 * checkout of (`remote`, `host/owner/repo`), which is how `start_session`
 * finds the caller's project there.
 */
async function projectsOnMachine(machine: MachineRef): Promise<JsonObject> {
  if (!machine.host) return { ...machineFields(machine), ...listProjects(useApp.getState()) };
  const rows = new Map<
    string,
    { path: string; name: string; threads: number; working: boolean; updatedAt: number }
  >();
  for (const x of useApp.getState().sessions) {
    if ((x.host ?? LOCAL_HOST) !== machine.host) continue;
    // A thread with no folder of its own is not a project: its machine lists a stand-in, and a root
    // is never a project either (issue 3425). Neither is the app's own data folder (issue 3524).
    if (x.workspaceUnknown) continue;
    const path = plainPath(x.worktree?.repo || x.repo || x.cwd);
    if (!path || isRootFolder(path) || isDataFolder(path)) continue;
    const entry = rows.get(path) ?? {
      path,
      name: projectName(path),
      threads: 0,
      working: false,
      updatedAt: 0,
    };
    entry.threads += 1;
    entry.working ||= !!useApp.getState().threads[x.id]?.running || x.status === "running";
    entry.updatedAt = Math.max(entry.updatedAt, x.updatedAt ?? 0);
    rows.set(path, entry);
  }
  for (const path of clonedOn.get(machine.host) ?? []) {
    if (!rows.has(path))
      rows.set(path, { path, name: projectName(path), threads: 0, working: false, updatedAt: 0 });
  }
  const listed = [...rows.values()].sort((a, b) => b.updatedAt - a.updatedAt).slice(0, MAX_PROJECTS);
  const remotes = await Promise.all(listed.map((p) => repositoryOf(machine.host, p.path)));
  const projects = listed.map((p, index) => ({
    path: p.path,
    name: p.name,
    // null: no `origin`; absent: the folder could not be read there.
    ...(remotes[index] === undefined ? {} : { remote: remotes[index] }),
    threads: p.threads,
    working: p.working,
  }));
  return { ...machineFields(machine), projects };
}

/**
 * `list_harnesses` on a machine: its snapshot, trimmed to what picks a thread
 * (id, name, availability, models and named accounts), never another
 * machine's plan numbers or this app's remembered choices.
 */
async function listHarnessesOn(
  machine: MachineRef,
  asked: { harness?: string; model?: string },
): Promise<JsonObject> {
  const snapshot = await remoteSnapshot(machine);
  if (isFailure(snapshot)) return failureJson(snapshot);
  const harness = harnessOn(snapshot, asked.harness, machine);
  // A filter that matches nothing names what is there, like the start it would serve.
  if (isFailure(harness) && asked.harness) return failureJson(harness);
  const filter = asked.model?.toLowerCase();
  const listed = isFailure(harness) ? snapshot.harnesses : [harness];
  return {
    ...machineFields(machine),
    current: {
      harness: snapshot.default.harness,
      ...(snapshot.default.model ? { model: snapshot.default.model } : {}),
    },
    harnesses: listed.map((h): JsonObject => {
      const reason = unavailableReason(h);
      if (reason) return { id: h.id, name: h.name, available: false, reason };
      const models = (
        filter ? h.models.filter((m) => `${m.id} ${m.name}`.toLowerCase().includes(filter)) : h.models
      )
        .slice(0, filter ? FILTERED_MODELS : LISTED_MODELS)
        .map((m) => ({ id: m.id, name: m.name, ...(m.efforts.length ? { efforts: m.efforts } : {}) }));
      return {
        id: h.id,
        name: h.name,
        available: true,
        ...(h.accounts?.length
          ? {
              accounts: h.accounts.map((a) => ({
                id: a.id,
                name: a.name,
                auth: a.auth,
                ...(accountSignedOut(h.id, a.id, useApp.getState().signedOutLogins)
                  ? { signed_out: true }
                  : {}),
              })),
            }
          : {}),
        models,
        ...(models.length < h.models.length ? { omitted_models: h.models.length - models.length } : {}),
      };
    }),
  };
}

/**
 * `list_sessions` on a machine: its threads, newest first, with the machine
 * each one is on. Another machine's threads are not narrowed to this
 * project: its folders are not this one's, and the user named the machine.
 */
function sessionsOnMachine(
  machine: MachineRef,
  caller: RuleThread,
  query: string,
  allProjects: boolean,
): JsonObject {
  const s = useApp.getState();
  const among = { ...s, sessions: s.sessions.filter((x) => (x.host ?? LOCAL_HOST) === machine.host) };
  const threads = among.sessions.flatMap((x) => ruleThread(among, x.id) ?? []);
  const listed = listSessions(threads, caller, query, true);
  const sessions = (Array.isArray(listed.sessions) ? listed.sessions : []).map((entry) => {
    const item = obj(entry) ?? {};
    return {
      ...item,
      ...machineFields(machine),
      ...(allProjects || machine.host ? { project: item.project ?? "" } : {}),
    };
  });
  return { ...machineFields(machine), sessions };
}

/**
 * The project a thread starts in on a machine: the agent's own path, else the
 * caller's path when that machine has it, else the project there that is a
 * checkout of the caller's repository (its `origin`, however each URL is
 * spelled), else, for a caller with no repository, a folder of the same name.
 * Nothing is guessed: a machine whose projects match none of those refuses
 * with each one's path and repository, and never starts in whichever it used
 * last (B6). The app's own data folder is never a project, however a
 * machine's thread list names it (issue 3524).
 */
async function projectOnMachine(
  machine: MachineRef,
  caller: RuleThread,
  asked: string | undefined,
): Promise<string | Failure> {
  // A path the agent names is its own choice, and the machine's engine refuses
  // one that is not a folder there: this app only knows what it has listed.
  if (asked?.trim()) return asked.trim().replace(/\/+$/, "");
  const projects = machineProjects(machine.host);
  const own = projectOf(caller);
  const there = projects.find((project) => sameProject(project.path, own));
  if (there) return there.path;
  const ours = own ? await repositoryOf(LOCAL_HOST, own) : null;
  const theirs = await Promise.all(projects.map((project) => repositoryOf(machine.host, project.path)));
  const found = projectByRepository(
    { name: own ? projectName(own) : "", repository: ours },
    projects.map((project, index) => ({ ...project, repository: theirs[index] })),
  );
  if (found) return found.path;
  const wanted = ours ?? (own ? projectName(own) : "this thread's project");
  if (!projects.length) {
    return failure(
      "invalid",
      `${machine.name} has no project listed yet, so none is ${wanted}; pass project with its absolute path there, or clone_project with machine`,
    );
  }
  const listed = projects
    .slice(0, MAX_PROJECTS)
    .map((project, index) => `${project.path}${theirs[index] ? ` (${theirs[index]})` : ""}`);
  return failure(
    "invalid",
    `${machine.name} has no project that is ${wanted}; pass project as one of: ${listed.join(", ")}, or clone_project with machine`,
  );
}

/** `session/new` on a machine, with the worktree fallback a folder outside git needs. */
async function threadOnMachine(
  machine: MachineRef,
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
  const ask = (worktree: boolean) =>
    createSession({
      host: machine.host,
      cwd: project,
      harness,
      account: options.account ?? undefined,
      model: options.model,
      effort: options.effort,
      title: options.title,
      parent: options.parent,
      worktree: worktree ? { repo: project, title: options.title } : undefined,
    });
  try {
    return await ask(wantWorktree);
  } catch (error) {
    if (wantWorktree && options.worktree === undefined && /git|repositor/i.test(errorText(error))) {
      return ask(false);
    }
    throw error;
  }
}

/** `start_session` on a paired machine: its harness, its project, its thread, this thread's link. */
async function startOnMachine(machine: MachineRef, caller: RuleThread, call: StartCall): Promise<JsonObject> {
  const standalone = call.standalone === true;
  if (!standalone) {
    const refused = checkStart(caller, book(), 1, delegateLimits());
    if (refused) return failureJson(refused);
  }
  const project = await projectOnMachine(machine, caller, call.project);
  if (isFailure(project)) return failureJson(project);
  const snapshot = await remoteSnapshot(machine);
  if (isFailure(snapshot)) return failureJson(snapshot);
  const harness = harnessOn(snapshot, call.harness, machine);
  if (isFailure(harness)) return failureJson(harness);
  const title = call.title ?? titleFrom(call.prompt, 60);
  const access = narrowed(standalone ? ownMode() : childMode(caller), call.access);
  const other = !sameProject(project, projectOf(caller));
  const no = await permitted(
    caller,
    `start@${machine.id}${other ? `:${project}` : ""}`,
    {
      kind: "start",
      caller: caller.id,
      callerTitle: caller.title,
      target: title,
      detail: call.prompt,
      project: other ? project : undefined,
      harnesses: [choiceLabel(harness.name, call.account ?? undefined, call.model, call.effort)],
      access,
      worktree: call.worktree ?? true,
      machine: machine.name,
      standalone: standalone || undefined,
    },
    undefined,
    harness.accounts?.length ? [delegateAccountKey(machine.id, harness.id, call.account ?? null)] : [],
  );
  if (no) return failureJson(no);
  if (!standalone) {
    const again = checkStart(caller, book(), 1, delegateLimits());
    if (again) return failureJson(again);
  }
  let id: string;
  try {
    id = await threadOnMachine(machine, project, harness.id, {
      model: call.model,
      effort: call.effort,
      // The login is that machine's own: never resolve it against this app's.
      account: call.account ?? null,
      title,
      worktree: call.worktree,
      parent: standalone ? undefined : caller.id,
    });
    // Before its first turn, so nothing it does runs broader than the thread that started it.
    setPermissionMode(id, access);
  } catch (error) {
    return failureJson(
      failure("failed", `the thread could not start on ${machine.name}: ${errorText(error)}`),
    );
  }
  if (standalone) {
    deliverOwn(id, call.prompt);
    return {
      session_id: id,
      receipt: "started",
      title,
      harness: harness.id,
      access: accessOf(id),
      standalone: true,
      ...machineFields(machine),
    };
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
    harnessName: harness.name,
    model: call.model,
    account: call.account,
    machine: machine.name,
  });
  deliver(id, delegated(call.prompt, caller.title), link, caller.id);
  return {
    session_id: id,
    link_id: link,
    receipt: "started",
    title,
    harness: harness.id,
    access: accessOf(id),
    ...machineFields(machine),
    links_left: linksLeft(book(), caller.id, delegateLimits()),
  };
}

/** `fan_out` on a paired machine: the same task on its harnesses, one link each. */
async function fanOutOnMachine(
  machine: MachineRef,
  caller: RuleThread,
  call: FanOutCall,
): Promise<JsonObject> {
  const standalone = call.standalone === true;
  if (!standalone) {
    const refused = checkStart(caller, book(), call.harnesses.length, delegateLimits());
    if (refused) return failureJson(refused);
  }
  const snapshot = await remoteSnapshot(machine);
  if (isFailure(snapshot)) return failureJson(snapshot);
  const project = await projectOnMachine(machine, caller, call.project);
  if (isFailure(project)) return failureJson(project);
  const chosen: { harness: Harness; model?: string; effort?: string; account?: string | null }[] = [];
  const targets = new Set<string>();
  for (const entry of call.harnesses) {
    const h = harnessOn(snapshot, entry.harness, machine);
    if (isFailure(h)) return failureJson(h);
    const key = [h.id, entry.model ?? "", entry.effort ?? "", entry.account ?? ""].join("\u0000");
    if (targets.has(key)) {
      return failureJson(
        failure(
          "invalid",
          `harnesses resolves to the same target twice on ${machine.name}: ${choiceLabel(h.name, entry.account, entry.model, entry.effort)}; give each entry a different harness, model, effort or account`,
        ),
      );
    }
    targets.add(key);
    chosen.push({ harness: h, model: entry.model, effort: entry.effort, account: entry.account ?? null });
  }
  const title = call.title ?? titleFrom(call.prompt, 60);
  const access = narrowed(standalone ? ownMode() : childMode(caller), call.access);
  const other = !sameProject(project, projectOf(caller));
  const no = await permitted(
    caller,
    `start@${machine.id}${other ? `:${project}` : ""}`,
    {
      kind: "fan_out",
      caller: caller.id,
      callerTitle: caller.title,
      target: title,
      detail: call.prompt,
      project: other ? project : undefined,
      harnesses: chosen.map((h) => choiceLabel(h.harness.name, h.account ?? undefined, h.model, h.effort)),
      access,
      worktree: true,
      machine: machine.name,
      standalone: standalone || undefined,
    },
    undefined,
    chosen
      .filter((h) => h.harness.accounts?.length)
      .map((h) => delegateAccountKey(machine.id, h.harness.id, h.account ?? null)),
  );
  if (no) return failureJson(no);
  if (!standalone) {
    const again = checkStart(caller, book(), chosen.length, delegateLimits());
    if (again) return failureJson(again);
  }
  const fanId = localId("fan");
  const out: JsonObject[] = [];
  const linkIds: string[] = [];
  for (const h of chosen) {
    const threadTitle = [title, h.harness.name, h.account].filter(Boolean).join(" · ");
    try {
      const id = await threadOnMachine(machine, project, h.harness.id, {
        model: h.model,
        effort: h.effort,
        account: h.account,
        title: threadTitle,
        parent: standalone ? undefined : caller.id,
      });
      setPermissionMode(id, access);
      if (standalone) {
        deliverOwn(id, call.prompt);
        out.push({
          session_id: id,
          harness: h.harness.id,
          access: accessOf(id),
          ...machineFields(machine),
        });
        continue;
      }
      const link = addLink({
        orchestrator: caller.id,
        target: id,
        orchestratorTitle: caller.title,
        targetTitle: threadTitle,
        message: call.prompt,
        state: "working",
        started: true,
        harness: h.harness.id,
        harnessName: h.harness.name,
        model: h.model,
        account: h.account ?? undefined,
        fanOut: fanId,
        machine: machine.name,
      });
      linkIds.push(link);
      out.push({
        session_id: id,
        link_id: link,
        harness: h.harness.id,
        access: accessOf(id),
        ...machineFields(machine),
      });
      deliver(id, delegated(call.prompt, caller.title), link, caller.id);
    } catch (error) {
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
  if (standalone) return { ...machineFields(machine), links: out, standalone: true };
  return {
    ...machineFields(machine),
    links: out,
    fan_out_id: fanId,
    links_left: linksLeft(book(), caller.id, delegateLimits()),
  };
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
      "reading across projects is off; turn on “Let agents read threads in other projects” in Settings › Harnesses",
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
      return transferSession(caller, call.target, call.harness);
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

/** Models listed per harness: an agent's context pays for each one. */
const LISTED_MODELS = 12;
/** Models a `model` filter returns: every match worth picking from, still bounded. */
const FILTERED_MODELS = 50;
/** Said beside `omitted_models`, so an agent narrows instead of guessing at what it cannot see. */
const MODEL_FILTER_HINT = "pass model to search the omitted ones";
/** Said when a `model` filter matched nothing, so an agent narrows it again. */
const MODEL_MISS_HINT = "no model matches; pass a shorter substring";

/** Whether a model matches a `model` filter: its id or its name, either case. */
const matchesModel = (m: HarnessModel, filter: string) =>
  m.id.toLowerCase().includes(filter) || m.name.toLowerCase().includes(filter);

/**
 * The models this project's threads ran on, most recent first, per harness: the
 * models an agent is likeliest to want are the ones the user already uses here.
 */
function usedModels(caller: RuleThread): Map<string, string[]> {
  const reach = reachOf(caller);
  const recent = useApp
    .getState()
    .sessions.filter((t) => t.harness && t.model && sameProject(reach, t.worktree?.repo || t.repo || t.cwd))
    .sort((a, b) => (b.updatedAt ?? 0) - (a.updatedAt ?? 0));
  const out = new Map<string, string[]>();
  for (const t of recent) {
    const used = out.get(t.harness!) ?? [];
    if (!used.includes(t.model!)) used.push(t.model!);
    out.set(t.harness!, used);
  }
  return out;
}

/**
 * A harness's models in the order that matters: the one a new thread on it starts
 * with, then the ones this project's threads used recently, then the harness's own
 * order. A `filter` narrows to the models whose id or name contains it.
 */
function orderedModels(
  h: Harness,
  configured: string | null,
  used: string[],
  filter: string | undefined,
): HarnessModel[] {
  // Only the models it lists lead: a remembered one this listing no longer has
  // is not offered.
  const ids = new Set(h.models.map((m) => m.id));
  const lead = [configured, ...used].filter((id): id is string => !!id && ids.has(id));
  const rank = new Map<string, number>();
  for (const id of lead) if (!rank.has(id)) rank.set(id, rank.size);
  const models = filter ? h.models.filter((m) => matchesModel(m, filter)) : [...h.models];
  // The sort is stable, so the rest stay in the harness's own order.
  return models.sort((a, b) => (rank.get(a.id) ?? lead.length) - (rank.get(b.id) ?? lead.length));
}

/** The efforts every listed model shares, when they share one and it is not empty. */
function sharedEfforts(models: HarnessModel[]): string[] | undefined {
  const first = models[0]?.efforts ?? [];
  if (!first.length) return undefined;
  const same = models.every(
    (m) => m.efforts.length === first.length && m.efforts.every((effort, i) => effort === first[i]),
  );
  return same ? first : undefined;
}

async function listHarnesses(
  caller: RuleThread,
  asked: { harness?: string; model?: string } = {},
): Promise<JsonObject> {
  const snapshot = useApp.getState().harnesses;
  const harnesses = snapshot?.harnesses ?? [];
  const signedOut = useApp.getState().signedOutLogins;
  await Promise.all(harnesses.filter((h) => !unavailableReason(h)).map((h) => ensureHarnessModels(h.id)));
  const fresh = useApp.getState().harnesses?.harnesses ?? harnesses;
  // The models a listing just brought in are the ones on record now, so the
  // choice a new thread starts on is restored against those, not the snapshot
  // taken before the ask.
  const current: HarnessSnapshot | undefined = snapshot ? { ...snapshot, harnesses: fresh } : undefined;
  const wanted = asked.harness?.toLowerCase();
  // An id is exact and wins; a display name may be shared by a harness the user
  // added, so several matches are refused rather than one picked silently.
  const byId = wanted ? fresh.filter((h) => h.id.toLowerCase() === wanted) : fresh;
  const named = wanted && !byId.length ? fresh.filter((h) => h.name.toLowerCase() === wanted) : byId;
  if (wanted && !named.length) {
    return failureJson(
      failure(
        "invalid",
        `no harness “${asked.harness}”; list_harnesses has ${fresh.map((h) => h.id).join(", ")}`,
      ),
    );
  }
  if (wanted && named.length > 1) {
    return failureJson(
      failure(
        "invalid",
        `“${asked.harness}” names ${named.length} harnesses; pass the id: ${named.map((h) => h.id).join(", ")}`,
      ),
    );
  }
  const listed = named;
  const filter = asked.model?.toLowerCase();
  const used = usedModels(caller);
  // What a new thread on each harness starts with: the same rule the composer uses.
  const remembered = localChoices();
  const efforts = effortMemory();
  const configuredModels = current
    ? new Map(fresh.map((h) => [h.id, restoreChoice(current, h.id, remembered, efforts).model]))
    : undefined;
  // Each login's plan windows come from the cache Settings › Usage and the
  // composer's meter fill, never from a read this call starts: a model's tool
  // call must not raise a Keychain prompt or an Anthropic request of its own.
  // A stale cache is refreshed in the background so that the next call has
  // newer numbers (`refreshReadPlanLogins`): the default login and the account
  // of the thread on screen, plus every named login that was read before, since
  // those are the accounts an agent is choosing between. It honours the pauses
  // and the read's rate limit, and it is not awaited: this answer is what the
  // cache holds now.
  const cache = useApp.getState().planCache;
  if (planCacheStale(cache)) refreshReadPlanLogins();
  const usage = (harness: string, account?: string | null) =>
    usageOf(cache[harnessPlanKey(harness, account)]);
  return {
    current: { harness: caller.harness ?? "rustcode", ...ranWith(caller.id) },
    harnesses: listed.map((h) => {
      const reason = unavailableReason(h);
      const models = orderedModels(h, configuredModels?.get(h.id) ?? null, used.get(h.id) ?? [], filter);
      const shown = models.slice(0, filter ? FILTERED_MODELS : LISTED_MODELS);
      const omitted = models.length - shown.length;
      const shared = sharedEfforts(shown);
      const entries = shown.map((m) => ({
        id: m.id,
        name: m.name,
        ...(shared || !m.efforts.length ? {} : { efforts: m.efforts }),
      }));
      // The harness's own login has no id, so its windows sit on the harness
      // entry: they are what a thread runs on with no `account`. A named
      // login's own numbers sit on its entry, and are never swapped with it.
      const own = usage(h.id);
      return {
        id: h.id,
        name: h.name,
        available: !reason,
        ...(own ? { usage: own } : {}),
        // The logins start_session and fan_out may name; the harness's own sign-in has no id.
        ...(h.accounts?.length
          ? {
              accounts: h.accounts.map((a) => {
                const numbers = usage(h.id, a.id);
                return {
                  id: a.id,
                  name: a.name,
                  auth: a.auth,
                  // The plan read saw this login signed out: naming it is refused.
                  ...(accountSignedOut(h.id, a.id, signedOut) ? { signed_out: true } : {}),
                  ...(numbers ? { usage: numbers } : {}),
                };
              }),
            }
          : {}),
        ...(reason
          ? { reason }
          : {
              // The efforts every listed model shares sit here once, not on each.
              ...(shared ? { efforts: shared } : {}),
              models: entries,
              ...(omitted ? { omitted_models: omitted, filter: MODEL_FILTER_HINT } : {}),
              ...(!entries.length && filter ? { filter: MODEL_MISS_HINT } : {}),
            }),
      };
    }),
  };
}

/**
 * What a paired machine's own index finds for `query`, keyed as this app
 * keys its threads; empty for a short query or an engine without the index.
 */
async function searchThreadsOn(host: string, query: string): Promise<Record<string, string>> {
  if (query.trim().length < 3) return {};
  try {
    const result = await requestOn<{ results?: { session_id?: string; snippet?: string }[] } | null>(
      host,
      "_rustcode/threads/search",
      { query, limit: 30 },
      30_000,
    );
    const hits: Record<string, string> = {};
    for (const hit of result?.results ?? []) {
      if (hit.session_id) hits[threadKey(host, hit.session_id)] = hit.snippet ?? "";
    }
    return hits;
  } catch {
    return {};
  }
}

/**
 * `search_sessions`: titles and what was said, from the index of the computer
 * the threads run on. A named machine's threads are all of its threads, as
 * `list_sessions` lists them, each with its project there.
 */
async function searchSessions(
  all: RuleThread[],
  caller: RuleThread,
  query: string,
  allProjects = false,
  machine?: MachineRef,
): Promise<JsonObject> {
  const host = machine?.host ?? "";
  const said = host ? await searchThreadsOn(host, query) : await searchThreads(query);
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  const matches = all
    .filter((t) => (host ? splitThreadKey(t.id).host === host : allProjects || reachesThread(caller, t)))
    .filter((t) => t.id in said || words.every((w) => t.title.toLowerCase().includes(w)))
    .sort((a, b) => (b.updatedAt ?? 0) - (a.updatedAt ?? 0))
    .slice(0, 30);
  return {
    ...(machine ? machineFields(machine) : {}),
    sessions: matches.map((t) => ({
      session_id: t.id,
      title: t.title,
      status: t.status ?? (t.running ? "running" : "idle"),
      ...(allProjects ? { project: projectOf(t) } : {}),
      ...(said[t.id] ? { snippet: said[t.id] } : {}),
    })),
  };
}

/** `list_worktrees`: this project's worktree threads, every project's, or every one a named machine has. */
function listWorktrees(
  all: RuleThread[],
  caller: RuleThread,
  allProjects = false,
  machine?: MachineRef,
): JsonObject {
  const host = machine?.host ?? "";
  const reached = (t: RuleThread) =>
    host ? splitThreadKey(t.id).host === host : allProjects || reachesThread(caller, t);
  return {
    ...(machine ? machineFields(machine) : {}),
    worktrees: all.flatMap((t) =>
      t.worktree && reached(t)
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

/**
 * `list_projects`: every project the user has a thread in or registered,
 * most recently updated first. A registered folder with no thread yet shows
 * with 0, so a project created or cloned before its first thread is there.
 */
function listProjects(s: AppState): JsonObject {
  const byPath = new Map<string, { path: string; threads: number; working: boolean; updatedAt: number }>();
  for (const x of s.sessions) {
    // A thread whose machine never recorded its folder has no project to count (issue 3425).
    if (x.workspaceUnknown) continue;
    const path = projectOf({ folder: x.cwd, worktree: x.worktree });
    if (!path || isRootFolder(path) || isDataFolder(path)) continue;
    const entry = byPath.get(path) ?? { path, threads: 0, working: false, updatedAt: 0 };
    entry.threads += 1;
    entry.working ||= !!s.threads[x.id]?.running || x.status === "running";
    entry.updatedAt = Math.max(entry.updatedAt, x.updatedAt ?? 0);
    byPath.set(path, entry);
  }
  for (const { path, entry } of registeredProjects()) {
    if (byPath.has(path)) continue;
    byPath.set(path, {
      path,
      threads: 0,
      working: false,
      updatedAt: entry.registeredAt ?? 0,
    });
  }
  const projects = [...byPath.values()]
    .sort((a, b) => b.updatedAt - a.updatedAt)
    .slice(0, MAX_PROJECTS)
    .map((p) => ({ path: p.path, name: projectLabel(p.path), threads: p.threads, working: p.working }));
  return { projects };
}

// ---------------------------------------------------------------- projects

/** A folder as the project book files it, the same rule `projectOf` reads. */
const projectPath = (path: string) => cleanProjectPath(path);

/** The threads that run in `path`, for a project's row: never a worktree's other repository. */
function projectThreads(path: string): { threads: number; working: boolean } {
  const s = useApp.getState();
  let threads = 0;
  let working = false;
  for (const x of s.sessions) {
    if (projectOf({ folder: x.cwd, worktree: x.worktree }) !== path) continue;
    threads += 1;
    working ||= !!s.threads[x.id]?.running || x.status === "running";
  }
  return { threads, working };
}

/** A path as the engine resolved it: what `_rustcode/project/*` answered. */
function projectAnswer(facts: JsonObject, path: string, extra: JsonObject = {}): JsonObject {
  const entry = projectEntry(path);
  return {
    project: {
      ...facts,
      path,
      name: projectLabel(path),
      ...projectThreads(path),
      ...(entry?.harness ? { harness: entry.harness } : {}),
      ...(entry?.model ? { model: entry.model } : {}),
      registered: !!entry?.registeredAt,
      ...extra,
    },
  };
}

/** The engine's project call, answered as a failure the agent can read. */
async function askProject(
  method: string,
  params: JsonObject,
  timeoutMs: number,
): Promise<JsonObject | Failure> {
  try {
    return await request<JsonObject>(method, params, timeoutMs);
  } catch (error) {
    if (error instanceof RpcError) {
      // -32602 is JSON-RPC's `invalid_params`: the folder is not there.
      return failure(error.code === -32602 ? "not_found" : "target_unavailable", error.message);
    }
    return failure("target_unavailable", `the project could not be read: ${errorText(error)}`);
  }
}

/** Why a path may not be read: this thread's project, or another while the setting allows it. */
function projectRefusal(caller: RuleThread, path: string): Failure | null {
  if (sameProject(path, reachOf(caller))) return null;
  if (useApp.getState().readAcrossProjects) return null;
  return failure(
    "not_allowed",
    "that project is outside this thread's; turn on “Let agents read threads in other projects” in Settings › Harnesses",
  );
}

/**
 * `read_project`: one folder's git facts and the app's own entry for it —
 * display name, default harness and model, thread count. A read: no approval.
 */
async function readProject(caller: RuleThread, path: string): Promise<JsonObject> {
  const asked = projectPath(path);
  const refused = projectRefusal(caller, asked);
  if (refused) return failureJson(refused);
  const facts = await askProject(Method.projectRead, { path: asked }, 15_000);
  if (isFailure(facts)) return failureJson(facts);
  return projectAnswer(facts, projectPath(String(facts.path ?? asked)));
}

/**
 * `read_project` on a paired machine: that folder's git facts, read by the
 * engine there, and how many of its threads this app lists. The user named the
 * machine, so the cross-project setting (this computer's) does not apply.
 */
async function readProjectOn(machine: MachineRef, path: string): Promise<JsonObject> {
  const asked = projectPath(path);
  let facts: JsonObject;
  try {
    facts = await requestOn<JsonObject>(machine.host, Method.projectRead, { path: asked }, 15_000);
  } catch (error) {
    const code = error instanceof RpcError && error.code === -32602 ? "not_found" : "target_unavailable";
    return failureJson(failure(code, `${machine.name} could not read ${asked}: ${errorText(error)}`));
  }
  const at = projectPath(String(facts.path ?? asked));
  repositories.set(`${machine.host}\n${at}`, remoteRepository(str(facts.remote)));
  const threads = useApp
    .getState()
    .sessions.filter(
      (x) =>
        (x.host ?? LOCAL_HOST) === machine.host &&
        sameProject(projectOf({ folder: x.cwd, worktree: x.worktree }), at),
    );
  return {
    ...machineFields(machine),
    project: {
      ...facts,
      path: at,
      name: projectName(at),
      threads: threads.length,
      working: threads.some((x) => !!useApp.getState().threads[x.id]?.running || x.status === "running"),
    },
  };
}

/** A folder's parent, on whichever platform wrote it. */
const parentFolder = (path: string) => path.replace(/[\\/]+$/, "").replace(/[\\/][^\\/]*$/, "");

/**
 * `clone_project` on a paired machine: `git clone` by the engine there. The
 * parent is the call's, else the one folder that machine's projects share;
 * with several it asks for one rather than pick. The clone is remembered as
 * one of that machine's projects, so the next `start_session` finds it by
 * its repository.
 */
async function cloneProjectOn(
  machine: MachineRef,
  caller: RuleThread,
  call: { url: string; parent?: string; name?: string },
): Promise<JsonObject> {
  const parents = [
    ...new Set(machineProjects(machine.host).map((project) => parentFolder(project.path))),
  ].filter(Boolean);
  const parent = call.parent ? projectPath(call.parent) : parents.length === 1 ? parents[0]! : "";
  if (!parent) {
    return failureJson(
      failure(
        "invalid",
        parents.length
          ? `pass parent: the folder on ${machine.name} to clone into, such as ${parents.slice(0, 10).join(", ")}`
          : `pass parent: the absolute folder on ${machine.name} to clone into`,
      ),
    );
  }
  const no = await permitted(caller, `project:clone@${machine.id}:${call.url}`, {
    kind: "project",
    caller: caller.id,
    callerTitle: caller.title,
    target: call.name ? `${parent}/${call.name}` : parent,
    detail: `It would run git clone on ${call.url} in ${parent} on ${machine.name}. A large repository over a slow link can take minutes.`,
    machine: machine.name,
  });
  if (no) return failureJson(no);
  let facts: JsonObject;
  try {
    facts = await requestOn<JsonObject>(
      machine.host,
      Method.projectClone,
      { url: call.url, parent, ...(call.name ? { name: call.name } : {}) },
      620_000,
    );
  } catch (error) {
    const code = error instanceof RpcError && error.code === -32602 ? "not_found" : "target_unavailable";
    return failureJson(failure(code, `the clone on ${machine.name} failed: ${errorText(error)}`));
  }
  const at = projectPath(String(facts.path ?? parent));
  const cloned = clonedOn.get(machine.host) ?? [];
  if (!cloned.includes(at)) clonedOn.set(machine.host, [...cloned, at]);
  repositories.set(`${machine.host}\n${at}`, remoteRepository(str(facts.remote) ?? call.url));
  return {
    ...machineFields(machine),
    project: { ...facts, path: at, name: projectName(at), threads: 0, created: true },
  };
}

/** `create_project`: register an existing folder, or make a new one a repository. */
async function createProject(
  caller: RuleThread,
  call: { path: string; init?: boolean },
): Promise<JsonObject> {
  const path = projectPath(call.path);
  const no = await permitted(caller, `project:${path}`, {
    kind: "project",
    caller: caller.id,
    callerTitle: caller.title,
    target: path,
    detail: `It would register ${path} as a project${call.init === false ? "" : ", making it a git repository if it is not one"}.`,
  });
  if (no) return failureJson(no);
  const facts = await askProject(Method.projectCreate, { path, init: call.init ?? true }, 60_000);
  if (isFailure(facts)) return failureJson(facts);
  const resolved = projectPath(String(facts.path ?? path));
  registerProject(resolved);
  return projectAnswer(facts, resolved, { created: true });
}

/** `clone_project`: `git clone` into a parent folder, through the engine, then register it. */
async function cloneProject(
  caller: RuleThread,
  call: { url: string; parent?: string; name?: string },
): Promise<JsonObject> {
  const s = useApp.getState();
  const parent = projectPath(call.parent || projectOf(caller) || s.projectCwd || s.defaultCwd);
  if (!parent) {
    return failureJson(failure("failed", "there is no folder to clone into; pass parent"));
  }
  const no = await permitted(caller, `project:clone:${call.url}`, {
    kind: "project",
    caller: caller.id,
    callerTitle: caller.title,
    target: call.name ? `${parent}/${call.name}` : parent,
    detail: `It would run git clone on ${call.url} in ${parent} and register the result as a project. A large repository over a slow link can take minutes.`,
  });
  if (no) return failureJson(no);
  const facts = await askProject(
    Method.projectClone,
    { url: call.url, parent, ...(call.name ? { name: call.name } : {}) },
    620_000,
  );
  if (isFailure(facts)) return failureJson(facts);
  const resolved = projectPath(String(facts.path ?? parent));
  registerProject(resolved);
  return projectAnswer(facts, resolved, { created: true });
}

/** `update_project`: a display name, and the harness and model new threads there start on. */
async function updateProject(
  caller: RuleThread,
  call: { path: string; name?: string; harness?: string; model?: string },
): Promise<JsonObject> {
  const path = projectPath(call.path);
  const harnesses = useApp.getState().harnesses?.harnesses ?? [];
  if (call.harness && !harnesses.some((h) => h.id === call.harness)) {
    return failureJson(
      failure(
        "not_found",
        `no harness ${call.harness}; list_harnesses shows the ids${harnesses.length ? ` (${harnesses.map((h) => h.id).join(", ")})` : ""}`,
      ),
    );
  }
  // A model without a harness still says what it is: the folder's new threads start on it.
  const on = call.harness ?? call.model;
  const no = await permitted(caller, `project:${path}`, {
    kind: "project",
    caller: caller.id,
    callerTitle: caller.title,
    target: path,
    detail: `It would ${[
      call.name && `call it “${call.name}”`,
      on && `start new threads there on ${on}${call.harness && call.model ? ` with ${call.model}` : ""}`,
    ]
      .filter(Boolean)
      .join(" and ")}. The folder itself is not touched.`,
  });
  if (no) return failureJson(no);
  const entry = updateProjectEntry(path, {
    name: call.name,
    harness: call.harness,
    model: call.model,
  });
  return projectAnswer({}, path, {
    ...(entry.name ? { name: entry.name } : {}),
  });
}

/** `remove_project`: forget the folder; its files and its threads stay. */
async function removeProject(caller: RuleThread, call: { path: string }): Promise<JsonObject> {
  const path = projectPath(call.path);
  const no = await permitted(caller, `remove-project:${path}`, {
    kind: "project",
    caller: caller.id,
    callerTitle: caller.title,
    target: path,
    detail: `It would unregister ${path} as a project. The folder and every file in it stay on disk, and its threads stay; only its display name and defaults go.`,
  });
  if (no) return failureJson(no);
  const known = forgetProject(path);
  return { path, removed: true, known, files_deleted: false };
}

// ---------------------------------------------------------------- scheduled work

/** The user's IANA time zone, which a cron with none is read in. */
function userTimeZone(): string | undefined {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || undefined;
  } catch {
    return undefined;
  }
}

/** How a schedule reads in the approval card. */
function scheduleText(schedule: {
  kind: "cron" | "at";
  expression?: string;
  timezone?: string;
  at?: string;
}): string {
  if (schedule.kind === "cron") {
    return `every cron ${schedule.expression}${schedule.timezone ? ` (${schedule.timezone})` : ""}`;
  }
  return `once at ${schedule.at}`;
}

/** The engine's scheduler call, answered as a failure the agent can read. */
async function askScheduler(method: string, params: JsonObject, timeoutMs = 30_000): Promise<JsonObject> {
  try {
    return await request<JsonObject>(method, params, timeoutMs);
  } catch (error) {
    if (error instanceof RpcError) {
      return failureJson(failure("target_unavailable", error.message));
    }
    return failureJson(failure("target_unavailable", `the scheduler did not answer: ${errorText(error)}`));
  }
}

/** `schedule_task`: a prompt the daemon runs on its schedule, with no thread open. */
async function scheduleTask(
  caller: RuleThread,
  call: {
    prompt: string;
    schedule: { kind: "cron"; expression: string; timezone?: string } | { kind: "at"; at: string };
    project?: string;
    name?: string;
    modelProfile?: string;
  },
): Promise<JsonObject> {
  const workspace = projectPath(call.project || projectOf(caller) || useApp.getState().projectCwd);
  if (!workspace) {
    return failureJson(failure("failed", "this thread has no project folder; pass project"));
  }
  // A cron is read in the user's own zone unless the call named one: the app knows it, the engine only guesses.
  const timezone = call.schedule.kind === "cron" ? call.schedule.timezone || userTimeZone() : undefined;
  const schedule =
    call.schedule.kind === "cron"
      ? {
          kind: "cron" as const,
          expression: call.schedule.expression,
          ...(timezone ? { timezone } : {}),
        }
      : call.schedule;
  const title = call.name || call.prompt.trim().split("\n")[0]?.slice(0, 60) || "this task";
  const no = await permitted(caller, "schedule:new", {
    kind: "schedule",
    caller: caller.id,
    callerTitle: caller.title,
    target: title,
    detail: `It would add “${title}” to the scheduler: ${scheduleText(schedule)}, running in ${workspace} with no thread open. The daemon must be running for it to fire.`,
  });
  if (no) return failureJson(no);
  return askScheduler(Method.scheduledCreate, {
    prompt: call.prompt,
    workspace,
    schedule,
    ...(call.name ? { name: call.name } : {}),
    ...(call.modelProfile ? { modelProfile: call.modelProfile } : {}),
  });
}

/** `list_scheduled_tasks`: what the daemon holds, and whether it is running. */
async function listScheduledTasks(): Promise<JsonObject> {
  return askScheduler(Method.scheduledList, {}, 20_000);
}

/**
 * What the list says a task is called and where it runs, so a card names it
 * rather than a `task-1723…` id. The daemon may not answer: the changing call
 * then reads as it did before, still naming the id it was given.
 */
async function scheduledTaskName(id: string): Promise<string> {
  const listed = await askScheduler(Method.scheduledList, {}, 20_000);
  const tasks = Array.isArray(listed.tasks) ? listed.tasks : [];
  const task = tasks.find((entry) => obj(entry)?.id === id);
  const name = str(obj(task)?.name);
  const workspace = str(obj(task)?.workspace);
  if (!name && !workspace) return "that scheduled task";
  return `“${name ?? id}”${workspace ? `, which runs in ${workspace}` : ""}`;
}

/** `delete_scheduled_task`: the task leaves the scheduler at once. */
async function deleteScheduledTask(caller: RuleThread, id: string): Promise<JsonObject> {
  const task = await scheduledTaskName(id);
  const no = await permitted(caller, `schedule:delete:${id}`, {
    kind: "schedule",
    caller: caller.id,
    callerTitle: caller.title,
    target: id,
    detail: `It would delete ${task}. It stops at once, and its run history goes with it.`,
  });
  if (no) return failureJson(no);
  return askScheduler(Method.scheduledDelete, { id });
}

/** `run_scheduled_task_now`: one run now, with the schedule left as it is. */
async function runScheduledTaskNow(caller: RuleThread, id: string): Promise<JsonObject> {
  const task = await scheduledTaskName(id);
  const no = await permitted(caller, `schedule:run:${id}`, {
    kind: "schedule",
    caller: caller.id,
    callerTitle: caller.title,
    target: id,
    detail: `It would run ${task} now, and keep its schedule. Its prompt runs with no thread open.`,
  });
  if (no) return failureJson(no);
  return askScheduler(Method.scheduledRun, { id });
}

/** Why a pull request read failed, in the shape the agent reads: `{ error: { code, message } }`. */
function pullFailure(error: { code: string; message: string }): Failure {
  return failure(
    error.code === "failed" ? "failed" : "target_unavailable",
    `${pullErrorTitle(error.code)}: ${error.message}`,
  );
}

/** `list_pull_requests`: the caller's project's pull requests, through the panel's own call. */
/**
 * `list_pull_requests`: this project's, read by `gh` on this computer, or on a
 * named machine in its checkout of this project's repository (found as
 * `start_session` finds it), with that machine's own `gh` login.
 */
async function listPullRequests(
  caller: RuleThread,
  state: PullStateChoice,
  machine: MachineRef = localMachine(),
): Promise<JsonObject> {
  const cwd = machine.host ? await projectOnMachine(machine, caller, undefined) : projectOf(caller);
  if (isFailure(cwd)) return failureJson(cwd);
  if (!cwd) return failureJson(failure("failed", "this thread has no project folder"));
  let result: Json;
  try {
    result = await requestOn(machine.host, Method.pulls, { cwd, state, limit: MAX_PULL_REQUESTS }, 60_000);
  } catch (error) {
    return failureJson(failure("target_unavailable", `pull requests could not be read: ${errorText(error)}`));
  }
  const error = parsePullError(result);
  const unsupported = noProviderPullList(
    error,
    machine.host ? { ...machineFields(machine), project: cwd } : {},
  );
  if (unsupported) return unsupported;
  if (error) return failureJson(pullFailure(error));
  const pulls = parsePullList(result).pulls;
  // The answer also seeds the panel's header: a pull opened after the agent's
  // read draws its title, state and checks before its own detail is read.
  if (!machine.host) rememberPullSummaries(cwd, pulls);
  return {
    ...(machine.host ? { ...machineFields(machine), project: cwd } : {}),
    pull_requests: pulls.map((p) => ({
      number: p.number,
      title: p.title,
      state: p.state,
      branch: p.headRef,
      ...(p.url ? { url: p.url } : {}),
    })),
  };
}
/** An unsupported remote is an empty list with an explanation, not a failed read. */
export function noProviderPullList(
  error: { code: string } | null | undefined,
  machine: JsonObject,
): JsonObject | null {
  return error?.code === "no_provider" ? { ...machine, pull_requests: [], note: NO_PROVIDER_NOTE } : null;
}
/** What a read says when the project's remote is not on GitHub: no pull requests exist to read. */
const NO_PROVIDER_NOTE = "no pull request provider for this remote";

/** A thread's pull request and checks, read through the engine's `gh` call. */
async function delegatePull(
  target: RuleThread,
  host = splitThreadKey(target.id).host,
): Promise<{ branch?: string; pull?: DelegatePull; error?: Failure; note?: string }> {
  const cwd = projectOf(target);
  if (!cwd) return { error: failure("failed", "this thread has no project folder") };
  const worktree = target.worktree;
  try {
    // Ask for the branch the worktree is on now, not the recorded one: a
    // delegate may have renamed it, and the engine echoes the branch it was
    // asked for, so naming the recorded one would hide the rename (and its
    // pull request) from status, merge and cleanup.
    // A thread on another machine is read by `gh` there, in its own folders.
    let answer = parsePullState(
      await requestOn(host, Method.pullState, { cwd: worktree?.path ?? cwd }, 60_000),
    );
    // A remote on another forge has no pull requests to read: that is an answer, not a failure.
    if (answer.error?.code === "no_provider") return { note: NO_PROVIDER_NOTE };
    // A detached HEAD has no branch to look up, so fall back to the recorded
    // one rather than reporting no pull request at all.
    if (answer.error && worktree?.branch) {
      answer = parsePullState(
        await requestOn(host, Method.pullState, { cwd, branch: worktree.branch }, 60_000),
      );
    }
    if (answer.error?.code === "no_provider") return { note: NO_PROVIDER_NOTE };
    if (answer.error) return { error: pullFailure(answer.error) };
    return { branch: answer.branch, pull: answer.pull };
  } catch (error) {
    return {
      error: failure("target_unavailable", `the pull request could not be read: ${errorText(error)}`),
    };
  }
}

/**
 * The delegates a thread has on `host` ("" is this computer): the targets of
 * its links, newest first, one row each, capped so the gh reads stay well
 * inside the request timeout.
 */
function delegateTargets(caller: RuleThread, host: string): { targets: RuleThread[]; omitted: number } {
  const ids = new Set(
    book()
      .links.filter((link) => link.orchestrator === caller.id && splitThreadKey(link.target).host === host)
      .map((l) => l.target),
  );
  ids.delete(caller.id);
  const all = [...ids].reverse().flatMap((id) => ruleThread(useApp.getState(), id) ?? []);
  return { targets: all.slice(0, MAX_DELEGATES), omitted: Math.max(0, all.length - MAX_DELEGATES) };
}

/**
 * `thread_status`: where each delegate stands — worktree, branch, pull request
 * and checks — in one call, instead of combining read_session, list_pull_requests
 * and a shell poll of `gh pr checks`. A read: no approval, same project. The
 * delegates are read concurrently, so a dozen do not add up to a timeout.
 */
async function threadStatus(
  caller: RuleThread,
  target: string | undefined,
  host: string,
): Promise<JsonObject> {
  const asked = target ? undefined : delegateTargets(caller, host);
  const targets = target ? [manageTarget(caller, target, true)] : (asked?.targets ?? []);
  const entries: RuleThread[] = [];
  for (const entry of targets) {
    if (isFailure(entry)) return failureJson(entry);
    if (entry.id === caller.id) {
      return failureJson(
        failure("not_allowed", "a thread reports its own pull request through list_pull_requests"),
      );
    }
    entries.push(entry);
  }
  const settled = await Promise.allSettled(
    entries.map(async (entry) => {
      // Another machine's branch and pull request are read by `gh` in its own
      // folders, over its connection, and its recorded activity is asked of
      // it too: that carries the provider error a failed remote turn left
      // (issue 3525).
      const [state, activity] = await Promise.all([
        delegatePull(entry),
        entry.running
          ? activityFrom(entry.id, { order: "newest", limit: 1 }).then((read) => read.answer ?? null)
          : null,
      ]);
      return { entry, state, activity };
    }),
  );
  const delegates: JsonObject[] = [];
  for (const result of settled) {
    if (result.status === "rejected") {
      delegates.push({ error: { code: "failed", message: errorText(result.reason) } });
      continue;
    }
    const { entry, state, activity } = result.value;
    const recorded = Array.isArray(activity?.items) ? obj(activity.items[0]) : undefined;
    const recordedAt = typeof recorded?.at === "string" ? Date.parse(recorded.at) : NaN;
    const transcript = useApp.getState().threads[entry.id];
    const transcriptAt = activityOf(transcript?.items ?? [], transcript?.children).lastAt;
    const times = [transcriptAt, Number.isFinite(recordedAt) ? recordedAt : undefined].filter(
      (time): time is number => time !== undefined,
    );
    const lastAt = times.length ? Math.max(...times) : entry.updatedAt;
    delegates.push({
      session_id: entry.id,
      title: entry.title,
      ...(targetMachine(entry.id) ? { machine: targetMachine(entry.id) } : {}),
      status: entry.status ?? (entry.running ? "running" : "idle"),
      ...(entry.running && lastAt !== undefined ? { idle_ms: Math.max(0, Date.now() - lastAt) } : {}),
      ...(entry.running && activity?.last_provider_error
        ? { last_provider_error: activity.last_provider_error }
        : {}),
      project: projectOf(entry),
      // The worktree's and the pull request's branch are named only where they differ from `branch`.
      ...(entry.worktree
        ? {
            worktree: {
              path: entry.worktree.path,
              ...(entry.worktree.branch !== state.branch ? { branch: entry.worktree.branch } : {}),
            },
          }
        : {}),
      ...(state.branch ? { branch: state.branch } : {}),
      pull: state.pull
        ? {
            number: state.pull.number,
            title: state.pull.title,
            state: state.pull.state,
            merged: state.pull.merged,
            draft: state.pull.draft,
            ...(state.pull.branch !== state.branch ? { branch: state.pull.branch } : {}),
            head_sha: state.pull.headSha,
            ci: state.pull.ci,
            failing: state.pull.failing,
            pending: state.pull.pending,
            passed: state.pull.passed,
            ...(state.pull.url ? { url: state.pull.url } : {}),
          }
        : null,
      ...(state.note ? { pull_note: state.note } : {}),
      ...(state.error ? { error: { code: state.error.code, message: state.error.message } } : {}),
    });
  }
  return {
    delegates,
    ...(asked?.omitted ? { omitted_delegates: asked.omitted } : {}),
    ...(delegates.length === 0 ? { note: "this thread has no delegates; start one with start_session" } : {}),
  };
}

// ---------------------------------------------------------------- delegate status (issue 3651)

/** How long a read pull request stands before the next read; the panel and "status" share it. */
const PULL_READ_TTL_MS = 30_000;
const pullReads = new Map<string, { at: number; value: StatusPull }>();
const pullFlights = new Map<string, Promise<StatusPull>>();

/** The pull request last read for a delegate, however old; undefined before the first read. */
export function cachedDelegatePull(id: string): StatusPull | undefined {
  return pullReads.get(id)?.value;
}

/**
 * A delegate's pull request and checks, the same read `thread_status` makes (`delegatePull`), kept
 * for {@link PULL_READ_TTL_MS} and shared by every caller in flight, so a panel that redraws and a
 * "status" typed meanwhile cost one `gh` read. A delegate outside a worktree has no branch of its
 * own to look up, so it reads as having none without asking.
 */
export function readDelegatePull(id: string, maxAgeMs = PULL_READ_TTL_MS): Promise<StatusPull> {
  const hit = pullReads.get(id);
  if (hit && Date.now() - hit.at < maxAgeMs) return Promise.resolve(hit.value);
  const flying = pullFlights.get(id);
  if (flying) return flying;
  const entry = ruleThread(useApp.getState(), id);
  if (!entry?.worktree) return Promise.resolve({ pull: null });
  const read = delegatePull(entry)
    .then((state): StatusPull => {
      const value: StatusPull = state.error
        ? { pull: null, note: state.error.message }
        : { pull: state.pull ?? null, ...(state.note ? { note: state.note } : {}) };
      pullReads.set(id, { at: Date.now(), value });
      return value;
    })
    .finally(() => pullFlights.delete(id));
  pullFlights.set(id, read);
  return read;
}

/** The delegates a thread has, newest first and one per thread, for the status panel and reply. */
export function delegateStatusInputs(s: AppState, orchestrator: string, max = 12): StatusInput[] {
  const seen = new Set<string>();
  const out: StatusInput[] = [];
  const links = s.links.links;
  for (let i = links.length - 1; i >= 0 && out.length < max; i--) {
    const link = links[i]!;
    if (link.orchestrator !== orchestrator || link.target === orchestrator || seen.has(link.target)) continue;
    seen.add(link.target);
    const thread = s.threads[link.target];
    const session = sessionById(s, link.target);
    const activity = activityOf(thread?.items ?? []);
    out.push({
      id: link.target,
      title: session?.title || link.targetTitle,
      state: LINK_AGENT_STATE[stateOf(s, link)],
      lastAt: activity.lastAt ?? session?.updatedAt,
      lastTool: activity.lastTool,
      harness: link.harness ?? thread?.harness?.id ?? session?.harness,
      account: thread?.harness?.account ?? session?.account,
      model:
        (thread?.configOptions && selectOption(thread.configOptions, "model")?.current) || session?.model,
      branch: thread?.worktree?.branch ?? session?.worktree?.branch,
      pull: cachedDelegatePull(link.target),
    });
  }
  return out;
}

/**
 * A bare "status" in a thread that has delegates is answered here, from what the app already
 * knows: the user's message and a local reply row go into the transcript, and no model turn runs
 * (each one resent the whole context and cost as much as any other). The pull requests are read
 * after the row is drawn and fill in as they arrive. False for a thread with no delegates, where
 * the model answers as it always did.
 */
export function answerStatusLocally(threadId: string, text: string): boolean {
  if (!isStatusQuery(text)) return false;
  const s = useApp.getState();
  if (!s.threads[threadId]) return false;
  const inputs = delegateStatusInputs(s, threadId, 20);
  if (!inputs.length) return false;
  const now = Date.now();
  const rows = statusRows(inputs, now);
  const statusId = localId("status");
  updateThread(threadId, (t) => ({
    ...t,
    items: [
      ...t.items,
      { kind: "user", id: localId("user"), text: text.trim(), at: now },
      { kind: "status", id: statusId, at: now, rows },
    ],
  }));
  for (const row of rows) {
    if (row.pull && Date.now() - now < PULL_READ_TTL_MS) continue;
    void readDelegatePull(row.id).then((pull) =>
      updateThread(threadId, (t) => ({
        ...t,
        items: t.items.map((item) =>
          item.kind === "status" && item.id === statusId
            ? { ...item, rows: item.rows.map((r) => (r.id === row.id ? { ...r, pull } : r)) }
            : item,
        ),
      })),
    );
  }
  return true;
}

/**
 * `merge_when_green`: waits for a delegate's checks and squash-merges, with the
 * semantics of scripts/pr-merge.sh (advisory jobs ignored, two quiet rounds,
 * a failing check stops it). The engine waits; the app asks the user first.
 */
async function mergeWhenGreen(
  caller: RuleThread,
  call: Extract<ReturnType<typeof parseCall>, { tool: "merge_when_green" }>,
): Promise<JsonObject> {
  const tracked = call.callId ? trackCall(call.callId, caller.id) : undefined;
  try {
    return await mergeTracked(caller, call, tracked);
  } finally {
    if (call.callId && droppable.get(call.callId) === tracked) droppable.delete(call.callId);
  }
}

async function mergeTracked(
  caller: RuleThread,
  call: Extract<ReturnType<typeof parseCall>, { tool: "merge_when_green" }>,
  tracked: Droppable | undefined,
): Promise<JsonObject> {
  const target = call.number ? caller : manageTarget(caller, call.target);
  if (isFailure(target)) return failureJson(target);
  const state = await delegatePull(target);
  if (tracked?.dropped) return failureJson(droppedFailure());
  if (state.error) return failureJson(state.error);
  if (state.note) return failureJson(failure("not_found", state.note));
  const pull = state.pull;
  if (!pull) {
    return failureJson(
      failure("not_found", `“${target.title}” has no pull request on ${state.branch ?? "its branch"} yet`),
    );
  }
  if (call.number && pull.number !== call.number)
    return failureJson(failure("not_allowed", "the PR number must match this thread’s current branch"));
  if (pull.merged) {
    return {
      session_id: target.id,
      number: pull.number,
      merged: true,
      state: "merged",
      ...(pull.url ? { url: pull.url } : {}),
    };
  }
  const no = await permitted(
    caller,
    `merge:${target.id}:${pull.number}`,
    {
      kind: "merge",
      caller: caller.id,
      callerTitle: caller.title,
      target: target.title,
      detail: `It would wait for pull request #${pull.number}'s checks and squash-merge it once every one passes, then delete its branch. A failing check stops it and reports the names.`,
    },
    tracked,
  );
  if (no) return failureJson(no);
  // Nothing merges for a call that was dropped while it waited for the approval.
  if (tracked?.dropped) return failureJson(droppedFailure());
  const finishMerge = async (): Promise<JsonObject> => {
    // The engine's merge loop runs under the call's id, so a drop ends it before it merges.
    const mergeId = call.number ? undefined : call.callId;
    const stopMerge = () => {
      if (mergeId) void request(Method.pullMergeCancel, { mergeId }, 10_000).catch(() => {});
    };
    if (tracked) tracked.onDrop.add(stopMerge);
    let result: Json;
    try {
      result = await request(
        Method.pullMerge,
        {
          cwd: projectOf(target),
          number: pull.number,
          ...(state.branch ? { branch: state.branch } : {}),
          timeoutMs: call.timeoutMs,
          ...(mergeId ? { mergeId } : {}),
        },
        call.timeoutMs + 120_000,
      );
    } catch (error) {
      // The app gives up on the answer, so the loop must not go on to merge unseen.
      stopMerge();
      return failureJson(failure("failed", `the merge did not answer: ${errorText(error)}`));
    } finally {
      tracked?.onDrop.delete(stopMerge);
    }
    const error = parsePullError(result);
    if (error) return failureJson(pullFailure(error));
    const outcome = parseMergeOutcome(result);
    if (!outcome) return failureJson(failure("failed", "the engine answered without a merge result"));
    if (outcome.merged) {
      // The branch's story is part of the link's: the chip's list says it merged.
      setBook((b) => noteLinkEvent(b, caller.id, target.id, "merged"));
    }
    return {
      session_id: target.id,
      number: outcome.number,
      state: outcome.state,
      merged: outcome.merged,
      ...(outcome.url ? { url: outcome.url } : {}),
      ...(outcome.failing.length ? { failing: outcome.failing } : {}),
      waited_secs: outcome.waitedSecs,
      detail: outcome.message,
    };
  };
  if (call.number) {
    void finishMerge().then((result) =>
      deliver(caller.id, `Pull request #${pull.number}: ${JSON.stringify(result)}`, "", ""),
    );
    return {
      number: pull.number,
      state: "watching",
      detail: "Merge approved and handed to RustCode. End the turn; the result will wake this thread.",
    };
  }
  return finishMerge();
}

/**
 * `cleanup_worktree`: removes a merged delegate's worktree and branch, then
 * settles it, after the engine confirms nothing unique is lost (git cherry,
 * allowing a squash merge's content). A branch the delegate renamed after this
 * read is accepted by the engine when that branch's pull request merged. Refuses
 * a dirty worktree or one still running. The user approves it.
 */
async function cleanupWorktree(
  caller: RuleThread,
  call: Extract<ReturnType<typeof parseCall>, { tool: "cleanup_worktree" }>,
): Promise<JsonObject> {
  const target = manageTarget(caller, call.target);
  if (isFailure(target)) return failureJson(target);
  if (target.id === caller.id) {
    return failureJson(failure("not_allowed", "a thread cannot clean up the checkout it runs in"));
  }
  if (!target.worktree) {
    return failureJson(failure("not_allowed", `“${target.title}” does not run in a worktree`));
  }
  if (target.running) {
    return failureJson(failure("conflict", `“${target.title}” is working; wait for its turn to finish`));
  }
  // A pull request read that fails leaves `merged` false, so the engine falls
  // back to the worktree's own `git cherry` check, which is the stricter one.
  const state = await delegatePull(target);
  const merged = state.pull?.merged ?? false;
  const branch = target.worktree.branch;
  const no = await permitted(caller, `cleanup:${target.id}`, {
    kind: "cleanup",
    caller: caller.id,
    callerTitle: caller.title,
    target: target.title,
    detail: `It would remove the worktree at ${target.worktree.path} and delete the branch ${branch}${merged ? " (its pull request merged)" : ""}, then ${call.settle ? "settle the thread" : "leave the thread as it is"}. It refuses when anything there would be lost.`,
  });
  if (no) return failureJson(no);
  let result: Json;
  try {
    result = await request(
      V2Method.worktreeCleanup,
      {
        path: target.worktree.path,
        branch,
        headSha: state.pull?.headSha ?? "",
        merged,
      },
      120_000,
    );
  } catch (error) {
    if (error instanceof RpcError) {
      const reason = cleanupRefusal(error.data);
      if (reason) {
        // "unknown" is not a refusal to fix but a git that could not answer.
        const code = reason === "unknown" ? "target_unavailable" : "conflict";
        // The engine's detail (which commits or files, which branch) is in
        // `error.data.message`; `error.message` alone is "Invalid params".
        return failureJson(failure(code, `${reason}: ${errorText(error)}`));
      }
    }
    return failureJson(failure("failed", `the worktree could not be cleaned up: ${errorText(error)}`));
  }
  const cleanup = obj(result)?.cleanup;
  // The worktree is gone and the engine moved the thread back to the
  // repository; the store must not keep offering the old folder.
  const repo = projectOf(target);
  useApp.setState((s) => ({
    sessions: s.sessions.map((x) => (x.id === target.id ? { ...x, worktree: undefined, cwd: repo } : x)),
  }));
  updateThread(target.id, (t) => ({ ...t, worktree: undefined, cwd: repo }));
  const answer: JsonObject = {
    session_id: target.id,
    removed: bool(obj(cleanup)?.removed) ?? true,
    branch: str(obj(cleanup)?.branch) ?? branch,
    branch_removed: bool(obj(cleanup)?.branch_removed) ?? false,
  };
  // A branch the delegate renamed: the engine removes the recorded one too
  // when it holds nothing unique, and says so.
  const recorded = str(obj(cleanup)?.recorded_branch);
  if (recorded) {
    answer.recorded_branch = recorded;
    answer.recorded_branch_removed = bool(obj(cleanup)?.recorded_branch_removed) ?? false;
  }
  setBook((b) => noteLinkEvent(b, caller.id, target.id, "cleaned"));
  if (call.settle) {
    await setThreadStatus(target.id, "done");
    answer.settled = true;
  }
  return answer;
}

/**
 * `worktree_status`: where the worktree threads stand — the branch each one is
 * on, its distance from the ref it tracks, how many files are uncommitted, its
 * last commit, and the pull request its branch heads. The git reads are one
 * engine call; the `gh` reads are bounded like `thread_status`'s. A read: no
 * approval.
 */
/** The worktree threads this computer runs, and so can read git for. */
function localWorktrees(): RuleThread[] {
  return allThreads().filter((t) => t.worktree && !isRemoteThread(t.id));
}

async function worktreeStatus(
  caller: RuleThread,
  call: Extract<ReturnType<typeof parseCall>, { tool: "worktree_status" }>,
  machine: MachineRef = localMachine(),
): Promise<JsonObject> {
  const host = machine.host;
  const where = useApp.getState().readAcrossProjects;
  if (call.allProjects && !where && !host) return reachRefused();
  const entries: RuleThread[] = [];
  // A named machine's worktrees are read by the engine there, every one it has:
  // its folders are in no project of this computer's.
  const all = host
    ? allThreads().filter((t) => t.worktree && splitThreadKey(t.id).host === host)
    : localWorktrees();
  if (call.target) {
    const target = manageTarget(caller, call.target, !!host);
    if (isFailure(target)) return failureJson(target);
    if (!target.worktree) {
      return { worktrees: [], note: `“${target.title}” does not run in a worktree` };
    }
    entries.push(target);
  } else {
    const rows = all.filter((t) => host || call.allProjects || reachesThread(caller, t));
    entries.push(...rows.slice(0, MAX_WORKTREES));
  }
  const omitted = call.target ? 0 : Math.max(0, all.length - entries.length);
  if (!entries.length) {
    return {
      worktrees: [],
      note: "no worktree threads here; start one with start_session and worktree",
    };
  }
  // One engine call reads every worktree's branch, distance and changed files:
  // a git process per path, not a `git` command from the agent.
  const paths = entries.flatMap((t) => (t.worktree ? [t.worktree.path] : []));
  let states: WorktreeState[] = [];
  let stateError: string | undefined;
  try {
    states = parseWorktreeStates(await requestOn(host, V2Method.worktreeState, { paths }, 60_000));
  } catch (error) {
    stateError = errorText(error);
  }
  // A pull request is one `gh` read, run together and capped like `thread_status`.
  const asking = entries.slice(0, MAX_WORKTREE_PULLS);
  const pulls = await Promise.allSettled(asking.map((entry) => delegatePull(entry, host)));
  const worktrees = entries.map((entry, index) => {
    const state = states.find((row) => row.path === entry.worktree?.path);
    const read = index < pulls.length ? pulls[index] : undefined;
    const pull = read?.status === "fulfilled" ? read.value : undefined;
    return {
      session_id: entry.id,
      title: entry.title,
      ...(call.allProjects || host ? { project: projectOf(entry) } : {}),
      status: entry.status ?? (entry.running ? "running" : "idle"),
      branch: state?.branch ?? entry.worktree?.branch ?? "",
      path: state?.path ?? entry.worktree?.path ?? "",
      ...(state ? { exists: state.exists } : {}),
      // The ref is named wherever a count is given, so `ahead: 3` is never
      // ambiguous about what it is ahead of.
      ...(state?.against ? { against: state.against } : {}),
      ...(state?.ahead === undefined ? {} : { ahead: state.ahead }),
      ...(state?.behind === undefined ? {} : { behind: state.behind }),
      ...(state?.dirty === undefined ? {} : { dirty: state.dirty }),
      ...(state?.lastCommit ? { last_commit: state.lastCommit } : {}),
      ...(state?.lastCommitSubject ? { last_commit_subject: state.lastCommitSubject } : {}),
      ...(state?.error ? { error: { code: "unavailable", message: state.error } } : {}),
      ...(pull?.error
        ? { pull_error: { code: pull.error.code, message: pull.error.message } }
        : pull
          ? {
              ...(pull.note ? { pull_note: pull.note } : {}),
              pull: pull.pull
                ? {
                    number: pull.pull.number,
                    title: pull.pull.title,
                    state: pull.pull.state,
                    merged: pull.pull.merged,
                    draft: pull.pull.draft,
                    ...(pull.pull.branch !== (state?.branch ?? entry.worktree?.branch)
                      ? { branch: pull.pull.branch }
                      : {}),
                    ...(pull.pull.url ? { url: pull.pull.url } : {}),
                  }
                : null,
            }
          : {}),
    };
  });
  const pullsOmitted = entries.length - asking.length;
  return {
    worktrees,
    ...(omitted ? { omitted_worktrees: omitted } : {}),
    ...(pullsOmitted ? { pulls_omitted: pullsOmitted } : {}),
    ...(stateError ? { note: `each worktree's git state could not be read: ${stateError}` } : {}),
  };
}

/** The failure code each handoff refusal answers with: the reason itself says what to fix. */
const HANDOFF_CODES: Record<HandoffRefusal, string> = {
  no_worktree: "not_allowed",
  has_worktree: "not_allowed",
  not_checkout: "not_allowed",
  dirty: "conflict",
  dirty_checkout: "conflict",
  unpushed_checkout: "conflict",
  ignored_files: "conflict",
  detached_checkout: "conflict",
  not_git: "target_unavailable",
  no_folder: "target_unavailable",
  unknown: "target_unavailable",
  failed: "failed",
};

/**
 * `handoff_worktree`: move a delegate's work into the user's checkout (so they
 * continue locally), or a thread out of the checkout into a worktree of its
 * own. The engine refuses rather than force — a dirty worktree or checkout, a
 * checkout on another branch with commits no remote has, a running thread —
 * and nothing is stashed, rebased, reset or deleted. The user approves it.
 */
async function handoffWorktree(
  caller: RuleThread,
  call: Extract<ReturnType<typeof parseCall>, { tool: "handoff_worktree" }>,
): Promise<JsonObject> {
  const target = manageTarget(caller, call.target);
  if (isFailure(target)) return failureJson(target);
  const toCheckout = call.to === "checkout";
  if (toCheckout && !target.worktree) {
    return failureJson(
      failure(
        "not_allowed",
        `“${target.title}” does not run in a worktree, so there is no branch to hand over`,
      ),
    );
  }
  if (!toCheckout && target.worktree) {
    return failureJson(failure("not_allowed", `“${target.title}” already runs in a worktree`));
  }
  // A thread mid-turn would keep writing in a folder it is losing.
  if (target.running) {
    return failureJson(failure("conflict", `“${target.title}” is working; wait for its turn to finish`));
  }
  const checkout = projectOf(target);
  const worktree = target.worktree;
  const no = await permitted(caller, `handoff:${target.id}:${call.to}`, {
    kind: "handoff",
    caller: caller.id,
    callerTitle: caller.title,
    target: target.title,
    detail: toCheckout
      ? `It would remove the worktree at ${worktree?.path}, switch your checkout ${checkout} to ${worktree?.branch}, and move “${target.title}” there so you can continue locally. It refuses when your checkout has uncommitted changes or holds commits no remote has, or the worktree has uncommitted or ignored files. Nothing is stashed, rebased or reset, and the branch keeps every commit.`
      : `It would create a worktree on a new branch from your checkout's HEAD and move “${target.title}” into it, leaving your checkout untouched. It refuses when your checkout has uncommitted changes.`,
  });
  if (no) return failureJson(no);
  let result: Json;
  try {
    result = await request(
      V2Method.worktreeHandoff,
      { sessionId: target.id, to: call.to, title: target.title },
      120_000,
    );
  } catch (error) {
    if (error instanceof RpcError) {
      const reason = handoffRefusal(error.data);
      if (reason) {
        // The reason names what to fix, and the engine's detail (which files,
        // which commits, which branch) follows it; `error.message` alone is
        // "Invalid params".
        return failureJson(failure(HANDOFF_CODES[reason], `${reason}: ${errorText(error)}`));
      }
    }
    return failureJson(failure("failed", `the handoff did not happen: ${errorText(error)}`));
  }
  const moved = parseHandoff(result);
  if (!moved?.moved) {
    return failureJson(failure("failed", "the engine answered without a handoff"));
  }
  // The thread's folder follows the move; the store must not keep the old one.
  if (moved.to === "checkout") {
    const repo = moved.path ?? checkout;
    useApp.setState((s) => ({
      sessions: s.sessions.map((x) => (x.id === target.id ? { ...x, worktree: undefined, cwd: repo } : x)),
    }));
    updateThread(target.id, (t) => ({ ...t, worktree: undefined, cwd: repo }));
  } else {
    const path = moved.path ?? target.folder;
    const at = { path, branch: moved.branch ?? "", repo: moved.repo ?? checkout };
    useApp.setState((s) => ({
      sessions: s.sessions.map((x) => (x.id === target.id ? { ...x, worktree: at, cwd: path } : x)),
    }));
    updateThread(target.id, (t) => ({ ...t, worktree: at, cwd: path }));
  }
  setBook((b) => noteLinkEvent(b, caller.id, target.id, "handed_off"));
  return {
    session_id: target.id,
    to: moved.to,
    moved: true,
    ...(moved.worktreeRemoved === undefined ? {} : { worktree_removed: moved.worktreeRemoved }),
    ...(moved.switched === undefined ? {} : { switched: moved.switched }),
    ...(moved.branch ? { branch: moved.branch } : {}),
    ...(moved.path ? { path: moved.path } : {}),
    ...(moved.detail ? { detail: moved.detail } : {}),
  };
}

// ---------------------------------------------------------------- managing threads

type UpdateCall = Extract<ReturnType<typeof parseCall>, { tool: "update_session" }>;
type ConfigureCall = Extract<ReturnType<typeof parseCall>, { tool: "configure_session" }>;

const STATUS_OF_CHOICE = { active: "idle", review: "review", settled: "done" } as const;

/** The thread a management tool acts on: the caller's own when none is named. */
/**
 * The thread a management call acts on. A thread on another machine is only
 * reachable when the call named that machine (`machine`): the app refuses to
 * act there by a bare guess, and the tools that cannot work across machines
 * say where the thread runs instead (`targetMachine`).
 */
function manageTarget(caller: RuleThread, id: string | undefined, machine = false): RuleThread | Failure {
  const target = id ? ruleThread(useApp.getState(), id) : caller;
  if (!target) return failure("not_found", "no thread has that id; use list_sessions");
  if (isRemoteThread(target.id) && !machine) {
    return failure(
      "not_allowed",
      `“${target.title}” runs on ${targetMachine(target.id) ?? "another computer"}: do that there`,
    );
  }
  if (isRemoteThread(target.id)) return target;
  return checkManage(caller, target) ?? target;
}

/** The machine a thread key names, as people say it; undefined for this computer. */
function targetMachine(id: string): string | undefined {
  const { host } = splitThreadKey(id);
  return host ? useApp.getState().machineNames[host] : undefined;
}

/** The changes `update_session` and `organize_threads` share, as one list. */
interface ManagedChange {
  title?: string;
  pinned?: boolean;
  unread?: boolean;
}

/** Whether a thread is the list's unread one: it finished something the user has not seen. */
const isUnread = (id: string) => {
  const s = useApp.getState();
  return id in s.finished || id in s.failed;
};

/**
 * Applies a thread's title, pin and unread flag with the store calls the app's own
 * controls use, and counts the ones that did something: a value the thread already
 * has is not a change, and `unread` is a two-way flag (`true` marks it unread,
 * `false` reads it, as opening the thread does).
 */
async function applyManagedChange(
  target: RuleThread,
  change: ManagedChange,
): Promise<{ applied: number; renameFailed: boolean }> {
  let applied = 0;
  const live = () => ruleThread(useApp.getState(), target.id) ?? target;
  if (change.title !== undefined && change.title !== live().title) {
    if (!(await renameThread(target.id, change.title))) return { applied, renameFailed: true };
    applied += 1;
  }
  if (change.pinned !== undefined && useApp.getState().pinned.includes(target.id) !== change.pinned) {
    togglePin(target.id);
    applied += 1;
  }
  if (change.unread !== undefined && isUnread(target.id) !== change.unread) {
    if (change.unread) markUnread(target.id);
    else clearFinished(target.id);
    applied += 1;
  }
  return { applied, renameFailed: false };
}

async function updateSession(caller: RuleThread, call: UpdateCall, machine = false): Promise<JsonObject> {
  const target = manageTarget(caller, call.target, machine);
  if (isFailure(target)) return failureJson(target);
  // A thread's name lives on the computer that owns it (`renameThread`).
  if (call.title && isRemoteThread(target.id)) {
    return failureJson(
      failure(
        "not_allowed",
        `a thread on ${targetMachine(target.id) ?? "another computer"} is renamed there`,
      ),
    );
  }
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
      machine: targetMachine(target.id),
    });
    if (no) return failureJson(no);
  }
  // An agent naming its own thread leaves a name the user chose alone (issue 3599); renaming
  // another thread is the user's own approval above.
  const titleStatus = call.title && target.id === caller.id ? await userTitled(target.id) : false;
  const keepTitle = titleStatus !== false;
  const applied = await applyManagedChange(target, {
    title: keepTitle ? undefined : call.title,
    pinned: call.pinned,
    unread: call.unread,
  });
  if (applied.renameFailed) return failureJson(failure("failed", "the thread could not be renamed"));
  if (call.status) await setThreadStatus(target.id, STATUS_OF_CHOICE[call.status]);
  if (call.snoozedUntil !== undefined) await setSnoozed(target.id, call.snoozedUntil);
  const after = ruleThread(useApp.getState(), target.id);
  const until = useApp.getState().snoozed[target.id];
  return {
    session_id: target.id,
    title: after?.title ?? target.title,
    pinned: useApp.getState().pinned.includes(target.id),
    // The status this call set, not the live one: a thread settled in the middle of its turn still
    // runs, and answering "running" read as if the settle had been refused (issue 3529). It is
    // settled now and leaves the active list when its turn ends; `running` says it still works.
    status: call.status ? STATUS_OF_CHOICE[call.status] : (after?.status ?? "idle"),
    running: !!useApp.getState().threads[target.id]?.running,
    snoozed_until: until ? new Date(until).toISOString() : null,
    ...(keepTitle && {
      title_kept:
        titleStatus === null
          ? "Could not check whether the user named this thread; its title stays as it is."
          : "The user named this thread, so its title stays as it is.",
    }),
  };
}

/** Null means the shell could not determine whether the user named the thread. */
async function userTitled(id: string): Promise<boolean | null> {
  if (!hasShell()) return null;
  return shellCall<boolean>("thread_user_titled", { id }).catch(() => null);
}

type AnswerCall = Extract<ReturnType<typeof parseCall>, { tool: "answer_request" }>;
type OrganizeCall = Extract<ReturnType<typeof parseCall>, { tool: "organize_threads" }>;

type Scope = ReturnType<typeof optionScope>;

/** What each scope is called when a choice cannot be answered with it. */
const SCOPE_WORD: Record<Scope, string> = {
  once: "allow once",
  hour: "allow for an hour",
  always: "allow always",
  decline: "decline",
  other: "another answer",
};

/**
 * The option a `choice` answers with: the scope it names, and no other. A request
 * that offers only a standing answer is refused rather than answered once, because
 * the engine records the grant the option carries, not the word the caller used.
 */
function optionFor(request: PermissionRequest, choice: ApprovalChoice): PermissionOption | null {
  const wanted = choice === "deny" ? "decline" : choice === "allow_always" ? "always" : "once";
  return request.options.find((o) => optionScope(o) === wanted) ?? null;
}

/** Why a choice has no option here, naming the scopes the request does offer. */
function scopeRefusal(request: PermissionRequest, choice: ApprovalChoice): Failure {
  const offers = [...new Set(request.options.map(optionScope))].map((scope) => SCOPE_WORD[scope]);
  return failure("invalid", `that request offers ${offers.join(", ")}, not “${ANSWER_LABEL[choice]}”`);
}

/** The options a question offers as answers: the allow and plain ones its card lists, not a reject. */
const questionAnswers = (request: PermissionRequest) =>
  request.options.filter((o) => !o.kind || o.kind === "allow_once" || o.kind === "allow_always");

/** One approval's answer, in the words the grant card and the notice use. */
const ANSWER_LABEL: Record<ApprovalChoice, string> = {
  allow_once: "allow it once",
  allow_always: "always allow it",
  deny: "decline it",
};

/** How one call answers one request; a question answered by words has `text` instead of a pick. */
interface Answering {
  option: PermissionOption | null;
  chosen: PermissionOption | null;
  text: string | null;
}

/**
 * The answer one call gives one request as it is now: an approval's option, a
 * question's option by id or its text, or a question's own Decline. Checked again
 * after the user answered the card, since that request may be gone by then.
 */
function answeringFor(request: PermissionRequest, call: AnswerCall): Answering | Failure {
  const question = request.question;
  if (!question) {
    if (call.answer) return failure("invalid", "that request is an approval: give choice");
    const option = optionFor(request, call.choice ?? "allow_once");
    return option ? { option, chosen: null, text: null } : scopeRefusal(request, call.choice ?? "allow_once");
  }
  // A question's card offers the same Decline an approval's does.
  if (call.choice === "deny") {
    const option = optionFor(request, "deny");
    return option ? { option, chosen: null, text: null } : scopeRefusal(request, "deny");
  }
  if (call.answer === undefined) {
    return failure("invalid", "that request is a question: give answer, or choice deny to decline it");
  }
  const answers = questionAnswers(request);
  const chosen = answers.find((o) => o.id === call.answer) ?? null;
  if (chosen) return { option: chosen, chosen, text: null };
  if (!answers.length) return failure("invalid", "that question offers no answer this call can give");
  // The store picks the carrier option, as the card's text field does.
  return { option: null, chosen: null, text: call.answer };
}

/**
 * `answer_request`: answer one approval or question a thread waits on. The
 * caller answers a thread below it in its own delegation tree, never itself,
 * and never one in another project; the card the user would have clicked is
 * the one this answers (`state/store.ts` `answerPermission`, `answerQuestionText`).
 */
async function answerRequest(caller: RuleThread, call: AnswerCall): Promise<JsonObject> {
  const target = ruleThread(useApp.getState(), call.target);
  // Another machine's thread is in no project of this computer's: the tree rule below
  // (a thread this one delegated to) is what lets it be answered from here.
  const refused = target && isRemoteThread(target.id) ? null : checkManage(caller, target);
  if (refused || !target) return failureJson(refused ?? failure("not_found", "no such thread"));
  if (target.id === caller.id) {
    return failureJson(failure("not_allowed", "a thread cannot answer its own request"));
  }
  if (!ancestorsOf(book(), target.id).has(caller.id)) {
    return failureJson(
      failure(
        "not_allowed",
        `“${target.title}” is not a thread this one delegated to; only a thread below this one in its delegation tree can be answered`,
      ),
    );
  }
  const waiting = () =>
    useApp.getState().threads[target.id]?.permissions.find((p) => String(p.rpcId) === call.requestId);
  const request = waiting();
  if (!request) {
    return failureJson(
      failure("not_found", "that request is not waiting any more; list_pending_requests again"),
    );
  }
  const answered = answeringFor(request, call);
  if (isFailure(answered)) return failureJson(answered);
  const kind = request.question ? "question" : "approval";
  const asked = capped(request.question?.text || request.title, 200).text;
  const choice = call.answer === undefined ? (call.choice ?? "allow_once") : undefined;
  const detail =
    choice === "deny"
      ? `It would decline “${asked}”`
      : choice
        ? `It would ${ANSWER_LABEL[choice]} “${asked}”`
        : `It would answer “${asked}” with: ${call.answer}`;
  // The grant names the request, not only the thread: "always" for one question or
  // command must not answer a later, unrelated one without a card of its own.
  const key = `answer:${target.id}:${kind}:${asked}`;
  const no = await permitted(caller, key, {
    kind: "answer",
    caller: caller.id,
    callerTitle: caller.title,
    target: target.title,
    detail,
    machine: targetMachine(target.id),
  });
  if (no) return failureJson(no);
  // The user took their time: the card may have been answered in its own chat meanwhile.
  const current = waiting();
  if (!current) {
    return failureJson(failure("conflict", "that request was answered before this call reached it"));
  }
  const now = answeringFor(current, call);
  if (isFailure(now)) return failureJson(now);
  if (!current.question) answerPermission(current, now.option);
  else if (now.text !== null) answerQuestionText(current, now.text);
  else if (now.chosen) answerQuestionOptions(current, [now.chosen]);
  else answerPermission(current, now.option);
  const picked = now.option ?? now.chosen;
  return {
    session_id: target.id,
    request_id: call.requestId,
    thread: target.title,
    kind,
    // A question answered in words or by option id reports that answer; a declined
    // question and every approval report the choice, and the option it picked.
    ...(call.answer !== undefined
      ? { answer: call.answer, ...(picked ? { option: picked.name } : {}) }
      : { choice: choice ?? "allow_once", option: picked?.name ?? "" }),
  };
}

/**
 * `organize_threads`: the batch form of `update_session`. Every listed thread
 * of the caller's project gets the same changes, the user is asked once, and a
 * thread this caller cannot manage is skipped with the reason instead of
 * failing the batch. A change a thread already has is not made and not counted.
 */
async function organizeThreads(caller: RuleThread, call: OrganizeCall): Promise<JsonObject> {
  const decided: RuleThread[] = [];
  const skipped: JsonObject[] = [];
  for (const id of call.targets) {
    const target = manageTarget(caller, id);
    if (isFailure(target)) {
      skipped.push({ session_id: id, reason: target.message });
      continue;
    }
    decided.push(target);
  }
  if (!decided.length) {
    return failureJson(
      failure("not_allowed", skipped[0]?.reason?.toString() ?? "no thread of that list can be changed"),
    );
  }
  const state = () => useApp.getState();
  /** Which of the call's changes this thread does not have yet. */
  const missing = (target: RuleThread) => ({
    title: call.title !== undefined && call.title !== (ruleThread(state(), target.id) ?? target).title,
    pinned: call.pinned !== undefined && state().pinned.includes(target.id) !== call.pinned,
    unread: call.unread !== undefined && isUnread(target.id) !== call.unread,
    archived: call.archived !== undefined && (statusOf(state(), target.id) === "done") !== call.archived,
  });
  const wanted = decided.filter((target) => Object.values(missing(target)).some(Boolean));
  // Nothing to do is not a change to ask the user about: say so instead.
  if (!wanted.length) return { changed: 0, threads: [], skipped };
  const others = wanted.filter((target) => target.id !== caller.id);
  if (others.length) {
    const any = (key: keyof ReturnType<typeof missing>) => wanted.some((target) => missing(target)[key]);
    const changes = [
      any("title") && `rename ${others.length === 1 ? "it" : "them"} to “${call.title}”`,
      any("pinned") && (call.pinned ? "pin" : "unpin"),
      any("unread") && (call.unread ? "mark unread" : "mark read"),
      any("archived") && (call.archived ? "settle" : "bring back"),
    ].filter(Boolean);
    const names = others.map((target) => `“${target.title}”`).join(", ");
    const key = `organize:${decided
      .map((target) => target.id)
      .sort()
      .join(",")}`;
    const no = await permitted(caller, key, {
      kind: "organize",
      caller: caller.id,
      callerTitle: caller.title,
      target: names,
      detail: `It would ${changes.join(", ")} ${names}.`,
    });
    if (no) return failureJson(no);
  }
  // The status of `archived` goes in one write for the batch, as the sidebar's own
  // multi-select does it; the rest goes one thread at a time, and only where it changes.
  const settled = wanted.filter((target) => missing(target).archived).map((target) => target.id);
  if (settled.length) await setThreadsStatus(settled, call.archived ? "done" : "idle");
  const threads: JsonObject[] = [];
  for (const target of decided) {
    const applied = await applyManagedChange(target, {
      title: call.title,
      pinned: call.pinned,
      unread: call.unread,
    });
    if (applied.renameFailed) {
      skipped.push({ session_id: target.id, reason: "the thread could not be renamed" });
      continue;
    }
    if (!applied.applied && !settled.includes(target.id)) continue;
    threads.push({
      session_id: target.id,
      title: ruleThread(state(), target.id)?.title ?? target.title,
      pinned: state().pinned.includes(target.id),
      unread: isUnread(target.id),
      status: statusOf(state(), target.id),
      archived: statusOf(state(), target.id) === "done",
    });
  }
  return {
    changed: threads.length,
    threads,
    ...(skipped.length ? { skipped } : {}),
  };
}

async function interruptSession(caller: RuleThread, id: string): Promise<JsonObject> {
  const target = manageTarget(caller, id, true);
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
    machine: targetMachine(target.id),
  });
  if (no) return failureJson(no);
  // What goes down with it, counted before the cascade starts.
  const stopped = activeDescendants(book(), target.id).length;
  await cancel(target.id);
  return { session_id: target.id, interrupted: true, stopped };
}

async function configureSession(
  caller: RuleThread,
  call: ConfigureCall,
  machine = false,
): Promise<JsonObject> {
  const target = manageTarget(caller, call.target, machine);
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
      machine: targetMachine(target.id),
    });
    if (no) return failureJson(no);
  }
  // An agent's own configuring is not the user's pick: it leaves the effort memory alone
  // (state/store.ts `setConfigOption`).
  for (const [id, value] of changes) await setConfigOption(target.id, id, value, { remember: false });
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
  // The login a new thread on that harness would start on, as the picker moves
  // a thread: its remembered account, never a harness login the picker no
  // longer offers. A harness with no named accounts passes `null`, exactly as
  // before accounts existed.
  const account = newThreadAccountOn(to, lastAccountFor(harness), useApp.getState().signedOutLogins);
  if (!(await switchThreadHarness(target.id, harness, account))) {
    return failureJson(failure("failed", "the thread could not be moved"));
  }
  return { session_id: target.id, harness, moved: true, ...ranWith(target.id) };
}

/** What this project's open threads wait on. Only what they ask: answering stays with the user. */
function listPendingRequests(caller: RuleThread, machine?: MachineRef): JsonObject {
  const s = useApp.getState();
  const rank = machine?.host;
  const waiting = Object.values(s.threads)
    .filter((t) => t.permissions.length)
    .map((thread) => ({ thread, rule: ruleThread(s, thread.id) }))
    .filter(({ rule }) => {
      if (!rule) return false;
      // A named machine is the whole answer; without one, every thread this
      // thread reaches, wherever it runs.
      if (machine) return splitThreadKey(rule.id).host === rank;
      return reachesThread(caller, rule);
    })
    .flatMap(({ thread, rule }) => thread.permissions.map((request) => ({ thread, rule, request })))
    // Oldest first, as docs/orchestrator.md says: the card that has waited longest comes first.
    .sort((a, b) => (a.request.at ?? 0) - (b.request.at ?? 0));
  const requests = waiting.map(({ thread, rule, request }) => ({
    session_id: thread.id,
    request_id: String(request.rpcId),
    ...(targetMachine(thread.id) ? { machine: targetMachine(thread.id) } : {}),
    title: rule?.title ?? "",
    kind: request.question ? "question" : "approval",
    asks: request.question?.text ?? request.title,
    // An approval's choices and a question's answers, in the order the card lists them.
    options: request.options.map((o) => ({ id: o.id, name: o.name })),
  }));
  return { requests };
}

async function read(
  caller: RuleThread,
  targetId: string,
  lastTurns: number,
  offset: number,
): Promise<JsonObject> {
  const target = ruleThread(useApp.getState(), targetId);
  // Reading another machine's thread is that machine's to allow; the app adds
  // nothing to the rule, since the user named the machine and approves the read.
  const refused = checkMessage(
    caller,
    target,
    book(),
    true,
    useApp.getState().readAcrossProjects || isRemoteThread(targetId),
  );
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
    machine: targetMachine(target.id),
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

/**
 * The files a message carries: each under the caller's project or worktree,
 * measured by the shell (the web view cannot stat a file). `src-tauri`'s
 * `attached_files` enforces the same roots and the 5 MB ceiling for the paths
 * the pure rule above accepted.
 */
async function attachmentFiles(
  caller: RuleThread,
  paths: readonly string[],
): Promise<AttachedFile[] | Failure> {
  if (!paths.length) return [];
  if (paths.length > MAX_ATTACHMENT_FILES) {
    return failure("invalid", `a message attaches at most ${MAX_ATTACHMENT_FILES} files`);
  }
  const roots = attachmentRoots(caller);
  for (const path of paths) {
    const refused = attachmentRefusal(path, roots);
    if (refused) return refused;
  }
  try {
    return await shellCall<AttachedFile[]>("attached_files", { paths: [...paths], roots });
  } catch (error) {
    return failure("invalid", `the files could not be attached: ${errorText(error)}`);
  }
}

function message(
  caller: RuleThread,
  targetId: string,
  text: string,
  attachments: readonly string[] = [],
  steerNow = false,
  wait = false,
): Promise<JsonObject> {
  return withLinkGate(() => messageNow(caller, targetId, text, attachments, steerNow, wait));
}

/** Do not message a settled or archived thread. */
function messageTargetRefusal(state: AppState, targetId: string): Failure | null {
  if (targetId in state.archived) {
    return failure("conflict", "that thread is archived; unarchive it before messaging");
  }
  const target = ruleThread(state, targetId);
  if (target && statusOf(state, targetId) === "done") {
    return failure("conflict", "that thread is settled; set it to idle before messaging");
  }
  return null;
}

async function messageNow(
  caller: RuleThread,
  targetId: string,
  text: string,
  attachments: readonly string[] = [],
  steerNow = false,
  /** `wait: false` is one-way: no link, so the caller never shows Waiting and nothing comes back (issue 3544). */
  wait = false,
): Promise<JsonObject> {
  const state = useApp.getState();
  const target = ruleThread(state, targetId);
  // A thread on another machine is not this app's project topic to check: the
  // user named that machine, and the card below is the gate.
  const remote = isRemoteThread(targetId);
  const refused = checkMessage(caller, target, book(), false, false, delegateLimits(), remote, !wait);
  if (refused || !target) return failureJson(refused ?? failure("not_found", "no such thread"));
  const unavailable = messageTargetRefusal(state, targetId);
  if (unavailable) return failureJson(unavailable);
  // The files are checked before the user is asked about a message that could not carry
  // all of them, so a refusal never costs an approval.
  const files = await attachmentFiles(caller, attachments);
  if (isFailure(files)) return failureJson(files);
  const extras = files.length ? attachedExtras(files) : undefined;
  const withFiles = files.length
    ? `${text}\n\n${files.length === 1 ? "1 file attached" : `${files.length} files attached`}: ${files.map((f) => f.name).join(", ")}`
    : text;
  const no = await permitted(caller, target.id, {
    kind: "message",
    caller: caller.id,
    callerTitle: caller.title,
    target: target.title,
    detail: steerNow
      ? `Deliver into the running turn if supported; otherwise queue.\n\n${withFiles}`
      : withFiles,
    ...(!sameReach(caller, target) ? { project: projectOf(target) } : {}),
    machine: targetMachine(target.id),
  });
  if (no) return failureJson(no);
  // The user took their time: look again.
  const afterPermission = useApp.getState();
  const current = ruleThread(afterPermission, targetId);
  const again = checkMessage(caller, current, book(), false, false, delegateLimits(), remote, !wait);
  if (again) return failureJson(again);
  const stopped = messageTargetRefusal(afterPermission, targetId);
  if (stopped) return failureJson(stopped);
  // A thread whose last turn failed, or whose harness process was stopped for idling, is opened
  // and prompted like any other: the engine starts its harness again with the prompt, as it does
  // when the user types there. Only a thread that cannot be opened at all is unavailable.
  const openError = await openThread(target.id);
  const afterOpen = useApp.getState();
  const openedTarget = ruleThread(afterOpen, targetId);
  const noLongerReachable = checkMessage(
    caller,
    openedTarget,
    book(),
    false,
    false,
    delegateLimits(),
    remote,
    !wait,
  );
  if (noLongerReachable) return failureJson(noLongerReachable);
  const noLongerAvailable = messageTargetRefusal(afterOpen, targetId);
  if (noLongerAvailable) return failureJson(noLongerAvailable);
  const thread = afterOpen.threads[target.id];
  if (openError || !thread || thread.openFailed) {
    return failureJson(failure("target_unavailable", openError ?? "that thread could not be opened"));
  }
  const busy = thread.running;
  const session = afterOpen.sessions.find((x) => x.id === target.id);
  const harnessName = session?.harness
    ? afterOpen.harnesses?.harnesses.find((h) => h.id === session.harness)?.name
    : undefined;
  const id = wait
    ? addLink({
        orchestrator: caller.id,
        target: target.id,
        orchestratorTitle: caller.title,
        targetTitle: target.title,
        message: text,
        state: busy ? "queued" : "working",
        harness: session?.harness,
        harnessName,
        model: session?.model,
      })
    : undefined;
  // The sender's id (and the link it waits on) rides under the heading, so the target can tell two
  // senders with the same title apart and the app links the sender's name (issue 3539).
  const sent = delivered(text, caller.title, { session: caller.id, link: id });
  let delivery: "steered" | "queued" | "started";
  if (
    steerNow &&
    busy &&
    sendDecisionFor(thread, extras).action === "steer" &&
    (await steer(target.id, sent, extras, caller.id, id))
  ) {
    delivery = "steered";
    if (id) linkSteered(target.id, id, sent);
  } else {
    const before = new Set(useApp.getState().threads[target.id]?.queue.map((q) => q.id));
    deliver(target.id, sent, id, caller.id, extras);
    delivery = useApp.getState().threads[target.id]?.queue.some((q) => !before.has(q.id))
      ? "queued"
      : "started";
  }
  return {
    ...(id ? { link_id: id } : { return_to: "none" }),
    receipt: delivery,
    delivered: delivery,
    session_id: target.id,
    title: target.title,
    ...(files.length ? { attachments: files.map((f) => f.path) } : {}),
  };
}

function addLink(link: Omit<Link, "id" | "createdAt" | "events">): string {
  let id = "";
  setBook((b) => {
    const next = nextId(b, link.orchestrator);
    id = next.id;
    const at = Date.now();
    // The first event is what made the link: a thread started on a harness and model, or a message
    // sent into one, which is the line the chip's tip opens with.
    const meta = [link.harnessName ?? link.harness, link.model].filter(Boolean).join(" · ");
    const first = noteEvent({ ...link, id, createdAt: at }, link.started ? "started" : "message", at, meta);
    // Then the state it is in from this moment: a queued message reads `Queued`, a turn that starts
    // at once reads `Working`, without waiting for the next transition to write it.
    const seeded =
      link.state === "queued" || link.state === "working"
        ? noteEvent(first, link.state === "queued" ? "queued" : "working", at)
        : first;
    return { ...next.book, links: [...next.book.links, seeded] };
  });
  return id;
}

/** After the turn the target runs, never into it: the same queue a typed message uses. */
export function deliver(
  target: string,
  text: string,
  link: string | undefined,
  from: string,
  extras?: ComposeExtras,
) {
  const thread = useApp.getState().threads[target];
  if (thread?.running) {
    // `from` is who queued it, so that thread can correct it while it waits (`manageQueue`).
    updateThread(target, (t) => ({
      ...t,
      queue: [
        ...t.queue,
        {
          id: localId("queued"),
          text,
          ...(link ? { link } : {}),
          from,
          queuedAt: Date.now(),
          ...(extras ? { extras } : {}),
        },
      ],
    }));
  } else {
    void runPrompt(target, text, extras, { link, sender: from });
  }
}

/** The ids of messages that left a queue in this window: they started, or a person took them out. */
function sentQueueIds(): readonly string[] {
  // The book that records them is read the way start-up reads it, and kept once read (state/host.ts
  // `restoreQueuedThreads`); a window that never restored a queue reads storage here instead.
  return (queueBook ?? parseQueueBook(storedQueues())).acked;
}

/** A queued message that is no longer waiting: it started, or nothing ever had its id. */
function goneFromQueue(queueId: string | undefined): Failure {
  return sentQueueIds().includes(queueId ?? "")
    ? failure("conflict", "that message has already started, so it cannot be changed")
    : failure("not_found", "no queued message of that thread has that queue_id; list them again");
}

/**
 * What a queued message says, without the `delivered` attribution: that line is the app's, so an
 * agent reads its own text back and an edit says only what it says (`manageQueue`).
 */
function bodyOf(item: QueuedPrompt, from: string): string {
  const said = parseDelivered(item.text);
  return said && said.title === from ? said.body : item.text;
}

/** One queued message as `manage_queue` lists it: which one, what it says, and when it waits. */
function listedQueueItem(item: QueuedPrompt, caller: RuleThread): JsonObject {
  return {
    queue_id: item.id,
    preview: bodyOf(item, caller.title).slice(0, QUEUE_PREVIEW_CHARS),
    ...(item.queuedAt ? { queued_at: new Date(item.queuedAt).toISOString() } : {}),
  };
}

type ManageQueueCall = Extract<ReturnType<typeof parseCall>, { tool: "manage_queue" }>;

/**
 * `manage_queue`: what this thread queued on another one while it was busy, and the three things it
 * may still do to those messages. Only a message this thread queued (`from`) is its own; anything
 * else in that queue belongs to the user or to another thread and is reported as a count. A message
 * that has started left the queue and cannot be touched. `steer` delivers one now, by the same rule
 * ⌘⏎ follows (`sendDecisionFor`), so a harness that takes no message mid-turn is refused rather
 * than quietly queued again.
 */
async function manageQueue(caller: RuleThread, call: ManageQueueCall): Promise<JsonObject> {
  const target = ruleThread(useApp.getState(), call.target);
  // Only the caller's own queued messages are exposed below, in any project.
  // Slot, mailbox and chain rules were checked when the message queued.
  const refused = checkMessage(caller, target, book(), true, true);
  if (refused || !target) return failureJson(refused ?? failure("not_found", "no such thread"));
  const waiting = () => useApp.getState().threads[target.id]?.queue ?? [];
  const mine = (id: string | undefined) => waiting().find((q) => q.id === id);

  if (call.action === "list") {
    const thread = useApp.getState().threads[target.id];
    const own = waiting()
      .filter((q) => !q.wake && q.from === caller.id)
      .slice(0, MAX_QUEUE_ITEMS);
    const others = waiting().filter((q) => !q.wake && q.from !== caller.id).length;
    return {
      session_id: target.id,
      title: target.title,
      running: !!thread?.running,
      ...(thread?.queueHeld ? { held: true } : {}),
      queued: own.map((item) => listedQueueItem(item, caller)),
      // What waits there that this thread may not touch: the user's own messages, or another
      // thread's. Count only: their text is not this thread's to read.
      ...(others ? { others } : {}),
    };
  }

  const item = mine(call.queueId);
  if (!item) return failureJson(goneFromQueue(call.queueId));
  if (item.from !== caller.id) {
    return failureJson(failure("not_allowed", "that message was not queued by this thread"));
  }
  const verb =
    call.action === "edit"
      ? "Change a queued message to"
      : call.action === "cancel"
        ? "Drop a queued message from"
        : "Send a queued message now to";
  const no = await permitted(caller, target.id, {
    kind: "message",
    caller: caller.id,
    callerTitle: caller.title,
    target: target.title,
    ...(!sameReach(caller, target) ? { project: projectOf(target) } : {}),
    detail: `${verb}: ${call.action === "edit" ? (call.text ?? "").trim() : bodyOf(item, caller.title).slice(0, QUEUE_PREVIEW_CHARS)}`,
  });
  if (no) return failureJson(no);
  // The user took their time: the message may have started meanwhile.
  const again = mine(call.queueId);
  if (!again) return failureJson(goneFromQueue(call.queueId));
  if (again.from !== caller.id) {
    return failureJson(failure("not_allowed", "that message was not queued by this thread"));
  }

  switch (call.action) {
    case "edit": {
      const text = (call.text ?? "").trim();
      // Delivered the way `message` delivered it, so the target still reads who asked.
      updateQueued(
        target.id,
        again.id,
        delivered(text, caller.title, { session: caller.id, ...(again.link ? { link: again.link } : {}) }),
      );
      // The link card says what was sent, so it says the new text now.
      const link = again.link;
      if (link) setBook((b) => updateLink(b, link, { message: text }));
      return { session_id: target.id, queue_id: again.id, preview: text.slice(0, QUEUE_PREVIEW_CHARS) };
    }
    case "cancel": {
      // The same removal the row's own ✕ makes: its link never answers, and it cannot come back.
      removeQueued(target.id, again.id);
      return { session_id: target.id, queue_id: again.id, cancelled: true };
    }
    case "steer": {
      const thread = useApp.getState().threads[target.id];
      if (!thread?.running) {
        return failureJson(failure("conflict", "no turn is running in that thread to steer into"));
      }
      const verdict = sendDecisionFor(thread, again.extras);
      if (verdict.action === "interruptRestart") {
        // Stopping the turn to take it is not steering: this call only steers.
        return failureJson(failure("not_allowed", "this harness cannot steer"));
      }
      if (verdict.action === "queue") {
        return failureJson(
          failure(
            "not_allowed",
            verdict.reason === "harness"
              ? "this harness cannot steer"
              : `it cannot go into the turn now: ${QUEUE_REASON[verdict.reason].toLowerCase()}`,
          ),
        );
      }
      if (!(await sendQueuedNow(target.id, again.id))) {
        return failureJson(failure("conflict", "the turn would not take it now, so it is still queued"));
      }
      return { session_id: target.id, queue_id: again.id, steered: true };
    }
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
        `${harness.name} account “${refusal.name}” (${refusal.account}) is signed out; sign it in in Settings › Harnesses or pick another login (${refusal.available.join(", ")})`,
      );
    case "unknown":
      return failure(
        "invalid",
        refusal.available.length
          ? `${harness.name} has no account “${refusal.asked}”; its named accounts are ${refusal.available.join(", ")}`
          : `${harness.name} has no named accounts; it runs on its own sign-in, so leave account out`,
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

/** `mode`, lowered to what the call asked for (a delegate preset's access, issue 3391); never raised. */
const narrowed = (mode: PermissionMode, asked: PermissionMode | undefined): PermissionMode =>
  asked ? effectiveMode(mode, asked) : mode;

/** What a thread may do without asking, as the orchestration answers report it. */
const accessOf = (id: string): PermissionMode => permissionModeOf(useApp.getState(), id);

/**
 * What a standalone thread runs at: Settings › General › Default access, the
 * same as a thread the user starts by hand. It is the user's own thread, not a
 * delegate, so the caller's own limit is not what bounds it.
 */
const ownMode = (): PermissionMode => useApp.getState().defaultPermissionMode;

/**
 * The first message of a standalone thread: the prompt as the user's own, with
 * no "From thread" header and no delegate report to end with (issue 3443).
 */
function deliverOwn(target: string, text: string) {
  // The thread was created a moment ago, so it is not running: the prompt is
  // its first turn, as if the user had typed it.
  void runPrompt(target, text);
}

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

function start(caller: RuleThread, call: StartCall): Promise<JsonObject> {
  return withLinkGate(() => startNow(caller, call));
}

async function startNow(caller: RuleThread, call: StartCall): Promise<JsonObject> {
  // A standalone thread is not a delegate: it takes no delegation slot, so the
  // depth and slot rules (which bound how much work one thread hands out) do
  // not apply to it.
  const standalone = call.standalone === true;
  if (!standalone) {
    const refused = checkStart(caller, book(), 1, delegateLimits());
    if (refused) return failureJson(refused);
  }
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
  const access = narrowed(standalone ? ownMode() : childMode(caller), call.access);
  const no = await permitted(
    caller,
    other ? `start:${project}` : "start",
    {
      kind: "start",
      caller: caller.id,
      callerTitle: caller.title,
      target: title,
      detail: call.prompt,
      project: other ? project : undefined,
      harnesses: [choiceLabel(harness.name, accountLabelOf(harness, account), model, call.effort)],
      access,
      worktree: call.worktree ?? true,
      standalone: standalone || undefined,
    },
    undefined,
    harness.accounts?.length ? [delegateAccountKey("local", harness.id, account)] : [],
  );
  if (no) return failureJson(no);
  if (!standalone) {
    const again = checkStart(caller, book(), 1, delegateLimits());
    if (again) return failureJson(again);
  }
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
      parent: standalone ? undefined : caller.id,
      account,
    });
  } catch (error) {
    return failureJson(failure("failed", `the thread could not start: ${errorText(error)}`));
  }
  if (standalone) {
    deliverOwn(id, call.prompt);
    return {
      session_id: id,
      receipt: "started",
      title,
      harness: harness.id,
      access: accessOf(id),
      standalone: true,
      ...ranWith(id),
    };
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
    harnessName: harness.name,
    model,
    account: account ?? undefined,
  });
  deliver(id, delegated(call.prompt, caller.title), link, caller.id);
  return {
    session_id: id,
    link_id: link,
    receipt: "started",
    title,
    harness: harness.id,
    access: accessOf(id),
    ...ranWith(id),
    links_left: linksLeft(book(), caller.id, delegateLimits()),
  };
}

function fanOut(caller: RuleThread, call: FanOutCall): Promise<JsonObject> {
  return withLinkGate(() => fanOutNow(caller, call));
}

async function fanOutNow(caller: RuleThread, call: FanOutCall): Promise<JsonObject> {
  const standalone = call.standalone === true;
  if (!standalone) {
    const refused = checkStart(caller, book(), call.harnesses.length, delegateLimits());
    if (refused) return failureJson(refused);
  }
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
  const project = projectFor(caller, call.project);
  if (isFailure(project)) return failureJson(project);
  const title = call.title ?? titleFrom(call.prompt, 60);
  const access = narrowed(standalone ? ownMode() : childMode(caller), call.access);
  const other = !sameProject(project, projectOf(caller));
  const no = await permitted(
    caller,
    other ? `start:${project}` : "start",
    {
      kind: "fan_out",
      caller: caller.id,
      callerTitle: caller.title,
      target: title,
      detail: call.prompt,
      project: other ? project : undefined,
      harnesses: chosen.map((h) =>
        choiceLabel(h.harness.name, accountLabelOf(h.harness, h.account), h.model, h.effort),
      ),
      access,
      worktree: true,
      standalone: standalone || undefined,
    },
    undefined,
    chosen
      .filter((h) => h.harness.accounts?.length)
      .map((h) => delegateAccountKey("local", h.harness.id, h.account)),
  );
  if (no) return failureJson(no);
  if (!standalone) {
    const again = checkStart(caller, book(), chosen.length, delegateLimits());
    if (again) return failureJson(again);
  }
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
        parent: standalone ? undefined : caller.id,
        access,
      });
      if (standalone) {
        deliverOwn(id, call.prompt);
        out.push({ session_id: id, harness: h.harness.id, access: accessOf(id), ...ranWith(id) });
        continue;
      }
      const link = addLink({
        orchestrator: caller.id,
        target: id,
        orchestratorTitle: caller.title,
        targetTitle: threadTitle,
        message: call.prompt,
        state: "working",
        started: true,
        harness: h.harness.id,
        harnessName: h.harness.name,
        model: h.model,
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
      deliver(id, delegated(call.prompt, caller.title), link, caller.id);
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
  if (standalone) return { links: out, standalone: true };
  return { links: out, fan_out_id: fanId, links_left: linksLeft(book(), caller.id, delegateLimits()) };
}

/**
 * A link's state now: what its target thread does, folded onto the record
 * ({@link liveState}). A running turn that waits on an approval or a question
 * needs the user, and a link a restart could not resume reads live again while
 * its thread keeps working.
 */
export function stateOf(s: AppState, link: Link) {
  return liveState(link, activityOfThread(s, link.target));
}

/** What a thread the app may not have open does now: its live turn, else what the list reports. */
function activityOfThread(s: AppState, id: string): ThreadStatus | undefined {
  const open = s.threads[id];
  if (open?.permissions.length) return "needs_input";
  if (open?.running) return "running";
  // The list's index, not a scan: `stateOf` runs in selectors on each store change (state/store.ts).
  return sessionById(s, id)?.status;
}

/**
 * One thread's activity from the data store of the computer it runs on: this
 * computer's, or the paired machine's own, over that machine's connection. No
 * answer (an older engine, a channel that went away) is said by its callers,
 * never passed on as an empty list that reads as a thread that did nothing
 * (issue 3525).
 */
async function activityFrom(
  thread: string,
  params: JsonObject,
  timeoutMs = 20_000,
): Promise<{ answer?: JsonObject; error?: string }> {
  const conn = connectionFor(splitThreadKey(thread).host);
  if (!conn) return { error: "it is not connected" };
  try {
    const answer = await conn.request<JsonObject>(
      Method.dataActivity,
      { sessionId: splitThreadKey(thread).id, ...params },
      timeoutMs,
    );
    return { answer };
  } catch (error) {
    return { error: errorText(error) };
  }
}

/** Why a link to a thread the app can no longer find failed: its row and its tip say the same. */
const REASON_THREAD_GONE = "That thread is gone.";

/**
 * Whether a link's thread has gone: no list names it and the app keeps no row for it. A thread
 * this window made there, or another client announced, is listed only once a `session/list`
 * catches up, which on a first turn is after the message that started it: the keep-unlisted set
 * (`state/store.ts` `isKeptUnlisted`) says which unlisted rows are still due. A thread with a
 * `threads` copy is no exception — every link target made in this window has one — so the copy
 * is not what tells an unlisted thread from a gone one (issue 3523).
 */
const vanished = (s: AppState, target: string) =>
  !s.sessions.some((x) => x.id === target) && !isKeptUnlisted(target);

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
  all = false,
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
    const activity = activityOf(s.threads[link.target]?.items ?? [], s.threads[link.target]?.children);
    // A settled link with no answer still has one: the target's last reply, read now.
    const reply = link.answer?.trim()
      ? link
      : filledWith(link, latestReply(s.threads[link.target]?.items ?? []));
    return linkResult(reply, shown, statusOf(s, link.target), { now, activity });
  };
  // The link's answer is out with this reply: no wake may repeat it. A link whose state goes out
  // with a timeout is handed over the same way, so a child that settles just as the wait gives up
  // does not come back a second time as a wake.
  const handOne = (link: Link, shown = shownOf(link)) => {
    if (shown === "done" || shown === "failed" || shown === "needs_user") {
      me.handed.push(markHeard(link, shown, Date.now()));
    }
    return result(link, shown);
  };
  const hand = (link: Link, shown = shownOf(link)) => ({ links: [handOne(link, shown)] });
  /** A wave's answer: every ready link in one reply, each answer cut to {@link ALL_ANSWER_CHARS}. */
  const handAll = (ready: Link[]) => ({ links: ready.map((link) => cutForAll(handOne(link))) });
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
      const ready: Link[] = [];
      let pending = false;
      let asking = false;
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
        if (isActive(link.state) && vanished(s, link.target)) {
          setBook((b) =>
            updateLink(b, id, {
              state: "failed",
              answer: REASON_THREAD_GONE,
              error: REASON_THREAD_GONE,
            }),
          );
          // Failed now: the next look hands it over with the rest.
          pending = true;
          continue;
        }
        const now = stateOf(s, link);
        if (now === "done" || now === "failed") {
          // Settled, but it works on: the answer is the one after its subtree is quiet.
          if (subtreeQuiet(link.target)) {
            if (!all) return hand(link);
            ready.push(link);
          } else pending = true;
          continue;
        }
        if (!isActive(now) || now === "needs_user") {
          if (!all) return hand(link);
          ready.push(link);
          asking ||= now === "needs_user";
        } else pending = true;
      }
      // With `all`, one answer for the whole wave: once every link is ready, or sooner when one
      // waits for the user, who may be needed before the rest can finish.
      if (all && ready.length && (asking || !pending)) return handAll(ready);
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
  const pending = droppable.get(callId);
  if (pending && (session === undefined || pending.caller === session)) {
    pending.dropped = true;
    for (const stop of [...pending.onDrop]) stop();
    return;
  }
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

// No storage (a private window): the queue is this window's only, as before. A web view whose
// storage is full keeps the queue in memory instead.
function storedQueues(): string | null {
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
  const activity: Record<string, LinkActivity> = {};
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
    if (!activity[link.target]) activity[link.target] = activityOf(target?.items ?? [], target?.children);
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
  if (!current?.running) {
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
  if (!state.sessionsLoaded) return;
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
    if (thread?.loading || thread?.queue.some((q) => q.link === link.id)) return false;
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

/** Forget a finished thread's messages: the strip draws one row however many it got. */
export function dismissLinkThread(orchestrator: string, target: string) {
  setBook((b) => ({
    ...b,
    links: b.links.filter(
      (link) => link.orchestrator !== orchestrator || link.target !== target || isActive(link.state),
    ),
  }));
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
