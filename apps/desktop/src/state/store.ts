import { forcingStops } from "./promptRun";
import { parseAppFont, parseTextSize } from "./settingsActions";
import {
  connectionForThread,
  machineOfThread,
  resetMachinesForTests,
  startMachineRouting,
  threadIsRemote,
} from "./threadMachines";

export * from "./settingsActions";

import { commandsFor, harnessOf, parsePanels } from "./viewActions";

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

import { startAgentPolling } from "./agentActions";
import { permissionModeOf, refreshGrants } from "./approvals";
import { forgetAskedModels, refreshHarnesses } from "./harnessActions";

export { agentHistory, hasAgents, refreshAgents, startAgentPolling } from "./agentActions";

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
  leaveSettledThreads,
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
  refreshAllHarnessModels,
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

import { prunePlans, refreshPlanCache, startPlanPolling, stopPlanPolling } from "./planUsage";

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

import { AcpConnection, type Inbound, type Json, type JsonObject, obj, RpcError, str } from "../acp/jsonrpc";
import type { Transport } from "../acp/transport";
import {
  byActivity,
  initializeParams,
  isReportUpdate,
  Method,
  parseAgentInfo,
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
import { parseOverrides } from "../lib/keys";
import { setShellTransport } from "../lib/shell";
import { ThemeMethod } from "../lib/themes";
import {
  applyWindowTitle,
  momentOf,
  spokenTitle,
  type ThreadMoment,
  threadAnnouncement,
  windowStatus,
  windowTitle,
} from "./announceThreads";
import { automaticCompactionText, DEFAULT_AUTO_COMPACT, parseAutoCompactSettings } from "./autoCompact";
import { forgetAutoCompact, noteCompactionCard, noteTyping, scheduleAutoCompact } from "./autoCompactRun";
import { createBatcher, frameQueue } from "./batch";
import { parseHarnessCommands, rememberCommands, serializeHarnessCommands } from "./commands";
import { type ComposeExtras, emptyExtras, hasExtras } from "./compose";
import {
  continueDueThreads,
  continueInternetThreads,
  interruptedThreads,
  resetContinueWaits,
  resumeInterrupted,
  syncWaitTimers,
  watchRunningThreads,
} from "./continueThreads";
import { DRAFTS_KEY, parseDrafts, persistable, withDraft } from "./drafts";
import {
  type EngineEvent,
  type EngineState,
  initialEngine,
  sessionsListed,
  startingText,
  transitionEngine,
} from "./engine";
import { selectOption } from "./harness";
import { sanitizeHeaderButtons } from "./headerButtons";
import {
  answerOrchestration,
  linkWaitStarted,
  loadBook,
  orchestrationCallDropped,
  reconcileLinks,
  resetLinkBook,
  resetWaits,
  restoreQueuedThreads,
  syncLinkBook,
  wakeTick,
} from "./host";
import type { InboxDismissals } from "./inboxDismissed";
import type { InternetWait } from "./internetWait";
import type { LimitWait } from "./limitResume";
import { lostTurnStatus, resetLostTurns } from "./lostTurns";
import { useOutbox } from "./machineOutbox";
import { flushingOutbox } from "./machineSync";
import { notifyThread, startNotifications, warmingUp } from "./notify";
import { CAPABILITY_VERSION, WAKE_BATCH_DEFAULT_S } from "./orchestration";
import { clearPageCache } from "./pageCache";
import { autoAnswer, type PermissionMode, parsePermissionMode } from "./permissions";
import { projectKey } from "./projectBook";
import { createdPull, deliverPullWatch, PULL_WATCH, subscribePullWatches } from "./pullWatch";
import { parseRecapHours } from "./recap";
import { resetRemoteStatusReads } from "./remoteAccess";
import { forgetRunDrift, reconcileRunState, resetRunStatePoll, startRunStatePoll } from "./runStatePoll";
import { parseScheduleBook, SCHEDULED_KEY } from "./schedule";
import { resetScheduledSends, restoreScheduled } from "./scheduledSends";
import type { SidebarOrder } from "./sidebarOrder";
import { resetStops, setStopSink } from "./stopTrace";
import {
  applyUpdate,
  endTurn,
  finishTurn,
  foldingBatch,
  isRunning,
  isWoken,
  localId,
  type ThreadState,
  TURN_PHASE_LABELS,
  waitingOn,
  wakeTurn,
  withChildActivity,
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
import { announceFinished, applySnooze, notifyTitle } from "./threadNotices";
import {
  autoRepaired,
  loading,
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
import { noticeOnlyHistory } from "./topLevel";
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
  untombstoneThread,
} from "./threadListGuards";
export {
  moveMachineBlockedReason,
  moveThreadToMachine,
  planMachineMoveDestination,
  rememberedMachineOrigin,
  rememberMachineOrigin,
  resetMoveMachineForTests,
  resolveTargetHarness,
  useMovingMachine,
} from "./threadMoveMachine";
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

/** The `initialize` sent a paired machine: its threads drawn with this app's Smart UI and viz,
 * and full fleet orchestration hosted by this app (with `engineOrchestration` as fallback while
 * this app is away). */
export const machineInitializeParams = (): JsonObject =>
  initializeParams(APP_VERSION, {
    smartUi: useApp.getState().smartUi,
    visualizations: 2,
    threads: 1,
    orchestration: useApp.getState().orchestration ? CAPABILITY_VERSION : undefined,
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
  trustDelegateAccounts: storedFlag(STORAGE.trustDelegateAccounts, SETTINGS_DEFAULTS.trustDelegateAccounts),
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
  settingsLanding: null,
  switcherOpen: false,
  modelPickerAsk: null,
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
/** A line of the app's own for Settings › Diagnostics, kept with the server's (the Stop trace). */
export function noteDiagnostic(line: string): void {
  logs.push(line);
}
setStopSink(noteDiagnostic);

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

function onTurnPhase(params: unknown) {
  const p = obj(params);
  const id = str(p?.sessionId);
  if (!id || !get().threads[id]) return;
  const phase = str(p?.phase);
  const age = p?.activityAgeMs;
  if (phase && typeof age === "number" && Object.hasOwn(TURN_PHASE_LABELS, phase)) {
    const label = TURN_PHASE_LABELS[phase as keyof typeof TURN_PHASE_LABELS];
    updateThread(id, (t) => ({ ...t, enginePhase: { label, lastActivityAt: Date.now() - age } }));
  } else if (get().threads[id]?.enginePhase) {
    updateThread(id, (t) => ({ ...t, enginePhase: undefined }));
  }
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

export async function handleInbound(
  message: Inbound,
  conn: AcpConnection | null = connection,
  host = LOCAL_HOST,
) {
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
            {
              ...(isRunning(t)
                ? finishTurn(t, "failed", Date.now(), "The server stopped during this turn.")
                : t),
              enginePhase: undefined,
            },
          ]),
        ),
      }));
      return;
    case "notification":
      if (message.method === Method.sessionUpdate) onSessionUpdate(message.params);
      else if (message.method === Method.sessionTurnPhase) onTurnPhase(message.params);
      else if (message.method === Method.sessionActivity) receiveActivity(message.params);
      else if (message.method === Method.sessionChildActivity) onChildActivity(message.params);
      else if (message.method === Method.threadCreated) threadCreated(message.params);
      else if (message.method === Method.threadDeleted) threadDeleted(message.params);
      else if (message.method === V2Method.orchestrationCancelled)
        orchestrationCallDropped(message.params, host);
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
        const result = await answerOrchestration(message.params, host);
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
    if (status === "done" && metadata?.selfSettled === true) leaveSettledThreads([sessionId]);
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
  resetStops();
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
export function isReportCommand(id: string, text: string, extras: ComposeExtras): boolean {
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

export {
  cancel,
  forcingStops,
  limitMoveFor,
  runPrompt,
  setConfigOption,
  stopNow,
  type TurnOutcome,
} from "./promptRun";
export {
  connectionFor,
  connectionForThread,
  harnessesOn,
  machineOfThread,
  resetMachinesForTests,
  startMachineRouting,
  threadIsRemote,
  threadOffline,
  useMachineOfThread,
  useThreadOffline,
  watchMachineConnection,
} from "./threadMachines";
