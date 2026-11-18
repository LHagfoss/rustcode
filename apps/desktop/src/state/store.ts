import { parseAppFont, parseTextSize } from "./settingsActions";

export * from "./settingsActions";

import { advertisedCommands, commandsFor, harnessOf, parsePanels } from "./viewActions";

export * from "./viewActions";

import {
  type AppState,
  type CachedPlan,
  loadTotalLimit,
  SETTINGS_DEFAULTS,
  STORAGE,
  store,
  stored,
  storedFlag,
  storedJson,
  storedThreadLimit,
} from "./appState";

export * from "./appState";

import { isLoaded } from "./thread";
// App state (Zustand) and the engine controller that drives it over ACP.
// Components read slices with `useApp(selector)` and call the actions below.

import { create } from "zustand";
import { onThemesChanged, refreshThemes } from "./themeActions";

export { createTheme, setActiveTheme } from "./themeActions";

import { refreshAgents, startAgentPolling } from "./agentActions";
import { permissionModeOf, refreshGrants } from "./approvals";
import {
  forgetAskedModels,
  lastAccountFor,
  noteChoice,
  refreshHarnesses,
  rememberModelEffort,
  switchThreadHarness,
} from "./harnessActions";

export { agentHistory, hasAgents, refreshAgents, startAgentPolling } from "./agentActions";

import { refreshCheckpoints } from "./checkpointActions";

export { refreshCheckpoints, returnToAttempt, rewindCheckpoint, turnDiff } from "./checkpointActions";

import { pruneSidebarOrder, threadDeleted } from "./threadActions";

export {
  askConfirm,
  cancelEdit,
  canRewind,
  closeConfirm,
  dismissInbox,
  editMessage,
  forgetSidebarOrder,
  pruneSidebarOrder,
  renameThread,
  restoreInbox,
  retryMessage,
  retryThread,
  setArchived,
  setPinned,
  setSectionOrder,
  threadDeleted,
  togglePin,
  trashThread,
} from "./threadActions";

import {
  clearSettledNotifications,
  clearStatusMark,
  reconcileStatusMarks,
  sessionById,
  setThreadStatus,
  statusOf,
} from "./threadStatus";

export {
  accountSlug,
  accountsOf,
  addCustomHarness,
  addHarnessAccount,
  appChoices,
  checkHarnessAccounts,
  completeOnboarding,
  discardCustomHarness,
  effortMemory,
  enableCustomHarness,
  ensureHarnessModels,
  fetchHarnessModels,
  forgetAskedModels,
  type LocalChoice,
  lastAccountFor,
  localChoices,
  noteChoice,
  pickNewThreadAccount,
  pickNewThreadAccountOn,
  pickNewThreadChoice,
  refreshHarnesses,
  rememberModelEffort,
  removeHarness,
  removeHarnessAccount,
  setDefaultHarness,
  setHarnessAdvanced,
  setHarnessEnabled,
  setNewThreadChoice,
  switchNewThreadHarness,
  switchThreadAccount,
  switchThreadHarness,
} from "./harnessActions";
export {
  clearStatusMark,
  leaveSettledThreads,
  sessionById,
  setThreadStatus,
  setThreadsStatus,
  settleThread,
  settleThreads,
  statusOf,
  unsettleThreads,
} from "./threadStatus";

import {
  prunePlans,
  refreshPlanCache,
  refreshPlanUsage,
  startPlanPolling,
  stopPlanPolling,
} from "./planUsage";

export {
  answerPermission,
  answerQuestion,
  answerQuestionOptions,
  answerQuestionText,
  answerSecret,
  approveAll,
  clearPermissionModes,
  defaultModeFor,
  hasOwnPermissionMode,
  inheritedModeOf,
  permissionModeOf,
  RESOLVED_APPROVAL_MS,
  type ResolvedApproval,
  refreshGrants,
  revokeGrant,
  secretTargets,
  setDefaultPermissionMode,
  setPermissionMode,
  setProjectPermissionMode,
} from "./approvals";

export {
  backgroundPlansAsk,
  commitPlansAnswer,
  forgetPlans,
  mergePlans,
  onlyCachedPlan,
  ownPlanUsageDue,
  PLAN_CACHE_POLL_MS,
  PLAN_STALE_MARGIN_MS,
  planCacheStale,
  planIsStale,
  planNow,
  plansToCache,
  pollPlans,
  prunePlans,
  refreshPlanCache,
  refreshPlanUsage,
  refreshReadPlanLogins,
  shownPlan,
  startPlanPolling,
  windowNow,
} from "./planUsage";

import {
  AcpConnection,
  ConnectionClosedError,
  type Inbound,
  type Json,
  type JsonObject,
  obj,
  RpcError,
  str,
} from "../acp/jsonrpc";
import type { Transport } from "../acp/transport";
import {
  byActivity,
  type Harness,
  type HarnessSnapshot,
  initializeParams,
  isReportUpdate,
  Method,
  parseAgentInfo,
  parseConfigOptions,
  parsePermissionRequest,
  parseSessionList,
  parseSessionUpdate,
  permissionCancelled,
  permissionSelected,
  type SessionSummary,
} from "../acp/types";
import { STATUS_LABEL, statusInUpdate, type ThreadStatus, V2Method } from "../acp/v2";
import { WorkflowMethod } from "../acp/workflows";
import { announce, mountAnnouncer } from "../lib/announce";
import { parseChatWidth } from "../lib/chatWidth";
import { capServerLogLine } from "../lib/diagnostics";
import { afterPaint } from "../lib/frame";
import { parseOverrides } from "../lib/keys";
import { setShellTransport } from "../lib/shell";
import { ThemeMethod } from "../lib/themes";
import { titleFrom } from "../lib/title";
import {
  applyWindowTitle,
  momentOf,
  spokenTitle,
  type ThreadMoment,
  threadAnnouncement,
  windowStatus,
  windowTitle,
} from "./announceThreads";
import { uploadPromptImages } from "./attachmentTransfer";
import { automaticCompactionText, DEFAULT_AUTO_COMPACT, parseAutoCompactSettings } from "./autoCompact";
import {
  beginAutoCompactIdle,
  forgetAutoCompact,
  noteCompactionCard,
  noteTyping,
  scheduleAutoCompact,
} from "./autoCompactRun";
import { createBatcher, frameQueue } from "./batch";
import { parseHarnessCommands, rememberCommands, serializeHarnessCommands } from "./commands";
import {
  type ComposeExtras,
  carriesExtras,
  composedText,
  emptyExtras,
  hasExtras,
  promptBlocks,
  sentAttachments,
} from "./compose";
import {
  clearStopped,
  continueDueThreads,
  continueInternetThreads,
  dropLimitWait,
  dropWaits,
  interruptedThreads,
  noteStopped,
  resetContinueWaits,
  resumeInterrupted,
  settleStopWait,
  syncWaitTimers,
  watchRunningThreads,
} from "./continueThreads";
import { contextPercent } from "./details";
import { DRAFTS_KEY, parseDrafts, persistable, withDraft } from "./drafts";
import {
  type EngineEvent,
  type EngineState,
  initialEngine,
  sessionsListed,
  startingText,
  transitionEngine,
} from "./engine";
import { selectOption, unavailableReason } from "./harness";
import { sanitizeHeaderButtons } from "./headerButtons";
import {
  answerOrchestration,
  linkFinished,
  linkStarted,
  linksFinished,
  linksReopened,
  linksResumed,
  linkWaitEnded,
  linkWaitStarted,
  loadBook,
  orchestrationCallDropped,
  reconcileLinks,
  requeueUnreadSteers,
  resetLinkBook,
  resetWaits,
  restoreQueuedThreads,
  stopLinksFrom,
  syncLinkBook,
  takeSteeredLinks,
  threadTurnEnded,
  wakeDelivered,
  wakeTick,
} from "./host";
import type { InboxDismissals } from "./inboxDismissed";
import type { InternetWait } from "./internetWait";
import { type FallbackRule, movedNotice, pickFallback, switchText } from "./limitFallback";
import type { LimitWait } from "./limitResume";
import { newThreadAccountOn } from "./logins";
import { lostTurn, lostTurnStatus, reloadThread, resetLostTurns, resumeLostTurns } from "./lostTurns";
import { saveTranscript } from "./machineCache";
import { mirrorTranscript, setMirrorRequest } from "./machineMirror";
import { useOutbox } from "./machineOutbox";
import { flushingOutbox, mergeMachineLists } from "./machineSync";
import { notifyThread, startNotifications, warmingUp } from "./notify";
import {
  CAPABILITY_VERSION,
  failure,
  failureJson,
  MACHINE_LOST_TURN,
  parseDelivered,
  WAKE_BATCH_DEFAULT_S,
} from "./orchestration";
import { clearPageCache } from "./pageCache";
import { autoAnswer, type PermissionMode, parsePermissionMode } from "./permissions";
import { projectKey } from "./projectBook";
import { createdPull, deliverPullWatch, PULL_WATCH, subscribePullWatches } from "./pullWatch";
import { nextQueued } from "./queue";
import { parseRecapHours } from "./recap";
import { resetRemoteStatusReads } from "./remoteAccess";
import { engineBusy, FORCE_STOP_MS } from "./runState";
import { forgetRunDrift, reconcileRunState, resetRunStatePoll, startRunStatePoll } from "./runStatePoll";
import { parseScheduleBook, SCHEDULED_KEY } from "./schedule";
import { resetScheduledSends, restoreScheduled } from "./scheduledSends";
import { acceptsImages, removeQueued, threadHarness } from "./sendActions";
import type { SidebarOrder } from "./sidebarOrder";
import {
  appendNotice,
  applyUpdate,
  beginQuietTurn,
  beginTurn,
  endTurn,
  finishTurn,
  foldingBatch,
  isContinueMessage,
  isRunning,
  isWakeMessage,
  isWoken,
  localId,
  requeueSteers,
  settleEveryWait,
  startCompaction,
  type ThreadState,
  waitingOn,
  wakeTurn,
  withChildActivity,
  withContinueMark,
} from "./thread";
import { isRemoteThread, LOCAL_HOST, splitThreadKey, threadKey } from "./threadKeys";
import {
  deferAnnouncement,
  isTombstoned,
  keepUnlisted,
  keptUnlisted,
  resetThreadListGuards,
  settleKeptUnlisted,
} from "./threadListGuards";
import {
  announceFinished,
  applySnooze,
  finishToast,
  isDelegated,
  markFinished,
  markMessaged,
  notifyTitle,
  threadName,
  wakeOnWrite,
} from "./threadNotices";
import {
  autoRepaired,
  closeThread,
  loading,
  loadMachineHarnesses,
  opening,
  openThread,
  receiveActivity,
  resetFocusList,
  selectThread,
} from "./threadOpen";
import {
  clear as clearQueue,
  dismiss as dismissInQueue,
  due as dueInQueue,
  enqueue,
  pause as pauseToastQueue,
  remove as removeFromQueue,
  resume as resumeToastQueue,
  TOAST_EXIT_MS,
  type Toast,
  type ToastTone,
} from "./toasts";
import { noticeOnlyHistory, userTypedPrompt } from "./topLevel";
import { readableTurnError } from "./turnError";
import { onWorkflowChanged, subscribeWorkflows } from "./workflows";

export {
  cancelWait,
  continueDueThreads,
  continueInternetThreads,
  dropInternetWait,
  dropLimitWait,
  limitTime,
  RESUME_TEXT,
  releaseLimitWaitLink,
  resumeWaitNow,
  setContinueAfterLimit,
} from "./continueThreads";
export { lostTurn, setLostMachineMsForTests } from "./lostTurns";
export { flushMachineOutbox, remoteStatusPending, syncNote } from "./machineSync";
export {
  continueDueScheduled,
  removeScheduled,
  restoreScheduled,
  scheduleMessage,
  sendScheduledNow,
} from "./scheduledSends";
export {
  acceptsImages,
  cancelQueuedEdit,
  endQueuedEdit,
  forkThread,
  moveQueued,
  openSideChat,
  quoteMessage,
  removeQueued,
  resumeQueue,
  runCommandAction,
  saveQueuedEdit,
  sendDecisionFor,
  sendQueuedNow,
  sendSideChatToParent,
  startPullThread,
  startQueuedEdit,
  steer,
  steeringOf,
  steerOrSubmit,
  submit,
  threadHarness,
  turnState,
  updateQueued,
} from "./sendActions";
export {
  beginCreatingSession,
  deferAnnouncement,
  endCreatingSession,
  isKeptUnlisted,
  isTombstoned,
  keepUnlisted,
  keptUnlisted,
  keptUnlistedThreads,
  releaseAnnouncements,
  settleKeptUnlisted,
  tombstoneThread,
} from "./threadListGuards";
export {
  announceFinished,
  applySnooze,
  askCustomSnooze,
  clearFinished,
  finishToast,
  markFinished,
  markUnread,
  seeFailure,
  setSnoozed,
  snoozeThread,
  snoozeWakeEvent,
  wakeOnWrite,
  wakeSnoozed,
  without,
} from "./threadNotices";
export {
  closeThread,
  createSession,
  createThread,
  dismissPendingSetup,
  HOME_FOLDER,
  isRepairableOpenError,
  lacksModels,
  loading,
  loadMachineHarnesses,
  moveThread,
  newThreadChoice,
  opening,
  openThread,
  projectThreadChoice,
  refreshSessionsOnFocus,
  repairThread,
  retryPendingSetup,
  selectThread,
  setDefaultCwd,
  setNewThreadHost,
  setProjectCwd,
  startNewThread,
} from "./threadOpen";

export type { Toast, ToastTone } from "./toasts";

export const APP_VERSION = "0.1.0";

/** The `initialize` sent a paired machine: its threads drawn with this app's Smart UI and viz, their
 * tool calls unanswered (it refuses a machine's delegations), so that engine serves them itself. */
export const machineInitializeParams = (): JsonObject =>
  initializeParams(APP_VERSION, {
    smartUi: useApp.getState().smartUi,
    visualizations: 2,
    threads: 1,
    engineOrchestration: useApp.getState().orchestration ? 1 : undefined,
  });

// Engine lines are applied a frame at a time; see batch.ts.
const batcher = createBatcher<AppState>();

export const useApp = create<AppState>((_set, _get, api) => ({
  // Lets `batcher.batch` defer subscribers (adds no state).
  ...batcher.attach(api),
  pinned: storedJson<string[]>(STORAGE.pinned, []),
  sidebarOrder: storedJson<SidebarOrder>(STORAGE.sidebarOrder, {}),
  archived: storedJson<Record<string, number>>(STORAGE.archived, {}),
  messaged: storedJson<Record<string, number>>(STORAGE.messaged, {}),
  inboxDismissed: storedJson<InboxDismissals>(STORAGE.inboxDismissed, {}),
  editing: {},
  queuedEdit: null,
  confirm: null,
  // On unless turned off: an agent asks before it starts or messages a thread (Ask), so
  // delegating is there to use without first finding the setting.
  orchestration: storedFlag(STORAGE.orchestration, SETTINGS_DEFAULTS.orchestration),
  threadLimit: storedThreadLimit(STORAGE.threadLimit, SETTINGS_DEFAULTS.threadLimit),
  totalLimit: loadTotalLimit(),
  trustDelegateAccounts: storedFlag(STORAGE.trustDelegateAccounts, false),
  // On unless turned off: reading knows no project boundary, so an agent asked
  // about the user's work is not answered with a fragment of it.
  readAcrossProjects: storedFlag(STORAGE.readAcrossProjects, SETTINGS_DEFAULTS.readAcrossProjects),
  // Kept on the server only (`limit-fallback`, `limit-fallback-map`); hydrated at start.
  limitFallback: false,
  limitFallbackMap: [],
  wakeBatchSeconds: WAKE_BATCH_DEFAULT_S,
  resumeAfterRestart: storedFlag(STORAGE.resumeAfterRestart, SETTINGS_DEFAULTS.resumeAfterRestart),
  systemNotifications: storedFlag(STORAGE.systemNotifications, SETTINGS_DEFAULTS.systemNotifications),
  notificationsBackgroundOnly: storedFlag(
    STORAGE.notificationsBackgroundOnly,
    SETTINGS_DEFAULTS.notificationsBackgroundOnly,
  ),
  recapWhileAway: storedFlag(STORAGE.recapWhileAway, SETTINGS_DEFAULTS.recapWhileAway),
  recapHours: parseRecapHours(stored(STORAGE.recapHours)),
  recapSince: null,
  recapDismissed: false,
  continueAfterLimit: storedFlag(STORAGE.continueAfterLimit, SETTINGS_DEFAULTS.continueAfterLimit),
  limitWaits: storedJson<Record<string, LimitWait>>(STORAGE.limitWaits, {}),
  internetWaits: storedJson<Record<string, InternetWait>>(STORAGE.internetWaits, {}),
  font: parseAppFont(stored(STORAGE.font), stored(STORAGE.roundedFont)),
  textSize: parseTextSize(stored(STORAGE.textSize)),
  animateStreaming: storedFlag(STORAGE.animateStreaming, SETTINGS_DEFAULTS.animateStreaming),
  reduceTransparency: storedFlag(STORAGE.reduceTransparency, SETTINGS_DEFAULTS.reduceTransparency),
  chatWidth: parseChatWidth(stored(STORAGE.chatWidth)),
  showToolCalls: storedFlag(STORAGE.showToolCalls, SETTINGS_DEFAULTS.showToolCalls),
  headerButtons: sanitizeHeaderButtons(storedJson<unknown>(STORAGE.headerButtons, null)),
  shortcuts: parseOverrides(stored(STORAGE.shortcuts)),
  orchestrationPending: false,
  links: loadBook(),
  grant: null,
  features: {},
  statusMarks: storedJson<Record<string, ThreadStatus>>(STORAGE.statusMarks, {}),
  engineRunning: {},
  autoCompact: parseAutoCompactSettings(stored(STORAGE.autoCompact)),
  autoCompactOff: storedJson<Record<string, boolean>>(STORAGE.autoCompactOff, {}),
  snoozed: storedJson<Record<string, number>>(STORAGE.snoozed, {}),
  snoozeAsk: null,
  defaultPermissionMode: parsePermissionMode(stored(STORAGE.permissionMode)),
  projectPermissionModes: loadProjectPermissionModes(),
  permissionModes: loadPermissionPins(),
  newThreadPermissionMode: null,
  finished: {},
  failed: {},
  grants: [],
  resolvedApprovals: {},
  remote: null,
  pairing: { phase: "idle" },
  remoteByThread: {},
  remoteError: null,
  connectCode: null,
  view: { kind: "chat" },
  checkpoints: {},
  attempts: {},
  newWorktree: { on: false, branch: "", edited: false },
  pendingSetup: null,
  backgroundSetups: {},
  smartUi: storedFlag(STORAGE.smartUi, SETTINGS_DEFAULTS.smartUi),
  smartUiPending: false,
  terminalOpen: false,
  panel: null,
  peekId: null,
  panelByThread: parsePanels(storedJson<unknown>(STORAGE.panelByThread, {})),
  agents: {},
  planUsage: {},
  planCache: prunePlans(storedJson<Record<string, CachedPlan>>(STORAGE.planCache, {})),
  signedOutLogins: {},
  engine: initialEngine,
  engineLog: [],
  sessions: [],
  machineNames: {},
  connectedHosts: [],
  sessionsCapped: false,
  threads: {},
  selectedId: null,
  projectCwd: stored(STORAGE.project) ?? "",
  defaultCwd: stored(STORAGE.project) ?? "",
  newThread: null,
  newThreadAccount: null,
  newThreadHost: LOCAL_HOST,
  remoteHarnesses: {},
  harnesses: null,
  baseCommands: [],
  harnessCommands: parseHarnessCommands(stored(STORAGE.harnessCommands)),
  sessionCommands: {},
  drafts: parseDrafts(stored(DRAFTS_KEY)),
  extras: {},
  scheduled: parseScheduleBook(stored(SCHEDULED_KEY)),
  settings: null,
  settingsMachine: null,
  switcherOpen: false,
  modelPickerAsk: 0,
  report: null,
  onboardingOpen: false,
  sidebarHidden: stored(STORAGE.sidebar) === "1",
  toasts: [],
  toastsPaused: false,
  theme: (stored(STORAGE.theme) as AppState["theme"] | null) ?? SETTINGS_DEFAULTS.theme,
  themes: null,
  themeActive: stored(STORAGE.themeActive) ?? "rustcode",
}));

const set = useApp.setState;
const get = useApp.getState;

export function updateThread(id: string, fn: (thread: ThreadState) => ThreadState) {
  set((state) => {
    const thread = state.threads[id];
    if (!thread) return state;
    const next = fn(thread);
    if (next === thread) return state;
    const model =
      next.configOptions !== thread.configOptions && next.configOptions
        ? selectOption(next.configOptions, "model")?.current
        : undefined;
    return {
      threads: { ...state.threads, [id]: next },
      ...(model && next.configOptions !== thread.configOptions
        ? { sessions: state.sessions.map((x) => (x.id === id ? { ...x, model } : x)) }
        : {}),
    };
  });
}

/**
 * Shows a toast (ui/Toaster.tsx, state/toasts.ts). Info and success leave after
 * 3 s, errors after 7 s; the pointer or focus on the stack, a drag, or the
 * window in the background stops that clock. Passing an `id` replaces the toast
 * with that id in place (the engine's toast moves from "Starting…" to an error
 * this way); the same text and tone again restarts the shown one instead of
 * stacking a copy. At most three show, newest on top. Returns the toast's id.
 */
export function toast(
  text: string,
  tone: ToastTone = "info",
  options: { id?: string; action?: Toast["action"]; persist?: boolean; progress?: number } = {},
): string {
  const now = Date.now();
  let id = options.id ?? localId("toast");
  set((s) => {
    const live = s.toasts.filter((t) => !t.leaving);
    const existing = live.find((t) => t.id === id) ?? live.find((t) => t.text === text && t.tone === tone);
    if (existing) id = existing.id;
    const toasts = enqueue(
      s.toasts,
      {
        id,
        text,
        tone,
        action: options.action,
        persist: options.persist ?? false,
        progress: options.progress,
      },
      { now, paused: s.toastsPaused },
    );
    return { toasts };
  });
  return id;
}

/** Changes a shown toast in place (its progress), without restarting its timer. */
export function updateToast(id: string, patch: Partial<Pick<Toast, "text" | "progress">>) {
  set((s) =>
    s.toasts.some((t) => t.id === id)
      ? { toasts: s.toasts.map((t) => (t.id === id ? { ...t, ...patch } : t)) }
      : s,
  );
}

/** Drops every toast at once, except the ones that stay until dismissed. */
export function clearToasts() {
  const toasts = clearQueue(get().toasts);
  if (toasts !== get().toasts) set({ toasts });
}

/**
 * Stops the toasts' clocks while the pointer or focus is on the stack, the
 * window is in the background, or one is being dragged, and starts them again
 * with what was left. Idempotent: every pause reason can call it.
 */
export function setToastsPaused(paused: boolean) {
  const now = Date.now();
  set((s) => {
    if (s.toastsPaused === paused) return s;
    return {
      toastsPaused: paused,
      toasts: paused ? pauseToastQueue(s.toasts, now) : resumeToastQueue(s.toasts, now),
    };
  });
}

/** Fades out every toast whose time is up; the Toaster runs this on one timer. */
export function expireToasts() {
  const now = Date.now();
  for (const id of dueInQueue(get().toasts, now)) dismissToast(id);
}

/** Fades a toast out, then drops it. */
export function dismissToast(id: string) {
  const toasts = dismissInQueue(get().toasts, id);
  if (toasts === get().toasts) return;
  set({ toasts });
  setTimeout(() => {
    const next = removeFromQueue(get().toasts, id);
    if (next !== get().toasts) set({ toasts: next });
  }, TOAST_EXIT_MS);
}

/**
 * What an error says in words. The engine answers a failed method with a
 * generic name in `message` ("Invalid params") and the reason in `data` — a
 * folder that is not a git repository, a session already active — so the
 * reason wins when there is one.
 */
export function errorText(error: unknown): string {
  // A paired computer's gateway refuses a method it does not relay: one on an older RustCode
  // than this app, which says so instead of "not permitted over remote ACP".
  if (error instanceof RpcError && str(obj(error.data)?.code) === "unauthorized_access") {
    return "The computer this thread runs on does not take that from another computer yet; update RustCode there.";
  }
  if (error instanceof RpcError) {
    const detail = rpcReason(error.data);
    if (detail) return detail;
  }
  return error instanceof Error ? error.message : String(error);
}

/** The reason inside an error's `data`: its message field, or the data when it is text. */
function rpcReason(data: Json | undefined): string | null {
  const text = str(data) ?? str(obj(data)?.message) ?? str(obj(obj(data)?.error)?.message);
  const trimmed = text?.trim();
  return trimmed ? trimmed : null;
}

// ---------------------------------------------------------------- connection

export let connection: AcpConnection | null = null;
let transport: Transport | null = null;

// ---------------------------------------------------------------- machines
//
// A thread on a paired computer is driven over that computer's own connection
// (state/machines.ts holds one per machine, the shell's supervisor keeps it).
// Local threads keep their bare id and this module's `connection`; a remote
// thread's key carries its machine (`state/threadKeys.ts`), and every call
// that names it on the machine goes through `connectionForThread` with the
// key split back into the machine's own session id. Responses to a machine's
// own requests go back on the same connection they arrived on.

const machineConnections = new Map<string, AcpConnection>();
const machineOff = new Map<string, () => void>();
let machineRouting: Promise<void> | null = null;
/** The machine store's own subscription, kept so a test reset can let it go. */
let machineMirrorOff: (() => void) | null = null;
let transcriptKeepOff: (() => void) | null = null;
/** Threads whose transcript is waiting to be kept on this computer, by key. */
const transcriptTimers = new Map<string, ReturnType<typeof setTimeout>>();
/** A remote thread's transcript is kept at most this often while it changes. */
const KEEP_TRANSCRIPT_MS = 2_000;

/**
 * Keep the transcript of a thread opened on a machine (`state/machineCache.ts`), so it can be read
 * while the machine is away. Only a copy the machine just gave is kept: one drawn from this
 * computer's own copy is already what is kept.
 */
function keepRemoteTranscript(key: string): void {
  transcriptTimers.delete(key);
  const thread = get().threads[key];
  if (!isLoaded(thread) || !machineConnections.has(splitThreadKey(key).host)) return;
  saveTranscript(thread, key);
  // The same read also goes to this computer's disk, where nothing ages out (docs/data-store.md ›
  // Thread mirror).
  mirrorTranscript(thread, key);
}

function watchRemoteTranscripts(next: AppState, prev: AppState): void {
  if (next.threads === prev.threads) return;
  for (const [key, thread] of Object.entries(next.threads)) {
    if (thread === prev.threads[key] || !isRemoteThread(key) || transcriptTimers.has(key)) continue;
    transcriptTimers.set(
      key,
      setTimeout(() => keepRemoteTranscript(key), KEEP_TRANSCRIPT_MS),
    );
  }
}
/** `state/machines.ts` `resetMachinesForTests`, once its module is loaded. */
let machineReset: (() => void) | null = null;

/** The connection a thread's operations go over: this computer's engine, or that machine's own. */
export function connectionFor(host: string): AcpConnection | null {
  return host ? (machineConnections.get(host) ?? null) : connection;
}

// The thread mirror writes through this computer's own engine (`state/machineMirror.ts`).
setMirrorRequest(async (params) => {
  const engine = connectionFor("");
  if (!engine) throw new Error("This computer's engine is not connected.");
  return engine.request(Method.mirror, params as JsonObject, 15_000);
});

/** The same, from a thread key (`state/threadKeys.ts`). */
export function connectionForThread(key: string): AcpConnection | null {
  return connectionFor(splitThreadKey(key).host);
}

/** Whether the thread runs on another computer. */
export function threadIsRemote(key: string): boolean {
  return isRemoteThread(key);
}

/** The name of the machine a remote thread runs on; undefined for this computer. */
export function machineOfThread(key: string): string | undefined {
  return get().machineNames[splitThreadKey(key).host];
}

/** Whether a machine's threads are listed but its connection is not open (read-only). */
export function threadOffline(key: string | null | undefined): boolean {
  if (!key) return false;
  const host = splitThreadKey(key).host;
  return !!host && !machineConnections.has(host);
}

/**
 * The harnesses, models and accounts one computer lists: this one's own, or a paired
 * machine's read over its connection (`loadMachineHarnesses`). Nothing for a machine that has
 * not answered yet, which the pickers draw as loading rather than as an empty engine.
 */
export function harnessesOn(
  s: { harnesses: HarnessSnapshot | null; remoteHarnesses?: Record<string, HarnessSnapshot> },
  host: string | null | undefined,
): HarnessSnapshot | null {
  return host ? (s.remoteHarnesses?.[host] ?? null) : s.harnesses;
}

/** The machine a thread runs on, as the chat's header reads it, and whether it is away now. */
export function useMachineOfThread(key: string | null | undefined): string | undefined {
  const names = useApp((s) => s.machineNames);
  return key ? names[splitThreadKey(key).host] : undefined;
}

/** A remote thread's machine is away: the row stays, the thread opens read-only (ChatView, Composer). */
export function useThreadOffline(key: string | null | undefined): boolean {
  const hosts = useApp((s) => s.connectedHosts);
  if (!key) return false;
  const host = splitThreadKey(key).host;
  return !!host && !hosts.includes(host);
}

/**
 * Follow the shell's machines: one inbound subscription per connection (so a
 * machine's notifications reach the transcript tagged with its key), and its
 * thread list mirrored into `sessions` whenever it is re-read.
 */
export async function startMachineRouting(): Promise<void> {
  if (machineRouting) return machineRouting;
  machineRouting = (async () => {
    // Loaded here, not at the top: state/machines.ts reads this module's `toast` and `askConfirm`.
    const machines = await import("./machines");
    machineReset = machines.resetMachinesForTests;
    machines.onMachineLinks((id, conn) => watchMachineConnection(id, conn));
    const mirror = () => {
      const state = machines.useMachines.getState();
      const names = Object.fromEntries(
        state.machines.map((machine) => [machine.id, machines.machineName(machine)]),
      );
      set((s) => {
        // A machine that was removed takes its kept harnesses with it, and **Run on** naming it
        // goes back here rather than to a machine that is no longer saved.
        const snapshots = Object.fromEntries(
          Object.entries(s.remoteHarnesses).filter(([host]) => !!names[host]),
        );
        const named = s.newThreadHost && !names[s.newThreadHost];
        return {
          machineNames: names,
          remoteHarnesses: snapshots,
          ...(named ? { newThreadHost: LOCAL_HOST, projectCwd: "" } : {}),
        };
      });
      mergeMachineLists(state.sessions);
    };
    machineMirrorOff = machines.useMachines.subscribe((state, prev) => {
      if (state.sessions !== prev.sessions || state.machines !== prev.machines) mirror();
    });
    transcriptKeepOff = useApp.subscribe(watchRemoteTranscripts);
    for (const link of machines.machineLinks()) watchMachineConnection(link.machine.id, link.connection);
    mirror();
  })();
  return machineRouting;
}

/**
 * Tests only: forget every machine's channel, the threads it listed and the names it had, so one
 * test file's machines cannot be another's (machinesThreads.test.ts, `resetForTests`).
 */
export function resetMachinesForTests(): void {
  for (const off of machineOff.values()) off();
  machineOff.clear();
  machineConnections.clear();
  machineRouting = null;
  machineMirrorOff?.();
  machineMirrorOff = null;
  transcriptKeepOff?.();
  transcriptKeepOff = null;
  for (const timer of transcriptTimers.values()) clearTimeout(timer);
  transcriptTimers.clear();
  // Kept once loaded: a later reset has to clear the machines module too, even after a file
  // attached machines without ever routing through this module (`state/machines.test.ts`).
  machineReset?.();
  set({ connectedHosts: [], machineNames: {}, sessions: [] });
}

/**
 * Point the store at a machine's live connection (or at none, when it drops).
 * `startMachineRouting` calls it for every change `state/machines.ts` announces;
 * exported so the routing is testable without the app's shell (machinesThreads.test.ts).
 */
export function watchMachineConnection(id: string, next: AcpConnection | null): void {
  const current = machineConnections.get(id) ?? null;
  if (current === next) return;
  machineOff.get(id)?.();
  machineOff.delete(id);
  if (!next) {
    machineConnections.delete(id);
    // Its threads stay listed and keep the copy this window drew, read-only: the last replay is
    // still the thread. A turn that was running there ended with the connection, and the request
    // it waited on died with it (the engine keeps no pending-permission replay): its card goes
    // with the turn's own failure rather than holding an answer no connection can carry. A request
    // on a thread that was not running goes too, for the same reason (`reply` refuses an answer
    // made in the moment before this runs).
    set((s) => ({
      connectedHosts: [...machineConnections.keys()],
      threads: Object.fromEntries(
        Object.entries(s.threads).map(([key, t]) => {
          if (splitThreadKey(key).host !== id) return [key, t];
          if (isRunning(t)) return [key, finishTurn(t, "failed", Date.now(), MACHINE_LOST_TURN)];
          return [key, t.permissions.length ? { ...t, permissions: [] } : t];
        }),
      ),
    }));
    return;
  }
  machineConnections.set(id, next);
  void subscribePullWatches(next, id).catch(() => {});
  void subscribeWorkflows(next).catch(() => {});
  machineOff.set(
    id,
    next.onInbound((message) => void handleInbound(scopeInbound(message, id), next, id)),
  );
  // Back: a harness or login added there while it was away is in the list a second read gives.
  if (get().remoteHarnesses[id]) void loadMachineHarnesses(id, { refresh: true });
  // Back: a turn whose request died with this machine's channel is asked about again now.
  resumeLostTurns(id);
  // Back: the copy on screen is from before it went away, so the thread it shows is read again.
  // Requests of the old connection did not survive the drop (they died with it), so there is
  // nothing to keep here: the reload is the machine's whole answer about the thread.
  set((s) => ({
    connectedHosts: [...machineConnections.keys()],
    threads: Object.fromEntries(
      Object.entries(s.threads).map(([key, t]) =>
        splitThreadKey(key).host === id
          ? [key, { ...endTurn(t), load: { phase: "idle" as const } }]
          : [key, t],
      ),
    ),
  }));
  // Reload background transcripts too: they may have finished while disconnected.
  for (const key of Object.keys(get().threads)) {
    if (splitThreadKey(key).host === id) void openThread(key);
  }
  for (const row of get().sessions) {
    if (row.host === id && (row.status === "running" || row.status === "needs_input"))
      void openThread(row.id);
  }
}

/** A machine's ids reach the store as keys: `‹host›/‹sessionId›`. */
function scopeInbound(message: Inbound, host: string): Inbound {
  if (message.type === "closed") return message;
  const sessionId = str(message.params.sessionId);
  if (!sessionId) return message;
  return { ...message, params: { ...message.params, sessionId: threadKey(host, sessionId) } };
}

/**
 * Threads whose messages contain `query`, from the engine's SQLite index
 * (`_rustcode/threads/search`): id → the words around the match. Empty for a
 * short query or an engine without the index.
 */
export async function searchThreads(query: string): Promise<Record<string, string>> {
  if (query.trim().length < 3 || !connection) return {};
  try {
    const result = (await connection.request("_rustcode/threads/search", { query, limit: 30 }, 30_000)) as {
      results?: { session_id?: string; snippet?: string }[];
    } | null;
    const hits: Record<string, string> = {};
    for (const hit of result?.results ?? []) if (hit.session_id) hits[hit.session_id] = hit.snippet ?? "";
    return hits;
  } catch {
    return {};
  }
}

export function engine(): AcpConnection {
  if (!connection) throw new Error("The server is not connected.");
  return connection;
}

/** Engine lines, applied together once a frame: one store notification and one render per frame. */
const inbound = frameQueue<string>((lines) =>
  batcher.batch(() =>
    foldingBatch(() => {
      for (const line of lines) {
        try {
          connection?.receive(line);
        } catch (error) {
          // One bad line must not drop the rest of the frame's lines.
          console.error("engine line failed", error);
        }
      }
    }),
  ),
);
/** Lines of server stderr kept for Settings › Diagnostics. */
export const SERVER_LOG_LINES = 1000;
/** Server stderr, kept for Settings › Diagnostics; appended once a frame, not per line. */
const logs = frameQueue<string>(
  (lines) =>
    set((s) => ({ engineLog: [...s.engineLog, ...lines.map(capServerLogLine)].slice(-SERVER_LOG_LINES) })),
  16,
  SERVER_LOG_LINES,
);

// ---------------------------------------------------------------- engine toast

/**
 * The engine's state as one toast (docs/design.md, Loading): nothing while it
 * starts fast, "Starting engine…" with progress once it takes over 2 s, an
 * error with Restart when it stops or crashes, and a short note when it comes
 * back on its own after a crash.
 */
const ENGINE_TOAST = "engine";
const ENGINE_SLOW_MS = 2000;
let slowTimer: ReturnType<typeof setTimeout> | undefined;
let engineWatch: (() => void) | undefined;

/** The store fields an engine event changes, for a `set` that changes more beside it. */
function engineAfter(s: AppState, event: EngineEvent): Pick<AppState, "engine"> {
  return { engine: transitionEngine(s.engine, event) };
}

function engineEvent(event: EngineEvent) {
  set((s) => engineAfter(s, event));
}

/** The engine toast follows the engine's phase; `prev` is absent for the first read. */
function onEngineChange(engine: EngineState, prev?: EngineState) {
  // Settings' answers name this engine's accounts and plans: none of them may
  // show for the next one (a restart can reach a different config).
  if (engine.phase !== "ready") clearPageCache();
  clearTimeout(slowTimer);
  if (engine.phase === "ready") {
    if (engine.restarted && prev) toast("The server restarted", "success", { id: ENGINE_TOAST });
    else dismissToast(ENGINE_TOAST);
    return;
  }
  if (engine.phase === "closed") {
    toast(engine.detail || "The server stopped.", "error", {
      id: ENGINE_TOAST,
      action: { label: "Restart", run: () => void restartEngine() },
    });
    return;
  }
  // Starting: an error toast from before goes at once; a slow start gets its own.
  if (get().toasts.some((t) => t.id === ENGINE_TOAST && t.tone === "error")) dismissToast(ENGINE_TOAST);
  slowTimer = setTimeout(() => {
    const now = get().engine;
    if (now.phase !== "connecting") return;
    toast(startingText(now), "info", { id: ENGINE_TOAST, persist: true, progress: now.progress });
  }, ENGINE_SLOW_MS);
}

function watchEngine() {
  if (engineWatch) return;
  engineWatch = useApp.subscribe((s, prev) => {
    if (s.engine.phase !== prev.engine.phase) onEngineChange(s.engine, prev.engine);
    // A start that is already slow shows how far it has got.
    else if (s.engine.phase === "connecting" && prev.engine.phase === "connecting")
      if (s.engine.progress !== prev.engine.progress)
        updateToast(ENGINE_TOAST, { progress: s.engine.progress });
  });
  onEngineChange(get().engine);
}

/** Start the transport once; every engine (re)start then gets a fresh ACP connection. */
export async function connect(next: Transport): Promise<void> {
  transport = next;
  setShellTransport(next);
  startAgentPolling();
  startPlanPolling();
  startRunStatePoll();
  set((s) => ({ ...engineAfter(s, { type: "start" }), transport: next.name }));
  watchEngine();
  try {
    await next.start((event) => {
      if (event.type === "line") inbound.push(event.line);
      else if (event.type === "lines") for (const line of event.lines) inbound.push(line);
      else if (event.type === "log") logs.push(event.line);
      else if (event.type === "closed") {
        // Every line the engine printed reaches the connection before it closes.
        inbound.flush();
        connection?.close(event.reason);
      }
      // The shell (re)started the engine, after a crash too: initialize a fresh connection.
      else if (event.type === "open") void handshake();
    });
    // The paired computers' supervisors are the shell's too: follow the connections they open.
    void startMachineRouting();
  } catch (error) {
    engineEvent({ type: "closed", detail: errorText(error) });
  }
}

async function handshake(): Promise<void> {
  connection?.close("The server restarted.");
  const conn = new AcpConnection((line) => transport?.send(line));
  connection = conn;
  conn.onInbound((message) => {
    if (connection === conn) void handleInbound(message);
  });
  engineEvent({ type: "handshake" });
  forgetAskedModels();
  try {
    const result = await conn.request(
      Method.initialize,
      initializeParams(APP_VERSION, {
        smartUi: get().smartUi,
        orchestration: get().orchestration ? CAPABILITY_VERSION : undefined,
        // rustcode-viz levels 1 and 2 are drawn natively (ui/Viz.tsx).
        visualizations: 2,
        // Discord Rich Presence (docs/discord-rich-presence.md); whether it publishes is the config.
        presence: 1,
        // A thread a paired device starts appears in this app's sidebar
        // (`_rustcode/threads/created`, docs/remote.md).
        threads: 1,
      }),
      20_000,
    );
    const agent = parseAgentInfo(result);
    // This machine's server settings, put back into the app (state/settingsScope.ts).
    void import("./settingsScope").then((scope) => scope.hydrateLocalServerSettings());
    // Read before the reset below marks every thread idle.
    const interrupted = interruptedThreads();
    set((s) => ({
      agent,
      ...engineAfter(s, { type: "ready" }),
      smartUiPending: false,
      orchestrationPending: false,
    }));
    // Threads that were open lost their engine-side state with the old process.
    forgetRunDrift();
    set((s) => ({
      engineRunning: {},
      threads: Object.fromEntries(
        Object.entries(s.threads).map(([id, t]) => [
          id,
          { ...endTurn(t), load: { phase: "idle" as const }, permissions: [] },
        ]),
      ),
    }));
    await Promise.all([
      refreshSessions(),
      refreshHarnesses(),
      probeFeatures(conn),
      refreshThemes(conn),
      refreshCommands(conn),
      refreshGrants(conn),
    ]);
    // The composer's meter is ready before the first thread opens.
    void refreshPlanCache();
    if (connection === conn) await syncPinsAndSettled(conn);
    // Queued messages and wakes come back from app storage, and the threads that kept one open, so a
    // row's own turn marks what it reports before any wake is worked out below.
    restoreQueuedThreads();
    void subscribePullWatches(conn).catch(() => {});
    void subscribeWorkflows(conn).catch(() => {});
    // Messages written now and sent later come back the same way; one whose time passed while the
    // app was closed goes out once its thread is open.
    restoreScheduled();
    // The link book now lives in the data store; local storage is the first paint only. Once the
    // store's copy is in, a child result nobody was told about (a window closed with its wake still
    // queued) is due: the queue is memory, the book is not, so the wake is read back from it here.
    void syncLinkBook().then(() => wakeTick());
    const selected = get().selectedId;
    if (selected) void openThread(selected);
    if (connection === conn) void resumeInterrupted(interrupted);
    // Waits survive a restart, and one may have come due while the app was closed.
    if (connection === conn) {
      syncWaitTimers();
      void continueDueThreads();
      void continueInternetThreads();
    }
  } catch (error) {
    engineEvent({ type: "closed", detail: errorText(error) });
  }
}

/** Which v2 methods the engine has: -32601 hides a feature, any other answer (even invalid params) shows it. */
async function probeFeatures(conn: AcpConnection): Promise<void> {
  const probe = async (method: string) => {
    try {
      await conn.request(method, {}, 10_000);
      return true;
    } catch (error) {
      return !(error instanceof RpcError && error.isMethodNotFound);
    }
  };
  const [worktrees, checkpoints, status, pins] = await Promise.all([
    probe(V2Method.worktreeList),
    probe(V2Method.checkpointList),
    probe(V2Method.sessionStatus),
    probe(V2Method.sessionPin),
  ]);
  if (connection === conn)
    set((s) => ({ features: { ...s.features, worktrees, checkpoints, status, pins } }));
}

// ---------------------------------------------------------------- commands

/**
 * The engine's command catalog (`_rustcode/commands`): commands otherwise only arrive with a
 * session, so a new thread's drawer would be empty until one had been opened. An engine without
 * the method is an older one; the per-harness lists still fill the drawer once they are known.
 */
async function refreshCommands(conn: AcpConnection): Promise<void> {
  try {
    const answer = obj(await conn.request(Method.commands, {}, 10_000));
    const update = parseSessionUpdate({ update: answer ?? {} });
    if (connection !== conn || update.kind !== "commands") return;
    set({ baseCommands: update.commands });
  } catch {
    // No catalog: a new thread shows what its harness last advertised, if anything.
  }
}

// ---------------------------------------------------------------- pins and settled

/**
 * When the app began sending pull request news into threads (PR 3498, 2026-11-11). Until issue 3721 such a
 * notice marked a delegate as messaged by the user, so it left its parent for a row of its own; only
 * marks made since then can be one of those.
 */
const NOTICE_MESSAGED_SINCE = 1_794_402_000_000;

/**
 * Give delegates a notice wrongly marked as the user's back to their parent: each delegate marked
 * since `NOTICE_MESSAGED_SINCE` is loaded once and unmarked when every message in it besides its
 * task is an app notice. A failed or offline load is tried again on the next connection.
 */
let repairingNoticeMessaged = false;
async function repairNoticeMessaged(): Promise<void> {
  if (repairingNoticeMessaged) return;
  repairingNoticeMessaged = true;
  const key = "rustcode.noticeMessagedRepaired";
  const checked = storedJson<Record<string, number>>(key, {});
  try {
    for (const session of get().sessions) {
      const stamp = get().messaged[session.id];
      if (
        session.parent?.relationship !== "delegate" ||
        stamp === undefined ||
        stamp < NOTICE_MESSAGED_SINCE ||
        threadIsRemote(session.id) ||
        checked[session.id] !== undefined
      )
        continue;
      const error = await openThread(session.id);
      const thread = get().threads[session.id];
      if (error || !isLoaded(thread)) continue;
      // A real message sent while history was loading must keep its row.
      if (get().messaged[session.id] !== stamp) continue;
      if (noticeOnlyHistory(thread.items)) {
        const messaged = { ...get().messaged };
        delete messaged[session.id];
        set({ messaged });
        store(STORAGE.messaged, JSON.stringify(messaged));
      }
      checked[session.id] = stamp;
      store(key, JSON.stringify(checked));
    }
  } finally {
    repairingNoticeMessaged = false;
  }
}

/**
 * Pins live in the engine when it has `_rustcode/session/pin`: local pins
 * move there once. Archived threads (an older app's) become Done once,
 * since Settled is exactly the threads marked Done.
 */
async function syncPinsAndSettled(conn: AcpConnection): Promise<void> {
  void repairNoticeMessaged();
  if (!stored(STORAGE.archiveMigrated)) {
    const archived = Object.keys(get().archived);
    store(STORAGE.archiveMigrated, "1");
    for (const id of archived) if (statusOf(get(), id) !== "done") void setThreadStatus(id, "done");
    if (archived.length) {
      set({ archived: {} });
      store(STORAGE.archived, "{}");
    }
  }
  if (!get().features.pins) return;
  const remote = get()
    .sessions.filter((x) => x.pinned)
    .map((x) => x.id);
  const local = get().pinned;
  if (!stored(STORAGE.pinsMigrated)) {
    const missing = local.filter((id) => !remote.includes(id) && get().sessions.some((x) => x.id === id));
    await Promise.all(
      missing.map((id) =>
        conn.request(V2Method.sessionPin, { session_id: id, pinned: true }, 10_000).catch(() => null),
      ),
    );
    store(STORAGE.pinsMigrated, "1");
    remote.push(...missing);
  }
  // The engine's pins, in the order the app last kept them.
  const pinned = [
    ...local.filter((id) => remote.includes(id)),
    ...remote.filter((id) => !local.includes(id)),
  ];
  set({ pinned });
  store(STORAGE.pinned, JSON.stringify(pinned));
}

/** Why a v2 feature is hidden, for its tooltip; null when the engine has it. */
export function featureMissing(feature: keyof AppState["features"]): string | null {
  const known = get().features[feature];
  if (known) return null;
  if (known === undefined) return "Checking what the server supports…";
  const what = {
    worktrees: "worktrees",
    checkpoints: "checkpoints",
    status: "thread status",
    themes: "themes",
    pins: "pins",
  }[feature];
  return `This server has no ${what} yet (update RustCode).`;
}

export async function restartEngine(): Promise<void> {
  if (!transport) return;
  engineEvent({ type: "restart" });
  await transport.restart().catch((error) => engineEvent({ type: "closed", detail: errorText(error) }));
}

/**
 * `_rustcode/session/activity`: a turn parked on background commands (`waiting`), woken when they
 * finish (`running`), and that woken turn ending (`idle`). Only a woken turn is finished here; a
 * prompt the app sent ends with its own response. A loading thread's waits (`receiveActivity`).
 */
export function onSessionActivity(params: unknown) {
  const p = obj(params);
  const id = str(p?.sessionId);
  const state = str(p?.state);
  if (!id || !get().threads[id]) return;
  const background = typeof p?.background === "number" ? p.background : 0;
  const before = get().threads[id];
  const wokenEnd = state !== "running" && isRunning(before) && isWoken(before);
  updateThread(id, (t) => {
    if (state === "running") return { ...wakeTurn(t), waiting: background || undefined };
    const ended = isRunning(t) && isWoken(t) ? finishTurn(t, "completed") : t;
    if (state === "waiting") return { ...ended, waiting: background || undefined };
    return { ...ended, waiting: undefined };
  });
  // The turn a thread woke itself for ended: the one finish worth announcing, when nothing it
  // started is still going (a turn that ended on "waiting on the tests" said nothing).
  if (wokenEnd) announceFinished(id);
  if (state !== "running") scheduleAutoCompact(id);
}

/**
 * `_rustcode/session/childActivity`: what the processes the thread's harness started are doing
 * (a build, a test run) while its turn runs. Kept for the stall notice, which counts a child that
 * still uses CPU as progress; no row is written for it.
 */
function onChildActivity(params: unknown) {
  const p = obj(params);
  const id = str(p?.sessionId);
  if (!id || !get().threads[id]) return;
  const processes = typeof p?.processes === "number" ? p.processes : 0;
  const cpuMs = typeof p?.cpuMs === "number" ? p.cpuMs : 0;
  const busiest = str(p?.busiest);
  const activeAgo = typeof p?.activeAgoMs === "number" ? p.activeAgoMs : undefined;
  const now = Date.now();
  updateThread(id, (t) =>
    withChildActivity(
      t,
      processes > 0
        ? {
            processes,
            cpuMs,
            ...(busiest ? { busiest } : {}),
            ...(activeAgo !== undefined ? { activeAt: now - activeAgo } : {}),
          }
        : undefined,
    ),
  );
}

/** `_rustcode/settings/changed`: the engine saw `settings.json` change. */
const SETTINGS_CHANGED = "_rustcode/settings/changed";

async function handleInbound(message: Inbound, conn: AcpConnection | null = connection, host = LOCAL_HOST) {
  switch (message.type) {
    case "closed":
      if (host) {
        // One machine's channel closed: this computer's engine is untouched. Its threads stay
        // listed and read-only (`watchMachineConnection`), and a turn it ran ended with it.
        return;
      }
      set((s) => ({
        ...engineAfter(s, { type: "closed", detail: message.reason }),
        threads: Object.fromEntries(
          Object.entries(s.threads).map(([id, t]) => [
            id,
            isRunning(t) ? finishTurn(t, "failed", Date.now(), "The server stopped during this turn.") : t,
          ]),
        ),
      }));
      return;
    case "notification":
      if (message.method === Method.sessionUpdate) onSessionUpdate(message.params);
      else if (message.method === Method.sessionActivity) receiveActivity(message.params);
      else if (message.method === Method.sessionChildActivity) onChildActivity(message.params);
      else if (message.method === Method.threadCreated) threadCreated(message.params);
      else if (message.method === Method.threadDeleted) threadDeleted(message.params);
      else if (message.method === V2Method.orchestrationCancelled) orchestrationCallDropped(message.params);
      else if (message.method === PULL_WATCH && conn)
        void deliverPullWatch(conn, message.params, host).catch(() => {});
      else if (message.method === WorkflowMethod.changed) onWorkflowChanged(message.params, host);
      else if (message.method === ThemeMethod.changed) onThemesChanged(message.params);
      // A setting changed outside this window: the CLI, an agent's `manage_settings`, another app.
      else if (message.method === SETTINGS_CHANGED)
        void import("./settingsScope").then((scope) => scope.onServerSettingsChanged(host, message.params));
      // A sign-in or a call now waits for the owner on that machine, or no longer does.
      else if (message.method === "_rustcode/outside/changed")
        void import("./outside").then((outside) => outside.refreshOutside(host));
      return;
    case "request":
      if (message.method === Method.requestPermission) {
        const parsed = parsePermissionRequest(message.id, message.params);
        if (!parsed) {
          conn?.respond(message.id, permissionCancelled);
          return;
        }
        // A thread this window has not opened (a child started elsewhere, or a thread after a
        // restart) is loaded first: the request is shown in its own chat and in the thread that
        // started it, and both answer it through the same card.
        if (!isLoaded(get().threads[parsed.sessionId])) await openThread(parsed.sessionId);
        const request = { ...parsed, at: Date.now() };
        if (!get().threads[request.sessionId]) {
          conn?.respond(message.id, permissionCancelled);
          return;
        }
        const auto = autoAnswer(request, permissionModeOf(get(), request.sessionId));
        if (auto) {
          conn?.respond(message.id, permissionSelected(auto.id));
          return;
        }
        updateThread(request.sessionId, (t) => {
          // An approval holds its call: the turn's Trace draws that time apart
          // from the call's work (state/thread.ts `waitingOn`).
          const held = request.question ? t : waitingOn(t, request.toolCallId, request.at ?? Date.now());
          return { ...held, permissions: [...held.permissions, request] };
        });
        // Its links stop their work clock while the user decides (state/delegates.ts).
        linkWaitStarted(request.sessionId);
        // The inbox is the screen that lists this request already: a toast there would
        // stack over its own first rows. Other toasts (copied, finished) still show. A thread the
        // user snoozed says nothing either: the request waits there until the snooze ends, and the
        // inbox and the badge leave it out until then (state/inbox.ts, state/notify.ts).
        const asleep = request.sessionId in get().snoozed;
        if (!asleep && request.sessionId !== get().selectedId && get().view.kind !== "inbox") {
          // On another computer, the notice says which one: two machines may run a thread each.
          const machine = machineOfThread(request.sessionId);
          toast(
            `${request.question ? "A thread has a question" : "A thread needs approval"}${machine ? ` on ${machine}` : ""}`,
            "info",
          );
        }
        // The system says the same, so a thread waiting behind another window is not forgotten.
        notifyThread(
          {
            threadId: request.sessionId,
            kind: request.question ? "question" : "approval",
            key: String(request.rpcId),
            title: notifyTitle(request.sessionId),
            detail: request.question?.text,
          },
          get(),
        );
        // A child that now waits on the user: its parent hears about it once.
        wakeTick();
        return;
      }
      if (message.method === V2Method.orchestration) {
        // A machine's threads are not this app's to orchestrate yet: its `start_session` would
        // create the child on this computer. Refused until cross-machine delegation lands.
        if (host) {
          conn?.respond(
            message.id,
            failureJson(failure("not_allowed", "Delegation across machines is not available yet.")),
          );
          return;
        }
        const result = await answerOrchestration(message.params);
        conn?.respond(message.id, result);
        return;
      }
      // Nothing else is advertised; answer so the engine never waits.
      conn?.respondError(message.id, -32601, `Method not found: ${message.method}`);
      return;
  }
}

function onSessionUpdate(params: JsonObject) {
  const sessionId = str(params.sessionId);
  if (!sessionId) return;
  const metadata = obj(obj(obj(params.update)?._meta)?.rustcode);
  if (metadata?.snoozedUntil === null || typeof metadata?.snoozedUntil === "number") {
    const until = metadata.snoozedUntil as number | null;
    applySnooze(sessionId, until !== null && until > Date.now() ? until : null);
  }
  let update = parseSessionUpdate(params);
  // A compaction the app sent for the thread's own good says so, whichever
  // harness produced the card. The card's before → after comes from the usage
  // the meter keeps (`autoCompactBeforeUsage`).
  // The harness's own "started" and "failed" are not the card: a failure ends the compaction
  // without the "Compacted automatically" notice that would follow a quiet turn.
  if (update.kind === "compaction" && update.phase !== "started" && noteCompactionCard(sessionId)) {
    if (update.phase !== "failed") update = { ...update, text: automaticCompactionText(update.text) };
  }
  if (update.kind === "commands") {
    // The thread's own list, and the harness's last one for the next new thread on it.
    const harness = harnessOf(get(), sessionId);
    const next = harness ? rememberCommands(get().harnessCommands, harness, update.commands) : undefined;
    set((s) => ({
      sessionCommands: { ...s.sessionCommands, [sessionId]: update.commands },
      ...(harness && next ? { harnessCommands: next } : {}),
    }));
    if (next) store(STORAGE.harnessCommands, serializeHarnessCommands(next));
    return;
  }
  if (isReportUpdate(params) && update.kind === "agent") {
    // A report streams in chunks: they gather in one panel over the thread's composer and stay
    // out of the transcript (state/report.ts reads its shape).
    set((s) => ({
      report: {
        threadId: sessionId,
        text: s.report?.threadId === sessionId ? s.report.text + update.text : update.text,
      },
    }));
    return;
  }
  const status = statusInUpdate(params);
  if (status) {
    if (isRemoteThread(sessionId) && (status === "running" || status === "needs_input"))
      void openThread(sessionId);
    if (status === "done") clearSettledNotifications([sessionId]);
    set((s) => ({ sessions: s.sessions.map((x) => (x.id === sessionId ? { ...x, status } : x)) }));
    clearStatusMark(sessionId);
    // A lost turn on this thread is settled now if the machine says it stopped running there.
    lostTurnStatus(sessionId, status);
    if (status !== "running" && status !== "needs_input") reconcileLinks();
  }
  if (update.kind === "title") {
    set((s) => ({
      sessions: s.sessions.map((x) => (x.id === sessionId ? { ...x, title: update.title } : x)),
    }));
  }
  updateThread(sessionId, (t) => applyUpdate(t, update));
  if (update.kind === "toolCallUpdate" || update.kind === "toolCall") {
    const item = get().threads[sessionId]?.items.find(
      (item) => item.kind === "tool" && item.id === update.id,
    );
    if (item?.kind === "tool" && item.status === "completed") {
      const pull = createdPull(item.input, item.output ?? "");
      const cwd = get().sessions.find((session) => session.id === sessionId)?.cwd;
      if (pull && cwd)
        void connectionForThread(sessionId)
          ?.request(
            "_rustcode/pulls/watch/start",
            {
              sessionId: splitThreadKey(sessionId).id,
              cwd,
              ...pull,
              link: true,
            },
            60_000,
          )
          .catch(() => {});
    }
  }
}

// ---------------------------------------------------------------- sessions

/**
 * A thread the engine started, or another client made on this machine or on a paired one
 * (`_rustcode/threads/created`, docs/remote.md and docs/desktop.md › Several machines): a
 * thread has no row in the store before its first turn. Remote threads are followed in background.
 */
export function threadCreated(params: unknown) {
  const p = obj(params);
  const key = str(p?.sessionId);
  const cwd = str(p?.cwd);
  if (!key || !cwd) return;
  if (deferAnnouncement(key, () => threadCreated(params))) return;
  // A thread trashed a moment ago is not made again by a late announcement.
  if (isTombstoned(key)) return;
  const { host } = splitThreadKey(key);
  const parent = parseSessionList({ sessions: [p ?? {}] }).sessions[0]?.parent;
  const summary: SessionSummary = {
    parent: parent
      ? { ...parent, id: parent.id && host ? threadKey(host, parent.id) : parent.id }
      : undefined,
    id: key,
    // A machine's own id reaches the store as `‹host›/‹id›` (state/threadKeys.ts).
    host: host || undefined,
    cwd,
    updatedAt: Date.now(),
    title: str(p?.title) || undefined,
    harness: str(p?.harness) || undefined,
    model: str(p?.model) || undefined,
  };
  // Both lists read the one keep-unlisted set: local in `refreshSessions`, a machine's in
  // `mergeMachineLists` and `machines.ts` `readThreads`. Kept even when the row is already listed:
  // the list a read is holding may have started before the thread, and must not take it away.
  keepUnlisted(summary);
  set((s) => {
    if (s.sessions.some((x) => x.id === key)) return {};
    return { sessions: [summary, ...s.sessions] };
  });
  if (host) void openThread(key);
}

/**
 * The computers whose own thread list came in whole this run: this one (LOCAL_HOST) once
 * `refreshSessions` read every page, a paired machine once `machines.ts` `readThreads` did. Only
 * there does a thread missing from the list mean it is gone.
 */
const wholeLists = new Set<string>();

/** A computer's list came in (`whole` when it was every page), or it can no longer be trusted. */
export function noteThreadList(host: string, whole: boolean): void {
  if (whole) wholeLists.add(host);
  else wholeLists.delete(host);
}

/**
 * Whether a thread no longer exists: its computer listed every thread it has and this one was not
 * among them. A list that stopped at its page cap, a machine that has not answered yet, or a read
 * that failed is not news that it is gone, so what waits on the thread keeps waiting.
 */
export function threadIsGone(id: string): boolean {
  const s = get();
  if (s.sessions.some((x) => x.id === id)) return false;
  const host = isRemoteThread(id) ? splitThreadKey(id).host : LOCAL_HOST;
  if (host === LOCAL_HOST && (!sessionsListed(s.engine) || s.sessionsCapped)) return false;
  return wholeLists.has(host);
}

export async function refreshSessions(options: { quiet?: boolean } = {}): Promise<void> {
  const conn = connection;
  if (!conn || !get().agent?.listSessions) {
    noteThreadList(LOCAL_HOST, false);
    set((s) => ({ ...engineAfter(s, { type: "listed" }), sessionsCapped: false }));
    return;
  }
  try {
    const all: SessionSummary[] = [];
    let cursor: string | undefined;
    let capped = false;
    for (let page = 0; page < 10; page++) {
      const params: JsonObject = cursor ? { cursor } : {};
      const result = parseSessionList(await conn.request(Method.sessionList, params, 15_000));
      all.push(...result.sessions);
      cursor = result.nextCursor;
      if (!cursor) break;
      if (page === 9) capped = true;
    }
    // A thread trashed while this read was out is not news that it came back.
    for (let i = all.length - 1; i >= 0; i--) if (isTombstoned(all[i]?.id ?? "")) all.splice(i, 1);
    // The engine's list is the newer truth about a status.
    reconcileStatusMarks(all);
    for (const x of all)
      if (x.snoozedUntil !== undefined)
        applySnooze(x.id, x.snoozedUntil !== null && x.snoozedUntil > Date.now() ? x.snoozedUntil : null);
    // Another computer's list is its own read: ask each connected machine again, too. The poll
    // (`startRunStatePoll`) reads this computer's engine only: a machine reports its own turns.
    if (!options.quiet) void import("./machines").then((m) => m.refreshMachineThreads());
    // A thread the engine does not list yet stays while the app keeps it; one it names now (or
    // one whose keep expired) needs nothing kept.
    const listed = new Set(all.map((x) => x.id));
    settleKeptUnlisted(LOCAL_HOST, listed);
    const stillKept = new Set(keptUnlisted(LOCAL_HOST, listed).map((x) => x.id));
    set((s) => {
      const started = s.sessions.filter(
        (x) => !x.host && !listed.has(x.id) && (s.threads[x.id] || stillKept.has(x.id)),
      );
      // Another machine's threads are listed by that machine (state/machines.ts), kept as they are.
      const remote = s.sessions.filter((x) => x.host);
      const sessions = byActivity([...started, ...all, ...remote]);
      noteThreadList(LOCAL_HOST, !capped);
      return {
        sessions,
        ...engineAfter(s, { type: "listed" }),
        sessionsCapped: capped,
        // The new-thread screen starts in this computer's most recent project: another machine's
        // folder is not a folder here (**Run on** moves the thread there, with its own project).
        projectCwd: s.projectCwd || sessions.find((x) => !x.host)?.cwd || "",
      };
    });
    // A thread the engine no longer lists keeps no place in the arrangement. A capped list is not
    // the whole list, so it prunes nothing: the rows of the rest are still there.
    if (!capped) pruneSidebarOrder(get().sessions.map((x) => x.id));
    // The window's run flags give way to the engine's list where they have drifted (issue 3652).
    reconcileRunState(all);
    // A link still Working beside a thread the engine says is idle is settled (issue 3541).
    reconcileLinks();
  } catch (error) {
    engineEvent({ type: "listed" });
    if (!options.quiet) toast(`Could not list threads: ${errorText(error)}`, "error");
  }
}

// ---------------------------------------------------------------- turns

export function setDraft(key: string, text: string) {
  const next = withDraft(get().drafts, key, text);
  if (next !== get().drafts) set({ drafts: next });
  if (text.trim()) noteTyping(key);
  saveDrafts();
}

/**
 * Drafts are written to app storage a moment after typing stops, so a long paste or a fast
 * typist does not write the whole book on every keystroke. Sending clears the draft (the
 * composer sets it empty), which takes its entry out of the book. Storage gets `persistable`:
 * nothing over 100 KB, and the 200 newest drafts.
 */
const DRAFTS_SAVE_MS = 400;
let draftsSave: ReturnType<typeof setTimeout> | undefined;

function writeDrafts() {
  store(DRAFTS_KEY, JSON.stringify(persistable(get().drafts)));
}

function saveDrafts() {
  if (draftsSave) clearTimeout(draftsSave);
  draftsSave = setTimeout(() => {
    draftsSave = undefined;
    writeDrafts();
  }, DRAFTS_SAVE_MS);
}

/** The book as it is now, written at once: for a page that is going away. */
export function flushDrafts() {
  if (!draftsSave) return;
  clearTimeout(draftsSave);
  draftsSave = undefined;
  writeDrafts();
}

export function setExtras(key: string, fn: (extras: ComposeExtras) => ComposeExtras) {
  set((s) => ({ extras: { ...s.extras, [key]: fn(s.extras[key] ?? emptyExtras) } }));
  if (hasExtras(get().extras[key])) noteTyping(key);
}

// ---------------------------------------------------------------- running a turn

export async function runPrompt(
  id: string,
  text: string,
  extras: ComposeExtras = emptyExtras,
  options: {
    fromView?: boolean;
    link?: string;
    sender?: string;
    /**
     * The links a restart severed and this resume turn answers (`resumeInterrupted`): started as
     * working when its turn begins and settled when it ends, as a link's own turn leaves its link
     * (host.ts `linksResumed`, `linksFinished`).
     */
    resumeLinks?: string[];
    wake?: boolean;
    /** The links a queued wake row reports: marked as heard when its turn starts (host.ts). */
    wakeLinks?: string[];
    /** The links that row named as waiting on the user: a question, not a finish (host.ts). */
    wakeAsking?: string[];
    automatic?: boolean;
    /** A `/compact` the app sent because the context or the cache said so. */
    autoCompact?: boolean;
    /** What the "Compacting context…" divider says for that `/compact` (`autoCompactingText`). */
    autoCompactText?: string;
    /**
     * The turn that continues a thread the app was waiting for (a usage limit, an overload, the
     * internet): its wait is kept while it runs and settled when it ends.
     */
    autoResume?: boolean;
    /**
     * The turn that carries the thread on after the app compacted it: its prompt reads as a divider
     * in the transcript, never as a message the user typed (`state/thread.ts` `CONTINUE_MARK`).
     */
    continued?: boolean;
    /**
     * The next message of a thread's queue, started because the turn before it ended: work the user
     * already sent, so starting it is not a new send and does not bring a snoozed thread back
     * (`state/store.ts` `wakeOnWrite`). Unlike `continued`, it is the user's turn in every other way.
     */
    queued?: boolean;
    /**
     * The thread was made for this message, and the new-thread screen already shows its row
     * (`state/setup.ts`): it does not fly in when the thread takes the screen over.
     */
    alreadyShown?: boolean;
  } = {},
): Promise<TurnOutcome> {
  // Check again when a queued notice starts: settling may have happened after delivery.
  if (
    (options.automatic ||
      options.autoCompact ||
      options.autoResume ||
      options.continued ||
      options.wake ||
      isContinueMessage(text) ||
      isWakeMessage(text)) &&
    (get().statusMarks[id] ?? sessionById(get(), id)?.status) === "done"
  )
    return "skipped";
  // The turn runs on the thread's own computer, which may have gone away since it was opened.
  const conn = connectionForThread(id);
  if (!conn) {
    toast(threadIsRemote(id) ? "That computer is not connected." : "The server is not connected.", "error");
    return "skipped";
  }
  const thread = get().threads[id];
  // The user starting a turn again settles any stop they made before it.
  if (!options.automatic) clearStopped(id);
  // The prompt reads as a divider both ways: live, and after a reload from the history the engine
  // kept, which carries the text and not this flag (state/thread.ts `CONTINUE_MARK`).
  if (options.continued) text = withContinueMark(text);
  // A prompt no agent asked for is the user's: the thread is theirs now, even
  // when it was started by an agent (state/topLevel.ts). A resume after a
  // restart, a wake or a message from another thread is not a message the user
  // sent, and neither is the turn that continues after a wait.
  if (userTypedPrompt(options, text)) markMessaged(id);
  // Whoever starts a turn now moots the wait, and if this one stops the same way it waits again.
  if (!options.autoResume && !options.autoCompact && !isRunning(thread)) dropWaits(id);
  // Never a second prompt into a running turn: an external harness takes it as the next turn and can
  // interrupt the tools it is still running (a parent's wait_session). It waits in the queue, like a
  // message typed meanwhile, and starts when this turn ends.
  if (isRunning(thread)) {
    // An automatic compaction never waits in the queue: the next turn may run
    // before it, and the guards will offer another one in a later idle period.
    if (options.autoCompact) return "skipped";
    updateThread(id, (t) => ({
      ...t,
      queue: [
        ...t.queue,
        {
          id: localId("queued"),
          text,
          extras: carriesExtras(extras) ? extras : undefined,
          fromView: options.fromView,
          link: options.link,
          resume: options.resumeLinks,
          wake: options.wake,
          wakeLinks: options.wakeLinks,
          wakeAsking: options.wakeAsking,
        },
      ],
    }));
    return "skipped";
  }
  // A user turn starts a new idle period, which may auto-compact once more. The app's own continue is
  // not one: it is the same idle period as the compaction that asked for it, and re-arming the rule
  // that just ran would let a harness whose compaction left the context over the threshold compact
  // and continue for ever.
  if (!options.autoCompact && !options.continued) beginAutoCompactIdle(id);
  // Writing in a thread brings it back from a snooze, as it always has: that is the user asking for
  // it (`submit` and `steerOrSubmit` have already done it for a message they took). A turn the app
  // starts — a delegate's wake, a resume after a restart, the next row of the queue (`queued`), the
  // turn after a usage limit — is not a new send and leaves the snooze alone, so a sleeping thread
  // keeps working in the background without coming back to the list.
  if (userTypedPrompt(options, text) && !options.queued) wakeOnWrite(id);
  const startIndex = thread?.items.length ?? 0;
  if (options.link) linkStarted(options.link);
  // The turn a restart resumes this thread with answers the links the quit severed: they work again
  // from here, so the parent reads this turn's answer and its time, never the time before the quit.
  if (options.resumeLinks?.length) {
    linksResumed(options.resumeLinks);
    // Links a usage limit settled as failed work again: this turn is the one that answers them.
    linksReopened(id, options.resumeLinks);
  }
  // A queued wake reports its links now, as its turn starts: late enough that a restart before this
  // point brings the wake back, early enough that nothing in the turn reports it again.
  if (options.wake && options.wakeLinks?.length) wakeDelivered(options.wakeLinks, options.wakeAsking ?? []);
  // The message the transcript shows: the command, links and typed text, with each long paste back
  // where it was pasted. The prompt's blocks say the same thing (below).
  const shown = composedText(text, extras);
  // A message typed here flies out of the composer on its first sighting; one the new-thread screen
  // already drew does not (lib/sendFlight.ts).
  const sent = userTypedPrompt(options, text) && !options.fromView && !options.alreadyShown;
  // The message is on screen before its blocks are built: a long paste or a 20 MB picture is copied
  // into the prompt's JSON after the bubble lands, not before it — and that update gets a painted
  // frame of its own, so the bubble's row (and the flight it starts) is drawn before the work that
  // follows it, which on a fast harness is the whole rest of the turn's first frame.
  updateThread(id, (t) =>
    options.autoCompact
      ? // The turn writes no rows, so "Compacting context…" is the one sign the thread is busy.
        startCompaction(beginQuietTurn(t), options.autoCompactText, "auto-compact")
      : beginTurn(
          t,
          shown,
          Date.now(),
          sentAttachments(extras),
          options.fromView,
          options.wake,
          sent,
          options.continued,
        ),
  );
  await afterPaint();
  const harness = threadHarness(thread);
  const prompt = promptBlocks(text, extras, {
    images: acceptsImages(thread),
    builtin: harness?.builtin ?? true,
    cwd: thread?.cwd ?? "",
    advertised: advertisedCommands(get(), id),
  });
  // A new prompt starts a new report, if it is one.
  if (get().report?.threadId === id) set({ report: null });
  if (!options.autoCompact) {
    set((s) => ({
      sessions: byActivity(
        s.sessions.map((x) =>
          x.id === id
            ? { ...x, updatedAt: Date.now(), title: x.title ?? (titleFrom(shown, 80) || "Image") }
            : x,
        ),
      ),
    }));
  }
  let outcome: "completed" | "cancelled" | "failed" = "completed";
  let failure: string | undefined;
  let unused: string[] = [];
  try {
    const sent = threadIsRemote(id) ? await remotePromptImages(conn, id, prompt) : prompt;
    const request: JsonObject = { sessionId: splitThreadKey(id).id, prompt: sent };
    // The access level rides on every prompt: a harness that sandboxes its own shell (Codex) leaves
    // the sandbox for Full access, on this machine and on a remote one alike.
    request._meta = {
      rustcode: {
        access: permissionModeOf(get(), id),
        ...(options.sender && parseDelivered(text)
          ? {
              origin: {
                kind: "agent",
                threads: [
                  {
                    sessionId: options.sender,
                    title: parseDelivered(text)?.title ?? "",
                    ...(options.link ? { linkId: options.link } : {}),
                  },
                ],
              },
            }
          : {}),
        ...(options.autoCompact ? { autoCompact: true } : {}),
      },
    };
    const result = (await conn.request(Method.sessionPrompt, request)) as JsonObject | null;
    if (str(result?.stopReason) === "cancelled") outcome = "cancelled";
    const meta = (result?._meta as JsonObject | undefined)?.rustcode as JsonObject | undefined;
    unused = Array.isArray(meta?.unusedSteers)
      ? meta.unusedSteers.filter((x): x is string => typeof x === "string")
      : [];
  } catch (error) {
    // A turn on another machine outlives the request that asked for it: the request dies with the
    // machine's connection, and the turn there goes on. Nothing is settled from the rows that had
    // arrived — a half-written answer is not an answer — so the links are handed to `lostTurn`: it
    // asks the machine how the thread ended once its channel is back, and gives them that answer
    // then, or their failure after `LOST_MACHINE_MS` (issue 3440). Anything else is the engine's own
    // refusal, and a failure as it always was.
    if (error instanceof ConnectionClosedError && threadIsRemote(id)) {
      // The channel's close usually ended the turn already (`watchMachineConnection`); when a
      // closed channel is what refused the request, this is what ends it.
      if (isRunning(get().threads[id]))
        updateThread(id, (t) => finishTurn(t, "failed", Date.now(), MACHINE_LOST_TURN));
      lostTurn(id, [options.link, ...(options.resumeLinks ?? []), ...takeSteeredLinks(id)]);
      return "skipped";
    }
    outcome = "failed";
    failure = errorText(error);
  }
  // A provider's raw answer (`Codex: {"detail":"Bad Request"}`) is said in words, with what to do
  // when the conversation is full; the raw text stays on the thread, behind the error's Details.
  const rawFailure = failure;
  if (failure !== undefined) failure = readableTurnError(failure, contextPercent(get().threads[id]?.usage));
  updateThread(id, (t) => ({
    ...requeueSteers(finishTurn(t, outcome, Date.now(), failure, options.autoCompact), unused),
    ...(rawFailure !== failure ? { errorDetail: rawFailure } : {}),
  }));
  // Steers the turn never read wait as turns of their own, and carry the links they came from.
  if (!options.autoCompact) requeueUnreadSteers(id, unused);
  // The stop is settled first: a turn it stopped is reported as the stop, not twice.
  // An automatic compaction must leave a pending wait alone, not settle it.
  const waiting = options.autoCompact
    ? (get().limitWaits[id] ?? null)
    : // The stop is read from what the harness said, not the words the thread shows.
      settleStopWait(id, outcome, rawFailure, startIndex, [options.link, ...(options.resumeLinks ?? [])]);
  // A thread waiting for the internet is held, not dead: it says so in the composer's row.
  const offline = options.autoCompact ? undefined : get().internetWaits[id];
  // An automatic compaction is not a turn the user waits for: it is not "Done"
  // news, and it must not wake the thread's parent as if its work finished.
  if (!options.autoCompact) {
    if (outcome === "completed") markFinished(id);
    else if (outcome === "failed") markFinished(id, Date.now(), true);
  }
  // A report command that answered in the panel leaves no turn behind: its message and footer go.
  const reported = get().report?.threadId === id;
  if (reported && outcome === "completed" && isReportCommand(id, text, extras)) {
    updateThread(id, (t) => ({ ...t, items: t.items.slice(0, startIndex) }));
  }
  if (options.link)
    linkFinished(options.link, id, outcome, failure, get().threads[id]?.items.slice(startIndex) ?? []);
  // The links this resume turn answers settle together: one write, one wake tick (host.ts).
  if (options.resumeLinks?.length)
    linksFinished(
      options.resumeLinks,
      id,
      outcome,
      failure,
      get().threads[id]?.items.slice(startIndex) ?? [],
      // Stopped on a limit or an overload again: the parent heard it once and waits for the end.
      !!waiting,
    );
  // Every finished turn of a thread matters to a parent waiting on it: a middle thread's real
  // answer is the one after the work it started (state/host.ts threadTurnEnded).
  if (!options.autoCompact) {
    threadTurnEnded(id, !!options.wake, outcome, failure, get().threads[id]?.items.slice(startIndex) ?? []);
  }
  // An automatic compaction must not move the thread in the sidebar: no list
  // refresh that would read the turn as new activity.
  if (!options.autoCompact) void refreshSessions();
  void refreshPlanUsage(id);
  // A window's numbers move with the turn, and so does the account's own row:
  // the thread on screen asks again as it ends.
  if (id === get().selectedId) void refreshPlanCache();
  void refreshAgents(id);
  void refreshCheckpoints(id);
  // A delegate stopped on a plan limit may go on elsewhere at once (issue 3392); the move says so.
  const move = waiting && !options.autoCompact ? limitMoveFor(id, waiting) : null;
  const moved = move && waiting ? await switchOnLimit(id, waiting, move) : false;
  if (move && !moved) {
    notifyThread({ threadId: id, kind: "limit", key: String(waiting!.until), title: threadName(id) }, get());
  }
  if (
    !moved &&
    !options.autoCompact &&
    !options.continued &&
    (outcome === "completed" || outcome === "failed")
  ) {
    finishToast(id, outcome, { waiting, offline, link: options.link });
  }
  // A queued message starts at once, so "working" stays up across the hand-off. A turn that was
  // stopped or failed leaves the queue held instead (thread.ts `holdsQueue`): the rows read Held and
  // Resume starts them.
  // Nor does one go out into a wait: it waits for the turn that continues.
  const after = get().threads[id];
  const next = after && !waiting && !moved && !offline ? nextQueued(after.queue, after) : undefined;
  if (next) {
    removeQueued(id, next.id);
    await runPrompt(id, next.text, next.extras, {
      fromView: next.fromView,
      link: next.link,
      resumeLinks: next.resume,
      wake: next.wake,
      wakeLinks: next.wakeLinks,
      wakeAsking: next.wakeAsking,
      automatic: next.resume !== undefined,
      // The row was typed before this turn: starting it is not a new send (`runPrompt` above).
      queued: true,
    });
  }
  if (outcome === "completed" && !options.continued) scheduleAutoCompact(id);
  return outcome;
}

/** How a turn ended, and `skipped` for a prompt that never became one (a queued one, a compaction that found a turn running). */
export type TurnOutcome = "completed" | "cancelled" | "failed" | "skipped";

export async function cancel(id = get().selectedId): Promise<void> {
  if (!id) return;
  const conn = connectionForThread(id);
  // A stop that finds nothing to send it on says so, instead of doing nothing.
  if (!conn) {
    toast(threadIsRemote(id) ? "That computer is not connected." : "The server is not connected.", "error");
    return;
  }
  const thread = get().threads[id];
  // Stopping a thread stops the work it gave other threads (M1); turns the user started there go on.
  void stopLinksFrom(id);
  // The engine may run a turn this window lost track of (`engineRunning`, the list's status): Stop
  // still stops it.
  const engineRuns =
    !!get().engineRunning[id] || (!isLoaded(thread) && engineBusy(sessionById(get(), id)?.status));
  if (!isRunning(thread) && !engineRuns) return;
  // Recorded first: a quit inside the harness's cancel grace would otherwise leave the thread
  // listed as running, and the restart would resume a turn the user stopped.
  noteStopped(id);
  try {
    conn.notify(Method.sessionCancel, { sessionId: splitThreadKey(id).id });
    // ACP: after `session/cancel` the client answers outstanding permission requests as cancelled.
    for (const request of thread?.permissions ?? []) conn.respond(request.rpcId, permissionCancelled);
    if (thread) {
      updateThread(id, (t) => settleEveryWait({ ...t, permissions: [] }, Date.now()));
      if (thread.permissions.length) linkWaitEnded(id);
    }
  } catch (error) {
    toast(errorText(error), "error");
    return;
  }
  void forceStopIfStuck(id, thread?.turn?.startedAt);
}

/**
 * Stops a thread's turn and waits for it to end, forcing it after a short wait: `cancel` for a
 * turn that obeys, closing the thread in the engine for one that does not. True once it runs nothing.
 */
export async function stopNow(id: string, waitMs = 3_000): Promise<boolean> {
  await cancel(id);
  const until = Date.now() + waitMs;
  while (isRunning(get().threads[id]) && Date.now() < until) await new Promise((r) => setTimeout(r, 100));
  if (isRunning(get().threads[id])) {
    await closeThread(id);
    updateThread(id, (t) => (isRunning(t) ? finishTurn(t, "cancelled", Date.now()) : t));
  }
  return !isRunning(get().threads[id]);
}

/** The stops waiting to be forced: one per thread, however many times Stop is pressed. */
const forcingStops = new Set<string>();

/**
 * `session/cancel` is a notification: nothing says it landed. A harness that ignores it, or a turn
 * wedged on a tool, would leave Stop doing nothing for good. When the turn is still the same one
 * after `FORCE_STOP_MS`, the thread is closed in the engine (which ends its harness) and the window
 * settles it as stopped and reads it again, so the thread can be used and closed (issue 3652).
 */
async function forceStopIfStuck(id: string, turnStartedAt: number | undefined): Promise<void> {
  if (forcingStops.has(id)) return;
  forcingStops.add(id);
  try {
    await new Promise((resolve) => setTimeout(resolve, FORCE_STOP_MS));
    const now = get();
    const thread = now.threads[id];
    // Ended, or a new turn began since: nothing is stuck.
    const sameTurn = isRunning(thread) && thread.turn?.startedAt === turnStartedAt;
    const engineStill = !isRunning(thread) && !!now.engineRunning[id];
    if (!sameTurn && !engineStill) return;
    toast("The turn did not stop, so it was ended.", "info");
    await closeThread(id);
    updateThread(id, (t) => (isRunning(t) ? finishTurn(t, "cancelled", Date.now()) : t));
    set((s) => {
      const { [id]: _gone, ...rest } = s.engineRunning;
      return _gone ? { engineRunning: rest } : {};
    });
    if (get().agent?.closeSession) void reloadThread(id);
  } finally {
    forcingStops.delete(id);
  }
}

/** Below this a prompt fits one frame of a paired computer's connection (1 MiB), with room to spare. */
const INLINE_PROMPT_CHARS = 900_000;

/**
 * A remote thread's images go ahead of its prompt, in chunks under the connection's 1 MiB frame
 * (state/attachmentTransfer.ts), and the prompt names each by the file the machine kept. A machine
 * on a RustCode from before the upload takes them inline, only when the prompt fits a frame.
 */
async function remotePromptImages(conn: AcpConnection, id: string, prompt: Json[]): Promise<Json[]> {
  if (!prompt.some((block) => obj(block)?.type === "image")) return prompt;
  try {
    return await uploadPromptImages(
      (method, params, timeout) => conn.request(method, params, timeout),
      prompt,
    );
  } catch (error) {
    if (!(error instanceof RpcError && error.isMethodNotFound)) throw error;
    if (JSON.stringify(prompt).length < INLINE_PROMPT_CHARS) return prompt;
    throw new Error(
      `${machineOfThread(id) ?? "That computer"} runs an older RustCode that cannot take an image this large from another computer. Update RustCode there, or send a smaller image.`,
    );
  }
}

/**
 * Sets a config option on a thread. `remember` (the default) mirrors a model or effort into the
 * harness's next new-thread choice and the model's own effort memory: the user picked it here.
 * A turn's own `configure_session`, and the apply after `session/new` (whose choice the picker
 * already noted), write their options with `remember: false`, so an agent never rewrites the
 * user's memory.
 */
export async function setConfigOption(
  id: string,
  configId: string,
  value: string | boolean,
  options: { remember?: boolean } = {},
): Promise<void> {
  // A remote thread's options are its machine's to list and set.
  const conn = connectionForThread(id);
  if (!conn) {
    toast("That computer is not connected.", "error");
    return;
  }
  const params: JsonObject =
    typeof value === "boolean"
      ? { sessionId: splitThreadKey(id).id, configId, type: "boolean", value }
      : { sessionId: splitThreadKey(id).id, configId, value };
  try {
    const result = (await conn.request(Method.sessionSetConfigOption, params, 15_000)) as JsonObject | null;
    if (result?.configOptions) {
      updateThread(id, (t) => ({ ...t, configOptions: parseConfigOptions(result.configOptions) }));
    }
    // A model or effort picked in a thread is also its harness's next new-thread choice; the
    // engine keeps it (docs/harnesses.md), the app only mirrors it, and the model keeps the
    // effort it was left on for the next time it is picked.
    const thread = get().threads[id];
    if (thread && options.remember !== false && (configId === "model" || configId === "effort")) {
      const current = (option: string) => thread.configOptions.find((o) => o.id === option)?.current ?? null;
      noteChoice(thread.harness?.id ?? "rustcode", { model: current("model"), effort: current("effort") });
      rememberModelEffort(thread.harness?.id ?? "rustcode", current("model"), current("effort"));
    }
  } catch (error) {
    toast(`Could not change ${configId}: ${errorText(error)}`, "error");
  }
}

// ---------------------------------------------------------------- permission loading

function loadProjectPermissionModes(): Record<string, PermissionMode> {
  const raw = storedJson<Record<string, unknown>>(STORAGE.projectPermissionModes, {});
  const modes: Record<string, PermissionMode> = {};
  for (const [path, mode] of Object.entries(raw ?? {})) {
    if (mode === "ask" || mode === "edits" || mode === "full") modes[projectKey(path)] = mode;
  }
  return modes;
}

/**
 * Each thread's own access. Builds before this pinned every listed thread to the default whenever
 * it changed (and at every start), so a thread stayed at Ask after the user chose Full access. Once,
 * the pins equal to the default are dropped, so those threads follow it again; one that differs may
 * be the user's own choice and stays, listed in Settings › Permissions to reset.
 */
function loadPermissionPins(): Record<string, PermissionMode> {
  const pins = storedJson<Record<string, PermissionMode>>(STORAGE.permissionModes, {});
  if (stored(STORAGE.permissionPinsThinned) === "1") return pins;
  const fallback = parsePermissionMode(stored(STORAGE.permissionMode));
  const kept = Object.fromEntries(Object.entries(pins ?? {}).filter(([, mode]) => mode !== fallback));
  store(STORAGE.permissionModes, JSON.stringify(kept));
  store(STORAGE.permissionPinsThinned, "1");
  return kept;
}

// ---------------------------------------------------------------- switching at a limit

// The harnesses a thread left on a limit, by thread: a row never sends it back to one of them, so two
// providers at their limits do not hand it to each other. Memory only: a restart starts afresh.
const leftOnLimit = new Map<string, Set<string>>();

interface LimitMove {
  rule: FallbackRule;
  fromName: string;
  to: Harness;
}

/**
 * Where a delegate that just stopped on a plan limit would go on instead of waiting (issue 3392), or
 * null: the setting is off, the stop is not a limit, the thread is not a delegate of this machine, or
 * no row of the table names an available harness for what it ran on.
 */
export function limitMoveFor(id: string, wait: LimitWait): LimitMove | null {
  const s = get();
  if (!s.limitFallback || (wait.reason ?? "limit") !== "limit") return null;
  if (threadIsRemote(id) || !isDelegated(id)) return null;
  const ran = harnessOf(s, id);
  const model = selectOption(s.threads[id]?.configOptions ?? [], "model")?.current;
  const rule = pickFallback(s.limitFallbackMap, { harness: ran, model }, leftOnLimit.get(id));
  if (!rule) return null;
  const listed = s.harnesses?.harnesses ?? [];
  const to = listed.find((h) => h.id === rule.to.harness);
  if (!to || unavailableReason(to)) return null;
  return { rule, fromName: listed.find((h) => h.id === ran)?.name ?? ran ?? "The harness", to };
}

/**
 * Move a limited delegate to the table's model and carry on there with the turn it was in, in the
 * same way `continueDueThreads` carries on after a reset: the links the stop settled are reopened and
 * answered by it. The thread says which model it moved to and why, and the engine records the move
 * in its activity. False when the move did not happen; the thread then waits for the reset as before.
 */
async function switchOnLimit(id: string, wait: LimitWait, move: LimitMove): Promise<boolean> {
  const { rule, fromName, to } = move;
  const from = harnessOf(get(), id);
  const account = rule.to.account ?? newThreadAccountOn(to, lastAccountFor(to.id), get().signedOutLogins);
  const moved = await switchThreadHarness(id, to.id, account, {
    model: rule.to.model,
    effort: rule.to.effort,
    reason: "limit",
    quiet: true,
  });
  if (!moved) return false;
  if (from) leftOnLimit.set(id, new Set([...(leftOnLimit.get(id) ?? []), from]));
  dropLimitWait(id);
  const options = get().threads[id]?.configOptions ?? [];
  const modelOption = selectOption(options, "model");
  const model =
    modelOption?.choices.find((c) => c.value === modelOption.current)?.name ?? modelOption?.current;
  const target = model ? `${to.name} · ${model}` : to.name;
  updateThread(id, (t) => appendNotice(t, movedNotice(fromName, target), "info"));
  toast(`“${threadName(id)}” hit its limit and moved to ${target}`, "info", {
    action: { label: "Open", run: () => selectThread(id) },
  });
  void runPrompt(id, switchText(fromName, target), undefined, {
    automatic: true,
    autoResume: true,
    resumeLinks: wait.links,
  });
  return true;
}

/** Test hook: reset to a clean slate. */
export function resetForTests() {
  // The wake tests follow what happens when a delegate settles, not the 60 s batch window; the
  // window's own rules are tested on the pure functions (state/orchestration.test.ts).
  useApp.setState({ wakeBatchSeconds: 0 });
  wholeLists.clear();
  // A machine's channel and its threads are process-global too: one test file's MockMachine must
  // not answer the next one's `openThread` (machinesThreads.test.ts › findings 1).
  resetMachinesForTests();
  resetFocusList();
  resetRemoteStatusReads();
  clearTimeout(slowTimer);
  resetContinueWaits();
  resetScheduledSends();
  clearPageCache();
  stopPlanPolling();
  resetWaits();
  resetLinkBook();
  resetThreadListGuards();
  flushingOutbox.clear();
  useOutbox.setState({ byHost: {} });
  forgetAutoCompact();
  // A lost turn's timers outlive a test file, and its links belong to that file's book.
  resetLostTurns();
  resetRunStatePoll();
  forcingStops.clear();
  engineWatch?.();
  engineWatch = undefined;
  forgetAskedModels();
  connection = null;
  transport = null;
  setShellTransport(null);
  loading.clear();
  opening.clear();
  autoRepaired.clear();
  set({
    engine: initialEngine,
    remote: null,
    pairing: { phase: "idle" },
    remoteByThread: {},
    remoteError: null,
    engineLog: [],
    sessions: [],
    sessionsCapped: false,
    threads: {},
    engineRunning: {},
    selectedId: null,
    projectCwd: "",
    defaultCwd: "",
    newThread: null,
    newThreadHost: LOCAL_HOST,
    remoteHarnesses: {},
    harnesses: null,
    baseCommands: [],
    harnessCommands: {},
    sessionCommands: {},
    drafts: {},
    extras: {},
    scheduled: {},
    settings: null,
    switcherOpen: false,
    onboardingOpen: false,
    toasts: [],
    toastsPaused: false,
    connectCode: null,
    limitWaits: {},
    internetWaits: {},
    continueAfterLimit: true,
    agent: undefined,
    pinned: [],
    sidebarOrder: {},
    archived: {},
    editing: {},
    confirm: null,
    panel: null,
    panelByThread: {},
    agents: {},
    planUsage: {},
    planCache: {},
    signedOutLogins: {},
    orchestration: false,
    orchestrationPending: false,
    trustDelegateAccounts: false,
    readAcrossProjects: false,
    links: { links: [], grants: {}, counters: {}, fanOuts: [] },
    grant: null,
    features: {},
    themes: null,
    themeActive: "rustcode",
    statusMarks: {},
    autoCompact: DEFAULT_AUTO_COMPACT,
    autoCompactOff: {},
    snoozed: {},
    snoozeAsk: null,
    defaultPermissionMode: "ask",
    projectPermissionModes: {},
    permissionModes: {},
    newThreadPermissionMode: null,
    view: { kind: "chat" },
    checkpoints: {},
    attempts: {},
    newWorktree: { on: false, branch: "", edited: false },
    pendingSetup: null,
    backgroundSetups: {},
  });
}

/** A command the engine marks as a report (`/status`, `/context`…), sent alone. */
function isReportCommand(id: string, text: string, extras: ComposeExtras): boolean {
  const name = extras.command?.name ?? text.trim().split(/\s/)[0];
  return (
    !extras.links.length &&
    !extras.attachments.length &&
    commandsFor(get(), id).some((c) => c.name === name && c.report)
  );
}

export function closeReport() {
  set({ report: null });
}

// ---------------------------------------------------------------- app wiring

// The running threads this window follows are written as they change (`state/continueThreads.ts`).
watchRunningThreads();

// What follows the app rather than a moment in it: the Dock badge, and a click on a notification
// opening its thread (state/notify.ts).
startNotifications({ get: () => useApp.getState(), subscribe: useApp.subscribe, open: selectThread });

/** A thread's name as a screen reader and the window's title say it (`state/announceThreads.ts`). */
export function spokenTitleOf(s: AppState, id: string): string {
  return spokenTitle(sessionById(s, id)?.title, s.threads[id]?.title);
}

// What a screen reader hears without reading the transcript (issue 3609): the window's title names
// the open thread and its status, and a thread's major moments are announced once each. The open
// thread says them all; one in the background only what needs the user, and not while it sleeps.
let moments = new Map<string, ThreadMoment>();
let windowTitleShown = "";
function announceThreads(s: AppState, prev?: AppState) {
  const selected = s.selectedId;
  // Review and Settled are the list's own words; the rest the thread's moment says more precisely.
  const listed = selected ? statusOf(s, selected) : "idle";
  const title = selected
    ? windowTitle(
        spokenTitleOf(s, selected),
        windowStatus(
          momentOf(s.threads[selected], s.failed[selected]),
          listed === "review" || listed === "done" ? STATUS_LABEL[listed] : undefined,
        ),
      )
    : windowTitle(undefined);
  if (title !== windowTitleShown) {
    windowTitleShown = title;
    applyWindowTitle(title);
  }
  if (prev && s.threads === prev.threads && s.failed === prev.failed) return;
  const next = new Map<string, ThreadMoment>();
  const quiet = warmingUp();
  for (const id of new Set([...Object.keys(s.threads), ...Object.keys(s.failed)])) {
    const moment = momentOf(s.threads[id], s.failed[id]);
    next.set(id, moment);
    if (quiet) continue;
    const active = id === selected;
    if (!active && id in s.snoozed) continue;
    const said = threadAnnouncement(spokenTitleOf(s, id), moments.get(id), moment, active);
    if (said) announce(said.text, { assertive: said.assertive });
  }
  moments = next;
}
mountAnnouncer();
announceThreads(useApp.getState());
useApp.subscribe(announceThreads);
