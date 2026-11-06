// App state (Zustand) and the engine controller that drives it over ACP.
// Components read slices with `useApp(selector)` and call the actions below.

import { create } from "zustand";
import { AcpConnection, type Inbound, type Json, type JsonObject, obj, RpcError, str } from "../acp/jsonrpc";
import {
  agentActive,
  harnessPlanKey,
  nextPlanRefresh,
  type PlanUsage,
  parseAgentList,
  parsePlans,
  parsePlanUsage,
  planHarness,
  planKey,
  type SubagentInfo,
  type UsagePlan,
  type UsageWindow,
} from "../acp/rustcode";
import type { Transport } from "../acp/transport";
import {
  type AgentInfo,
  byActivity,
  displayPath,
  fallbackHarnessSnapshot,
  type HarnessAccount,
  type HarnessLast,
  type HarnessSnapshot,
  initializeParams,
  isReportUpdate,
  Method,
  type PermissionOption,
  type PermissionRequest,
  parseAgentInfo,
  parseConfigOptions,
  parseHarnessLast,
  parseHarnessModels,
  parseHarnessSnapshot,
  parsePermissionRequest,
  parseSessionHarness,
  parseSessionList,
  parseSessionUpdate,
  permissionCancelled,
  permissionSelected,
  projectName,
  questionOptionsAnswer,
  questionTextAnswer,
  type SessionSummary,
  type SlashCommand,
} from "../acp/types";
import {
  type Checkpoint,
  type PatchFile,
  parseCheckpoints,
  parseTurnDiff,
  parseWorktree,
  parseWorktreeList,
  removeRefusal,
  statusInUpdate,
  type ThreadStatus,
  V2Method,
  type Worktree,
} from "../acp/v2";
import { CHAT_WIDTH_DEFAULT, type ChatWidth, parseChatWidth } from "../lib/chatWidth";
import {
  APP_COMMANDS,
  bindingOf,
  commandFor,
  commandHint,
  type KeyEventLike,
  parseOverrides,
  SHORTCUTS_KEY,
} from "../lib/keys";
import { dismissOverlays } from "../lib/overlays";
import { setShellTransport, shellCall } from "../lib/shell";
import { BUILTIN_THEMES, parseThemes, ThemeMethod, type ThemesSnapshot } from "../lib/themes";
import { titleFrom } from "../lib/title";
import { createBatcher, frameQueue } from "./batch";
import {
  type HarnessCommands,
  menuCommands,
  parseHarnessCommands,
  rememberCommands,
  serializeHarnessCommands,
} from "./commands";
import {
  type ComposeExtras,
  emptyExtras,
  hasExtras,
  promptBlocks,
  sentAttachments,
  splitImageMarkers,
} from "./compose";
import { DRAFTS_KEY, parseDrafts, persistable, withDraft } from "./drafts";
import { restoreChoice, selectOption } from "./harness";
import {
  DEFAULT_HEADER_BUTTONS,
  HEADER_BUTTON_IDS,
  type HeaderButtonId,
  sanitizeHeaderButtons,
} from "./headerButtons";
import {
  answerOrchestration,
  linkFinished,
  linkStarted,
  loadBook,
  orchestrationCallDropped,
  queuedLinkDropped,
  resetWaits,
  stopLinksFrom,
  syncLinkBook,
  threadTurnEnded,
  wakeDelivered,
  wakeTick,
} from "./host";
import { LIMIT_RESUME_TEXT, type LimitWait, limitReset, nextWait, waitDue, waitHolds } from "./limitResume";
import { newThreadAccountOn, type SignedOutLogins, signedOutFrom } from "./logins";
import {
  CAPABILITY_VERSION,
  forget,
  type GrantAsk,
  type LinkBook,
  MAX_ACTIVE_LIMIT,
  MAX_ACTIVE_LINKS,
  MIN_ACTIVE_LINKS,
} from "./orchestration";
import { clearPageCache } from "./pageCache";
import { autoAnswer, type PermissionMode, parsePermissionMode } from "./permissions";
import { NO_PAUSES, type PlanPauses, type PlansAsk, pausesAfter, plansAskFor, queueAsks } from "./plans";
import { nextQueued, STEER_REFUSAL, type SteerConditions, steerability } from "./queue";
import { beginSetup, dismissSetup, type PendingSetup, setupFailed, setupSucceeded } from "./setup";
import { dueSnoozes } from "./snooze";
import {
  appendSteer,
  applyUpdate,
  beginTurn,
  emptyThread,
  finishLoad,
  finishTurn,
  foldingBatch,
  localId,
  messagesAfter,
  questionItem,
  requeueSteers,
  type ThreadState,
  truncateBefore,
  userIndexOf,
  wakeTurn,
} from "./thread";
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

export type { Toast, ToastTone } from "./toasts";

export const APP_VERSION = "0.1.0";

export type SettingsSection =
  | "general"
  | "chat"
  | "agents"
  | "connections"
  | "capabilities"
  | "appearance"
  | "keyboard"
  | "usage"
  | "statistics"
  | "advanced"
  | "about";

/** Every Settings page, in the order the panel's nav lists them. */
export const SETTINGS_SECTIONS: readonly SettingsSection[] = [
  "general",
  "chat",
  "agents",
  "connections",
  "capabilities",
  "appearance",
  "keyboard",
  "usage",
  "statistics",
  "advanced",
  "about",
];

/**
 * What an older build's section id opens: Harnesses became Agents and Engine
 * became Advanced. An id no build knows opens General.
 */
const LEGACY_SECTIONS: Record<string, SettingsSection> = {
  harnesses: "agents",
  engine: "advanced",
};

/** A section id as the panel shows it, with the fallbacks a stored old one needs. */
export function settingsSection(id: string | null | undefined): SettingsSection | null {
  if (!id) return null;
  if ((SETTINGS_SECTIONS as readonly string[]).includes(id)) return id as SettingsSection;
  return LEGACY_SECTIONS[id] ?? "general";
}

export interface NewThreadChoice {
  harness: string;
  model: string | null;
  effort: string | null;
}

/**
 * A harness's plan windows, with where they came from. `_rustcode/usage/plans`
 * names the subscription; a thread's `_rustcode/usage/limits` only has numbers.
 */
export interface CachedPlan extends PlanUsage {
  /** The subscription as Settings › Usage names it ("Claude"), when it was read there. */
  name?: string;
  /** The harness it belongs to, and the account when it is not the default login. */
  harness?: string;
  account?: string | null;
  /** Its tier ("max"), when the provider named one. */
  plan?: string;
}

export interface AppState {
  status: "connecting" | "ready" | "closed";
  statusDetail?: string;
  transport?: Transport["name"];
  engineLog: string[];
  agent?: AgentInfo;
  sessions: SessionSummary[];
  sessionsLoaded: boolean;
  /** The session list stopped at the page cap, so some threads may be absent. */
  sessionsCapped: boolean;
  threads: Record<string, ThreadState>;
  /** Null shows the new-thread screen. */
  selectedId: string | null;
  /** The folder a new thread starts in. */
  /** The folder the next new thread starts in. */
  projectCwd: string;
  /** Where new threads start by default (Settings › General, ⌘⇧O). */
  defaultCwd: string;
  newThread: NewThreadChoice | null;
  /** The account a new thread on that harness starts on; null is its default login. */
  newThreadAccount: string | null;
  harnesses: HarnessSnapshot | null;
  /** The engine's own catalog (`_rustcode/commands`), so a new thread's drawer works before it starts. */
  baseCommands: SlashCommand[];
  /** Each harness's last advertised commands, by harness id (kept in app storage). */
  harnessCommands: HarnessCommands;
  /** The commands an open thread's session advertised, by thread id; absent falls back to the menu above. */
  sessionCommands: Record<string, SlashCommand[]>;
  drafts: Record<string, string>;
  /** Command token, link chips and attachments of each composer, by thread id (or "new"). */
  extras: Record<string, ComposeExtras>;
  settings: SettingsSection | null;
  switcherOpen: boolean;
  /** A report command's answer (`/status`, `/context`…), shown over the composer of its thread. */
  report: { threadId: string; text: string } | null;
  /** Bumped to open the composer's model picker (`/model`). */
  modelPickerAsk: number;
  onboardingOpen: boolean;
  sidebarHidden: boolean;
  /** Newest first, at most three (state/toasts.ts). */
  toasts: Toast[];
  /** The pointer, focus or the window is away: their clocks are stopped. */
  toastsPaused: boolean;
  /** The light/dark preference; a theme's own mode sets it when the theme is picked. */
  theme: "system" | "light" | "dark";
  /** The engine's themes (`_rustcode/themes`); null until listed, or when the engine has none. */
  themes: ThemesSnapshot | null;
  /** The theme picked, painted when its mode is the one shown (lib/themes.ts `resolveTheme`). */
  themeActive: string;
  /** Pinned threads, kept in app storage like the SwiftUI app's `pinnedSessions`. */
  pinned: string[];
  /** Archived threads and when, kept in app storage; they leave the list until shown. */
  archived: Record<string, number>;
  /**
   * Threads the user sent a message to themselves, and when. A delegated thread
   * here is the user's own and keeps its place in the sidebar (state/topLevel.ts).
   */
  messaged: Record<string, number>;
  /** A sent message being edited in the composer: sending rewinds the thread to it first. */
  editing: Record<string, { itemId: string; userIndex: number; original: string }>;
  /**
   * A queued message opened in the composer (⏎ saves it in the queue, never sends it). It carries
   * what the composer held before, which comes back when the edit ends either way.
   */
  queuedEdit: {
    threadId: string;
    queuedId: string;
    text: string;
    extras: ComposeExtras;
  } | null;
  /** Smart UI blocks run in the transcript (Settings › Chat). */
  smartUi: boolean;
  /** The engine learned a different Smart UI setting at initialize: a restart applies the new one. */
  smartUiPending: boolean;
  /** The workspace terminal under the chat (⌘J). */
  terminalOpen: boolean;
  /** The panel on the right of the chat. */
  panel: "details" | "changes" | "pulls" | "peek" | null;
  /** The thread shown beside this one in the panel's Thread tab. */
  peekId: string | null;
  /**
   * The thread whose panel the user closed with its own controls. A thread
   * opens with the panel by default, and this keeps that choice for the thread
   * on screen; opening the panel, or viewing another thread, forgets it.
   */
  panelClosedFor: string | null;
  /** Each thread's subagents, as `_rustcode/agents` last listed them. */
  agents: Record<string, SubagentInfo[]>;
  /** The plan's usage windows (`_rustcode/usage/limits`), by thread. */
  planUsage: Record<string, PlanUsage | null>;
  /**
   * Each harness's plan windows as last learnt, by harness id, from Settings'
   * `_rustcode/usage/plans` and from any thread's `_rustcode/usage/limits`.
   * The composer's meter reads it for a thread with no numbers of its own.
   */
  planCache: Record<string, CachedPlan>;
  /**
   * The logins the last plan read found signed out, by cache key
   * (`claude-code`, `claude-code#work`; see state/logins.ts). The harness
   * snapshot cannot say: Claude Code has no sign-in probe.
   */
  signedOutLogins: SignedOutLogins;
  /** An app-drawn confirmation (the webview has no native dialogs). */
  confirm: {
    title: string;
    message: string;
    action: string;
    destructive?: boolean;
    run: () => void;
    /** A second, less prominent action (e.g. "Files and conversation"). */
    alt?: { action: string; run: () => void };
  } | null;
  /** Threads can start and message other threads (Settings › Agents); the engine learns it at initialize. */
  orchestration: boolean;
  /** How many distinct threads one thread may run at once (Settings › Agents); a tree holds twice as many. */
  threadLimit: number;
  /** Agents may read threads of every project, not only the calling thread's (Settings › Agents). */
  readAcrossProjects: boolean;
  /** Threads that were running when the app quit or the engine restarted continue on their own. */
  resumeAfterRestart: boolean;
  /** A thread stopped by its plan's limit continues when the limit resets. */
  continueAfterLimit: boolean;
  /** The threads waiting on that, by thread id (state/limitResume.ts). */
  limitWaits: Record<string, LimitWait>;
  /** The interface in SF Pro Rounded (the default) or the standard system font. */
  roundedFont: boolean;
  /** A streaming reply's new words fade in as the model writes them. */
  animateStreaming: boolean;
  /** How wide the chat lane grows, transcript and composer alike (Settings › Chat). */
  chatWidth: ChatWidth;
  /** Which buttons the thread header shows, in order (Settings › Chat); anything absent is hidden. */
  headerButtons: HeaderButtonId[];
  /** Shortcuts the user changed, by command id (lib/keys.ts holds the defaults). */
  shortcuts: Record<string, string>;
  orchestrationPending: boolean;
  /** Links between threads, fan-outs and grants, kept in app storage. */
  links: LinkBook;
  /** A thread asking the user whether it may reach another one. */
  grant: (GrantAsk & { id: string }) | null;
  /** v2 engine methods, probed at initialize: false hides the feature. Undefined: not known yet. */
  features: {
    worktrees?: boolean;
    checkpoints?: boolean;
    status?: boolean;
    themes?: boolean;
    pins?: boolean;
  };
  /** Review / done marks kept by the app when the engine does not store them. */
  statusMarks: Record<string, ThreadStatus>;
  /** Threads out of the list until a time (state/snooze.ts). */
  snoozed: Record<string, number>;
  /** The thread the custom snooze dialog is open for. */
  snoozeAsk: string | null;
  /** What threads may do without asking (Settings › General › Default access)... */
  defaultPermissionMode: PermissionMode;
  /** ...each thread's own, once it was set from the composer's pill... */
  permissionModes: Record<string, PermissionMode>;
  /** ...and the next thread's, when the pill is used before one exists. */
  newThreadPermissionMode: PermissionMode | null;
  /** Threads whose turn completed while you were elsewhere, by when: Done in the sidebar until opened. */
  finished: Record<string, number>;
  /** Threads whose last turn failed while you looked elsewhere, until you open them. */
  failed: Record<string, number>;
  /** What the main column shows instead of the chat. */
  view:
    | { kind: "chat" }
    | { kind: "board" }
    | { kind: "review"; threadId: string }
    | { kind: "compare"; fanOut: string };
  /** Each thread's per-turn checkpoints (`_rustcode/checkpoints/list`). */
  checkpoints: Record<string, Checkpoint[]>;
  /** The new-thread composer's worktree choice. */
  newWorktree: { on: boolean; branch: string; edited: boolean };
  /**
   * A thread sent with a worktree before the engine has its session: the row and the chat show it
   * while `session/new` runs (state/setup.ts). In memory only, so a reload leaves no ghost row.
   */
  pendingSetup: PendingSetup | null;
}

/** A cached plan older than this is dropped on launch: its windows have moved on. */
export const PLAN_CACHE_MAX_AGE_MS = 24 * 60 * 60_000;

const STORAGE = {
  smartUi: "rustcode.smartUi",
  pinned: "rustcode.pinned",
  archived: "rustcode.archived",
  messaged: "rustcode.messaged",
  project: "rustcode.projectCwd",
  theme: "rustcode.theme",
  themeActive: "rustcode.themeActive",
  pinsMigrated: "rustcode.pinsMigrated",
  archiveMigrated: "rustcode.archiveMigrated",
  sidebar: "rustcode.sidebarHidden",
  orchestration: "rustcode.orchestration",
  threadLimit: "rustcode.threadLimit",
  readAcrossProjects: "rustcode.readAcrossProjects",
  resumeAfterRestart: "rustcode.resumeAfterRestart",
  continueAfterLimit: "rustcode.continueAfterLimit",
  limitWaits: "rustcode.limitWaits",
  /** The ids of the threads running a turn, kept while the engine is up. */
  runningThreads: "rustcode.runningThreads",
  roundedFont: "rustcode.roundedFont",
  animateStreaming: "rustcode.animateStreaming",
  chatWidth: "rustcode.chatWidth",
  headerButtons: "rustcode.headerButtons",
  statusMarks: "rustcode.statusMarks",
  snoozed: "rustcode.snoozed",
  permissionMode: "rustcode.permissionMode",
  permissionModes: "rustcode.permissionModes",
  /** Each harness's last new-thread choice, for an engine that does not remember it. */
  harnessChoices: "rustcode.harnessChoices",
  /** Each harness's last account picked for new threads, by harness id. */
  harnessAccountChoices: "rustcode.harnessAccountChoices",
  /** Each harness's last advertised slash commands, so a new thread's drawer is filled in. */
  harnessCommands: "rustcode.harnessCommands",
  /** Each harness's plan windows, so the composer's meter is there on launch. */
  planCache: "rustcode.planCache",
  /** Bindings the user changed, by command id; the table in lib/keys.ts holds the rest. */
  shortcuts: SHORTCUTS_KEY,
};

/**
 * What Settings' reset puts a value back to, and what a fresh install starts
 * with: the initial state below and every reset row read this one table, so
 * the two cannot drift apart. Rows with no default (a folder, an engine-chosen
 * model) offer no reset.
 */
export const SETTINGS_DEFAULTS = {
  theme: "system" as AppState["theme"],
  roundedFont: false,
  animateStreaming: true,
  chatWidth: CHAT_WIDTH_DEFAULT,
  headerButtons: DEFAULT_HEADER_BUTTONS,
  resumeAfterRestart: true,
  continueAfterLimit: true,
  orchestration: true,
  threadLimit: MAX_ACTIVE_LINKS,
  readAcrossProjects: true,
  smartUi: true,
  defaultPermissionMode: parsePermissionMode(null),
};

function stored(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

/** A boolean kept as "1"/"0", or the default when it was never set. */
function storedFlag(key: string, fallback: boolean): boolean {
  const raw = stored(key);
  return raw === null ? fallback : raw === "1";
}

/** A whole number kept as text, kept to the setting's own range. */
function storedThreadLimit(key: string, fallback: number): number {
  const raw = stored(key);
  const n = raw === null ? Number.NaN : Number.parseInt(raw, 10);
  if (!Number.isFinite(n)) return fallback;
  return Math.min(MAX_ACTIVE_LIMIT, Math.max(MIN_ACTIVE_LINKS, n));
}

export function store(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {
    // Storage can be unavailable (private window); the setting just does not persist.
  }
}

function storedJson<T>(key: string, fallback: T): T {
  try {
    const raw = stored(key);
    return raw ? (JSON.parse(raw) as T) : fallback;
  } catch {
    return fallback;
  }
}

// Engine lines are applied a frame at a time; see batch.ts.
const batcher = createBatcher<AppState>();

export const useApp = create<AppState>((_set, _get, api) => ({
  // Lets `batcher.batch` defer subscribers (adds no state).
  ...batcher.attach(api),
  pinned: storedJson<string[]>(STORAGE.pinned, []),
  archived: storedJson<Record<string, number>>(STORAGE.archived, {}),
  messaged: storedJson<Record<string, number>>(STORAGE.messaged, {}),
  editing: {},
  queuedEdit: null,
  confirm: null,
  // On unless turned off: an agent asks before it starts or messages a thread (Ask), so
  // delegating is there to use without first finding the setting.
  orchestration: storedFlag(STORAGE.orchestration, SETTINGS_DEFAULTS.orchestration),
  threadLimit: storedThreadLimit(STORAGE.threadLimit, SETTINGS_DEFAULTS.threadLimit),
  // On unless turned off: reading knows no project boundary, so an agent asked
  // about the user's work is not answered with a fragment of it.
  readAcrossProjects: storedFlag(STORAGE.readAcrossProjects, SETTINGS_DEFAULTS.readAcrossProjects),
  resumeAfterRestart: storedFlag(STORAGE.resumeAfterRestart, SETTINGS_DEFAULTS.resumeAfterRestart),
  continueAfterLimit: storedFlag(STORAGE.continueAfterLimit, SETTINGS_DEFAULTS.continueAfterLimit),
  limitWaits: storedJson<Record<string, LimitWait>>(STORAGE.limitWaits, {}),
  roundedFont: storedFlag(STORAGE.roundedFont, SETTINGS_DEFAULTS.roundedFont),
  animateStreaming: storedFlag(STORAGE.animateStreaming, SETTINGS_DEFAULTS.animateStreaming),
  chatWidth: parseChatWidth(stored(STORAGE.chatWidth)),
  headerButtons: sanitizeHeaderButtons(storedJson<unknown>(STORAGE.headerButtons, null)),
  shortcuts: parseOverrides(stored(STORAGE.shortcuts)),
  orchestrationPending: false,
  links: loadBook(),
  grant: null,
  features: {},
  statusMarks: storedJson<Record<string, ThreadStatus>>(STORAGE.statusMarks, {}),
  snoozed: storedJson<Record<string, number>>(STORAGE.snoozed, {}),
  snoozeAsk: null,
  defaultPermissionMode: parsePermissionMode(stored(STORAGE.permissionMode)),
  permissionModes: storedJson<Record<string, PermissionMode>>(STORAGE.permissionModes, {}),
  newThreadPermissionMode: null,
  finished: {},
  failed: {},
  view: { kind: "chat" },
  checkpoints: {},
  newWorktree: { on: false, branch: "", edited: false },
  pendingSetup: null,
  smartUi: storedFlag(STORAGE.smartUi, SETTINGS_DEFAULTS.smartUi),
  smartUiPending: false,
  terminalOpen: false,
  panel: null,
  peekId: null,
  panelClosedFor: null,
  agents: {},
  planUsage: {},
  planCache: prunePlans(storedJson<Record<string, CachedPlan>>(STORAGE.planCache, {})),
  signedOutLogins: {},
  status: "connecting",
  engineLog: [],
  sessions: [],
  sessionsLoaded: false,
  sessionsCapped: false,
  threads: {},
  selectedId: null,
  projectCwd: stored(STORAGE.project) ?? "",
  defaultCwd: stored(STORAGE.project) ?? "",
  newThread: null,
  newThreadAccount: null,
  harnesses: null,
  baseCommands: [],
  harnessCommands: parseHarnessCommands(stored(STORAGE.harnessCommands)),
  sessionCommands: {},
  drafts: parseDrafts(stored(DRAFTS_KEY)),
  extras: {},
  settings: null,
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
    return next === thread ? state : { threads: { ...state.threads, [id]: next } };
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

let connection: AcpConnection | null = null;
let transport: Transport | null = null;
/** Sessions whose `session/load` replay is arriving. */
const loading = new Set<string>();

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
  if (!connection) throw new Error("The engine is not connected.");
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
/** Engine stderr, kept for Settings › Advanced; appended once a frame, not per line. */
const logs = frameQueue<string>((lines) =>
  set((s) => ({ engineLog: [...s.engineLog, ...lines].slice(-400) })),
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
let bootProgress = 0;
let slowTimer: ReturnType<typeof setTimeout> | undefined;
let engineWatch: (() => void) | undefined;
/** The engine stopped since it was last ready: its next start is a restart. */
let crashed = false;

function bootStep(progress: number) {
  bootProgress = progress;
  updateToast(ENGINE_TOAST, { progress });
}

function onEngineStatus(s: AppState, prev?: AppState) {
  // Settings' answers name this engine's accounts and plans: none of them may
  // show for the next one (a restart can reach a different config).
  if (s.status !== "ready") clearPageCache();
  clearTimeout(slowTimer);
  if (s.status === "ready") {
    // Batched updates can go from closed straight to ready: this keys on `crashed` alone.
    if (crashed && prev) toast("The engine restarted", "success", { id: ENGINE_TOAST });
    else dismissToast(ENGINE_TOAST);
    crashed = false;
    return;
  }
  if (s.status === "closed") {
    crashed = true;
    toast(s.statusDetail ?? "The engine stopped.", "error", {
      id: ENGINE_TOAST,
      action: { label: "Restart", run: () => void restartEngine() },
    });
    return;
  }
  // Starting: an error toast from before goes at once; a slow start gets its own.
  if (get().toasts.some((t) => t.id === ENGINE_TOAST && t.tone === "error")) dismissToast(ENGINE_TOAST);
  slowTimer = setTimeout(() => {
    if (get().status !== "connecting") return;
    toast(crashed ? "Restarting engine…" : "Starting engine…", "info", {
      id: ENGINE_TOAST,
      persist: true,
      progress: bootProgress,
    });
  }, ENGINE_SLOW_MS);
}

function watchEngine() {
  if (engineWatch) return;
  engineWatch = useApp.subscribe((s, prev) => {
    if (s.status !== prev.status) onEngineStatus(s, prev);
  });
  onEngineStatus(get());
}

/** Start the transport once; every engine (re)start then gets a fresh ACP connection. */
export async function connect(next: Transport): Promise<void> {
  transport = next;
  setShellTransport(next);
  startAgentPolling();
  startPlanPolling();
  bootProgress = 0.15;
  set({ status: "connecting", transport: next.name, statusDetail: undefined });
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
  } catch (error) {
    set({ status: "closed", statusDetail: errorText(error) });
  }
}

async function handshake(): Promise<void> {
  connection?.close("The engine restarted.");
  const conn = new AcpConnection((line) => transport?.send(line));
  connection = conn;
  conn.onInbound((message) => {
    if (connection === conn) void handleInbound(message);
  });
  set({ status: "connecting", statusDetail: undefined });
  modelsAsked.clear();
  bootStep(0.5);
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
      }),
      20_000,
    );
    const agent = parseAgentInfo(result);
    // Read before the reset below marks every thread idle.
    const interrupted = interruptedThreads();
    set({
      agent,
      status: "ready",
      statusDetail: undefined,
      smartUiPending: false,
      orchestrationPending: false,
    });
    // Threads that were open lost their engine-side state with the old process.
    set((s) => ({
      threads: Object.fromEntries(
        Object.entries(s.threads).map(([id, t]) => [
          id,
          { ...t, loaded: false, running: false, permissions: [] },
        ]),
      ),
    }));
    await Promise.all([
      refreshSessions(),
      refreshHarnesses(),
      probeFeatures(conn),
      refreshThemes(conn),
      refreshCommands(conn),
    ]);
    // The composer's meter is ready before the first thread opens.
    void refreshPlanCache();
    if (connection === conn) await syncPinsAndSettled(conn);
    // The link book now lives in the data store; local storage is the first paint only. Once the
    // store's copy is in, a child result nobody was told about (a window closed with its wake still
    // queued) is due: the queue is memory, the book is not, so the wake is read back from it here.
    void syncLinkBook().then(() => wakeTick());
    const selected = get().selectedId;
    if (selected) void openThread(selected);
    if (connection === conn) void resumeInterrupted(interrupted);
    // Waits survive a restart, and one may have come due while the app was closed.
    if (connection === conn) {
      syncLimitTimer();
      void continueDueThreads();
    }
  } catch (error) {
    set({ status: "closed", statusDetail: errorText(error) });
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

// ---------------------------------------------------------------- themes

/** The project whose `.rustcode/themes` the engine lists too. */
function themeParams(): JsonObject {
  const s = get();
  const cwd = (s.selectedId ? s.threads[s.selectedId]?.cwd : undefined) ?? s.projectCwd;
  return cwd ? { cwd } : {};
}

/** Lists the engine's themes; an engine without them keeps the built-in ones. */
async function refreshThemes(conn: AcpConnection): Promise<void> {
  try {
    const themes = parseThemes(await conn.request(ThemeMethod.list, themeParams(), 10_000));
    if (connection !== conn) return;
    set((s) => ({ themes, features: { ...s.features, themes: true } }));
  } catch (error) {
    if (connection !== conn) return;
    const missing = error instanceof RpcError && error.isMethodNotFound;
    set((s) => ({ themes: null, features: { ...s.features, themes: missing ? false : s.features.themes } }));
  }
}

/** `_rustcode/themes/changed`: a theme file changed, or the CLI picked another theme. */
function onThemesChanged(params: JsonObject) {
  if (!Array.isArray(params.themes)) {
    if (connection) void refreshThemes(connection);
    return;
  }
  const themes = parseThemes(params);
  const changed = themes.active !== get().themes?.active;
  set({ themes, features: { ...get().features, themes: true } });
  // Picked elsewhere (the CLI, another window): follow it, mode and all.
  if (changed) adoptTheme(themes.active);
}

function adoptTheme(name: string) {
  const theme = (get().themes?.themes ?? BUILTIN_THEMES).find((t) => t.name === name);
  if (!theme) return;
  set({ themeActive: name, theme: theme.mode });
  store(STORAGE.themeActive, name);
  store(STORAGE.theme, theme.mode);
}

/** Picks a theme: it applies at once, and the engine saves it when it has themes. */
export async function setActiveTheme(name: string): Promise<void> {
  adoptTheme(name);
  // The theme changes under the pointer, and the picker shows the name: no toast to say so.
  if (!get().features.themes || !connection) return;
  try {
    const themes = parseThemes(await connection.request(ThemeMethod.set, { name, ...themeParams() }, 10_000));
    set({ themes });
  } catch (error) {
    toast(`Could not save the theme: ${errorText(error)}`, "error");
  }
}

/**
 * Makes a new theme file from `from` through the engine. Null when the
 * engine cannot (the UI then shows the CLI command instead).
 */
export async function createTheme(name: string, from: string): Promise<string | null> {
  if (!get().features.themes || !connection) return null;
  try {
    const result = obj(await connection.request(ThemeMethod.create, { name, from }, 10_000));
    if (connection) await refreshThemes(connection);
    return str(result?.path) ?? "";
  } catch (error) {
    if (error instanceof RpcError && error.isMethodNotFound) return null;
    throw error;
  }
}

// ---------------------------------------------------------------- pins and settled

/**
 * Pins live in the engine when it has `_rustcode/session/pin`: local pins
 * move there once. Archived threads (an older app's) become Done once,
 * since Settled is exactly the threads marked Done.
 */
async function syncPinsAndSettled(conn: AcpConnection): Promise<void> {
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
  if (known === undefined) return "Checking what the engine supports…";
  const what = {
    worktrees: "worktrees",
    checkpoints: "checkpoints",
    status: "thread status",
    themes: "themes",
    pins: "pins",
  }[feature];
  return `This engine has no ${what} yet (update RustCode).`;
}

export async function restartEngine(): Promise<void> {
  if (!transport) return;
  crashed = true;
  bootProgress = 0.15;
  set({ status: "connecting", statusDetail: "Restarting the engine…" });
  await transport.restart().catch((error) => set({ status: "closed", statusDetail: errorText(error) }));
}

/**
 * `_rustcode/session/activity`: a turn parked on background commands
 * (`waiting`), woken when they finish (`running`), and that woken turn ending
 * (`idle`). Only a woken turn is finished here; a prompt the app sent ends
 * with its own response.
 */
function onSessionActivity(params: unknown) {
  const p = obj(params);
  const id = str(p?.sessionId);
  const state = str(p?.state);
  if (!id || !get().threads[id]) return;
  const background = typeof p?.background === "number" ? p.background : 0;
  updateThread(id, (t) => {
    if (state === "running") return wakeTurn(t);
    const ended = t.running && t.woken ? finishTurn(t, "completed") : t;
    if (state === "waiting") return { ...ended, waiting: background || undefined };
    return { ...ended, waiting: undefined };
  });
}

async function handleInbound(message: Inbound) {
  switch (message.type) {
    case "closed":
      set((s) => ({
        status: "closed",
        statusDetail: message.reason,
        threads: Object.fromEntries(
          Object.entries(s.threads).map(([id, t]) => [
            id,
            t.running ? finishTurn(t, "failed", Date.now(), "The engine stopped during this turn.") : t,
          ]),
        ),
      }));
      return;
    case "notification":
      if (message.method === Method.sessionUpdate) onSessionUpdate(message.params);
      else if (message.method === Method.sessionActivity) onSessionActivity(message.params);
      else if (message.method === V2Method.orchestrationCancelled) orchestrationCallDropped(message.params);
      else if (message.method === ThemeMethod.changed) onThemesChanged(message.params);
      return;
    case "request":
      if (message.method === Method.requestPermission) {
        const parsed = parsePermissionRequest(message.id, message.params);
        if (!parsed) {
          connection?.respond(message.id, permissionCancelled);
          return;
        }
        // A thread this window has not opened (a child started elsewhere, or a thread after a
        // restart) is loaded first: the request is shown in its own chat and in the thread that
        // started it, and both answer it through the same card.
        if (!get().threads[parsed.sessionId]?.loaded) await openThread(parsed.sessionId);
        const request = { ...parsed, at: Date.now() };
        if (!get().threads[request.sessionId]) {
          connection?.respond(message.id, permissionCancelled);
          return;
        }
        const auto = autoAnswer(request, permissionModeOf(get(), request.sessionId));
        if (auto) {
          connection?.respond(message.id, permissionSelected(auto.id));
          return;
        }
        updateThread(request.sessionId, (t) => ({ ...t, permissions: [...t.permissions, request] }));
        if (request.sessionId !== get().selectedId) {
          toast(request.question ? "A thread has a question" : "A thread needs approval", "info");
        }
        // A child that now waits on the user: its parent hears about it once.
        wakeTick();
        return;
      }
      if (message.method === V2Method.orchestration) {
        const conn = connection;
        const result = await answerOrchestration(message.params);
        conn?.respond(message.id, result);
        return;
      }
      // Nothing else is advertised; answer so the engine never waits.
      connection?.respondError(message.id, -32601, `Method not found: ${message.method}`);
      return;
  }
}

function onSessionUpdate(params: JsonObject) {
  const sessionId = str(params.sessionId);
  if (!sessionId) return;
  const update = parseSessionUpdate(params);
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
    set((s) => ({ sessions: s.sessions.map((x) => (x.id === sessionId ? { ...x, status } : x)) }));
    if (status === "done" || status === "idle") clearStatusMark(sessionId);
  }
  if (update.kind === "title") {
    set((s) => ({
      sessions: s.sessions.map((x) => (x.id === sessionId ? { ...x, title: update.title } : x)),
    }));
  }
  updateThread(sessionId, (t) => applyUpdate(t, update));
}

// ---------------------------------------------------------------- sessions

export async function refreshSessions(): Promise<void> {
  const conn = connection;
  if (!conn || !get().agent?.listSessions) {
    set({ sessionsLoaded: true, sessionsCapped: false });
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
    set((s) => {
      // Threads started here that the engine does not list yet (it lists a session once it saved history).
      const listed = new Set(all.map((x) => x.id));
      const started = s.sessions.filter((x) => !listed.has(x.id) && s.threads[x.id]);
      const sessions = byActivity([...started, ...all]);
      return {
        sessions,
        sessionsLoaded: true,
        sessionsCapped: capped,
        projectCwd: s.projectCwd || sessions[0]?.cwd || "",
      };
    });
  } catch (error) {
    set({ sessionsLoaded: true });
    toast(`Could not list threads: ${errorText(error)}`, "error");
  }
}

/**
 * A turn ended: flag the thread unless you are looking at it. The row says Done (or Failed) until you open the thread.
 */
export function markFinished(id: string, now = Date.now(), failed = false) {
  const looking = get().selectedId === id && typeof document !== "undefined" && document.hasFocus();
  if (looking) return;
  set((s) =>
    failed
      ? { failed: { ...s.failed, [id]: now } }
      : { finished: { ...s.finished, [id]: now }, failed: without(s.failed, id) },
  );
}

/**
 * The user sent `id` a message themselves: a delegated thread is their own now
 * and keeps its place in the sidebar (state/topLevel.ts). Recorded once, and
 * only for a prompt an agent did not ask for (`runPrompt`).
 */
export function markMessaged(id: string) {
  if (get().messaged[id] !== undefined) return;
  const messaged = { ...get().messaged, [id]: Date.now() };
  set({ messaged });
  store(STORAGE.messaged, JSON.stringify(messaged));
}

/** Take a thread out of the list until `at`, or bring it back now with `null`. */
export function snoozeThread(id: string, at: number | null) {
  const { [id]: _, ...rest } = get().snoozed;
  const snoozed = at === null ? rest : { ...rest, [id]: at };
  set({ snoozed });
  store(STORAGE.snoozed, JSON.stringify(snoozed));
}

export function askCustomSnooze(id: string | null) {
  set({ snoozeAsk: id });
}

/** Bring back the threads whose time has come, each marked unread so it is noticed. */
export function wakeSnoozed(now = Date.now()) {
  for (const id of dueSnoozes(get().snoozed, now)) {
    snoozeThread(id, null);
    markUnread(id);
    const title = get().sessions.find((s) => s.id === id)?.title || "A thread";
    toast(`“${title}” is back`, "info");
  }
}

export function markUnread(id: string) {
  set((s) => ({ finished: { ...s.finished, [id]: Date.now() } }));
}

function without(map: Record<string, number>, id: string): Record<string, number> {
  if (!(id in map)) return map;
  const { [id]: _, ...rest } = map;
  return rest;
}

/** You saw it: opening the thread (or coming back to the window on it) settles the flag. */
export function clearFinished(id: string | null) {
  if (!id || !(id in get().finished || id in get().failed)) return;
  set((s) => ({ finished: without(s.finished, id), failed: without(s.failed, id) }));
}

if (typeof window !== "undefined") {
  window.addEventListener("focus", () => clearFinished(get().selectedId));
}

export function selectThread(id: string | null) {
  // The pending row is not a session: opening it shows the new thread it will become, and sends
  // nothing to the engine with its client id.
  if (id && id === get().pendingSetup?.id) id = null;
  clearFinished(id);
  dismissOverlays();
  set((s) => ({
    ...viewingThread(s, id),
    switcherOpen: false,
    settings: null,
    view: { kind: "chat" },
  }));
  // The account this thread runs on may never have been read: its meter has no
  // numbers until it is, and it appears without a visit to Settings › Usage.
  // The list's summary names the account before the thread itself is opened.
  void refreshPlanCache();
  if (id) void openThread(id);
}

export function startNewThread(cwd?: string) {
  const s = get();
  // The thread you were in, else the default folder; picking one here does not change the default.
  const projectCwd =
    cwd ?? (s.selectedId ? s.threads[s.selectedId]?.cwd : undefined) ?? (s.defaultCwd || s.projectCwd);
  dismissOverlays();
  set({
    selectedId: null,
    projectCwd,
    // The panel belongs to a thread: the new-thread screen has none, and a
    // thread picked next opens it again (unless it was closed there by hand).
    panel: null,
    panelClosedFor: null,
    // The Thread tab shows a thread beside this one; a new thread has none.
    peekId: null,
    settings: null,
    switcherOpen: false,
    newThread: null,
    newThreadPermissionMode: null,
    view: { kind: "chat" },
    newWorktree: { on: s.newWorktree.on, branch: "", edited: false },
  });
}

/** The folder of the new thread being written (⌘O there, the composer footer's folder menu). */
export function setProjectCwd(cwd: string) {
  set({ projectCwd: cwd });
}

/** Where new threads start by default (⌘⇧O, Settings › General); a new thread being written follows. */
export function setDefaultCwd(cwd: string) {
  set((s) => ({ defaultCwd: cwd, projectCwd: s.selectedId ? s.projectCwd : cwd }));
  store(STORAGE.project, cwd);
}

/**
 * ⌘O on an open thread: it works in another folder from now on. The engine
 * moves only a closed session, so the thread is closed, moved and loaded
 * again; a running turn has to stop first.
 */
export async function moveThread(id: string, cwd: string): Promise<boolean> {
  const thread = get().threads[id];
  if (thread?.running) {
    toast("Stop the running turn before moving this thread to another folder.");
    return false;
  }
  if (!get().agent?.sessionMove || !connection) {
    toast("This engine cannot move a thread to another folder. Update RustCode.", "error");
    return false;
  }
  if (thread && get().agent?.closeSession) await closeThread(id);
  try {
    await connection.request(Method.sessionMove, { sessionId: id, cwd }, 10_000);
  } catch (error) {
    toast(`Could not move the thread: ${errorText(error)}`, "error");
    if (thread) void openThread(id);
    return false;
  }
  set((s) => {
    const { [id]: _old, ...threads } = s.threads;
    return {
      threads,
      sessions: s.sessions.map((x) => (x.id === id ? { ...x, cwd, worktree: undefined } : x)),
    };
  });
  await openThread(id);
  toast(`Moved to ${projectName(cwd)}`, "success");
  return true;
}

/** Loads a thread's history (the replay) if it is not loaded yet. */
export async function openThread(id: string) {
  const conn = connection;
  const existing = get().threads[id];
  if (!conn || existing?.loaded || existing?.loading || loading.has(id)) return;
  const summary = get().sessions.find((x) => x.id === id);
  const cwd = existing?.cwd ?? summary?.cwd ?? get().projectCwd;
  loading.add(id);
  set((s) => ({
    threads: { ...s.threads, [id]: { ...emptyThread(id, cwd, summary?.title), loading: true } },
  }));
  try {
    const result = await conn.request(Method.sessionLoad, { sessionId: id, cwd, mcpServers: [] }, 60_000);
    updateThread(id, (t) => ({
      ...finishLoad(t),
      configOptions: parseConfigOptions((result as JsonObject | null)?.configOptions),
      harness: parseSessionHarness(result) ?? t.harness,
    }));
    const opened = get().threads[id];
    if (opened && lacksModels(opened)) void ensureHarnessModels(opened.harness!.id);
    void refreshPlanUsage(id);
    void refreshCheckpoints(id);
  } catch (error) {
    updateThread(id, (t) => ({ ...finishLoad(t), error: `Could not open this thread: ${errorText(error)}` }));
  } finally {
    loading.delete(id);
  }
}

/**
 * Repair Thread: the engine checks a closed thread the way opening it does and
 * puts right what it safely can (a harness record pointing at another folder).
 * The thread is then opened again; what is left (a folder that is gone) is said.
 */
export async function repairThread(id: string): Promise<void> {
  const conn = connection;
  if (!conn || !get().agent?.sessionRepair) {
    toast("This engine cannot repair threads. Update RustCode.", "error");
    return;
  }
  if (get().threads[id]?.running) {
    toast("Stop the running turn before repairing this thread.");
    return;
  }
  await closeThread(id);
  let answer: JsonObject;
  try {
    answer = ((await conn.request(Method.sessionRepair, { sessionId: id }, 10_000)) ?? {}) as JsonObject;
  } catch (error) {
    toast(`Could not repair the thread: ${errorText(error)}`, "error");
    return;
  }
  const list = (v: unknown) => (Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : []);
  const fixed = list(answer.fixed);
  const problems = list(answer.problems);
  const cwd = typeof answer.cwd === "string" ? answer.cwd : undefined;
  set((s) => {
    const { [id]: _old, ...threads } = s.threads;
    return {
      threads,
      sessions: cwd ? s.sessions.map((x) => (x.id === id ? { ...x, cwd } : x)) : s.sessions,
    };
  });
  if (problems.length) {
    toast(problems.join(" "), "error", {
      action: { label: "Move…", run: () => void import("./folders").then((f) => f.chooseFolder(false)) },
    });
  } else {
    toast(
      fixed.length ? `Repaired: ${fixed.join(" ")}` : "Nothing needed repair: the thread checks out.",
      "success",
    );
  }
  await openThread(id);
}

export async function closeThread(id: string) {
  try {
    await connection?.request(Method.sessionClose, { sessionId: id }, 10_000);
  } catch {
    // Closing is best effort.
  }
}

export interface NewSession {
  cwd: string;
  harness?: string;
  /** The harness account to run on; omitted is its default login. */
  account?: string | null;
  model?: string | null;
  effort?: string | null;
  /** Create a worktree first and run the thread in it (docs/orchestrator.md). */
  worktree?: { repo: string; branch?: string; title?: string };
  /** The thread an agent starts this one from (`state/host.ts` `newThread`). */
  parent?: string;
  title?: string;
  select?: boolean;
  /** The setup this session finishes (state/setup.ts): it is listed and the setup cleared together. */
  pendingId?: string;
}

/** `session/new` with the harness, model, effort and worktree in `_meta.rustcode`; the thread is added to the list. */
export async function createSession(options: NewSession): Promise<string> {
  const conn = engine();
  const s = get();
  const params: JsonObject = { cwd: options.cwd, mcpServers: [] };
  const meta: JsonObject = {};
  if (options.harness && !s.harnesses?.unsupported) {
    meta.harness = options.harness;
    if (options.account) meta.account = options.account;
    if (options.model) meta.model = options.model;
    if (options.effort) meta.effort = options.effort;
  }
  if (options.worktree) {
    const w: JsonObject = { repo: options.worktree.repo };
    if (options.worktree.branch) w.branch = options.worktree.branch;
    if (options.worktree.title) w.title = options.worktree.title;
    meta.worktree = w;
  }
  // The engine keeps this in the thread's own metadata, so pruning the link
  // book does not make the child a top-level thread (`state/topLevel.ts`).
  if (options.parent) meta.parent = options.parent;
  if (Object.keys(meta).length) params._meta = { rustcode: meta };
  const result = (await conn.request(Method.sessionNew, params, 120_000)) as JsonObject;
  const id = str(result.sessionId);
  if (!id) throw new Error("The engine did not return a session id.");
  const worktree: Worktree | undefined = parseWorktree(
    ((result._meta as JsonObject | undefined)?.rustcode as JsonObject | undefined)?.worktree,
  );
  const cwd = worktree?.path ?? options.cwd;
  const thread: ThreadState = {
    ...emptyThread(id, cwd, options.title),
    loaded: true,
    configOptions: parseConfigOptions(result.configOptions),
    harness: parseSessionHarness(result),
    worktree,
  };
  const summary: SessionSummary = {
    id,
    cwd,
    updatedAt: Date.now(),
    harness: thread.harness?.id ?? options.harness,
    account: thread.harness?.account ?? options.account ?? undefined,
    worktree,
    title: options.title,
    // A thread an agent started is nested from its first frame: without the parent here the
    // sidebar draws it as a row of its own until the engine lists it with the same record, then
    // moves it under its parent (`state/topLevel.ts`).
    parent: options.parent ? { id: options.parent } : undefined,
  };
  // A pending setup keeps its row and its message until the thread is listed below, so nothing it
  // shows goes away and comes back between the session and its model. Any other session is listed
  // at once.
  const pending = !!options.pendingId && get().pendingSetup?.id === options.pendingId;
  set((st) => ({
    threads: { ...st.threads, [id]: thread },
    ...(pending
      ? {}
      : {
          ...(options.select ? { ...viewingThread(st, id), newThread: null } : {}),
          sessions: [summary, ...st.sessions],
          pendingSetup: setupSucceeded(st.pendingSetup, options.pendingId),
        }),
  }));
  // Saved, not only shown: otherwise the thread's first message replaces it at the next list.
  // A pending setup lets its rename run beside it: the message starts the moment the thread is listed.
  if (options.title) {
    const saved = renameThread(id, options.title);
    if (!pending) await saved;
  }
  // A model or effort picked before the thread existed applies now, when the thread did not take it
  // from `session/new` (an older engine, or a harness that ignores it there).
  const current = (configId: string) => thread.configOptions.find((o) => o.id === configId)?.current;
  const has = (configId: string) => thread.configOptions.some((o) => o.id === configId);
  if (options.model && current("model") !== options.model && has("model")) {
    await setConfigOption(id, "model", options.model);
  }
  if (options.effort && current("effort") !== options.effort && has("effort")) {
    await setConfigOption(id, "effort", options.effort);
  }
  if (pending) {
    // Listed now, with the setup gone in the same update: the row the message was sent to becomes
    // the thread, in place, and the chat goes on from the message it already shows.
    set((st) => ({
      sessions: [summary, ...st.sessions],
      ...(options.select && !st.selectedId ? { ...viewingThread(st, id), newThread: null } : {}),
      pendingSetup: setupSucceeded(st.pendingSetup, options.pendingId),
    }));
  }
  return id;
}

/**
 * A restored external thread whose harness has not started again offers only its own model (and
 * maybe no effort) until the harness lists its models: ask for them.
 */
export function lacksModels(thread: ThreadState): boolean {
  const id = thread.harness?.id;
  if (!id || id === "rustcode") return false;
  const model = thread.configOptions.find((o) => o.id === "model" && o.type === "select");
  const effort = thread.configOptions.some((o) => o.id === "effort");
  return (model?.choices.length ?? 0) <= 1 || (!effort && !!thread.harness?.capabilities.effort);
}

/** The new-thread choice: the picked one, else the default harness with its remembered model and effort. */
export function newThreadChoice(s: Pick<AppState, "newThread" | "harnesses">): NewThreadChoice | null {
  if (s.newThread) return s.newThread;
  if (!s.harnesses) return null;
  return restoreChoice(s.harnesses, s.harnesses.default.harness, appChoices(s.harnesses));
}

/**
 * Creates a thread with the new-thread choice (or the default harness) and worktree toggle, and
 * selects it. A worktree takes a while, so the thread appears while `session/new` runs; its message
 * comes back to the composer if the worktree or the harness cannot start.
 */
async function createThread(firstMessage = ""): Promise<string | null> {
  const s = get();
  const cwd = s.projectCwd;
  if (!cwd) throw new Error("Choose a project folder for this thread first.");
  const choice = newThreadChoice(s);
  const harness = s.harnesses?.harnesses.find((h) => h.id === harnessOf(s));
  const wt = s.newWorktree;
  const worktree =
    wt.on && s.features.worktrees
      ? { repo: cwd, branch: wt.branch.trim() || undefined, title: titleFrom(firstMessage, 80) }
      : undefined;
  const options = {
    cwd,
    harness: choice?.harness,
    // The account shown for the new thread: a signed-out default login is not one.
    account: newThreadAccountOn(harness, s.newThreadAccount, s.signedOutLogins),
    model: choice?.model,
    effort: choice?.effort,
    worktree,
    select: true,
  };
  let id: string;
  if (!worktree) {
    id = await createSession(options);
  } else {
    // The user sent a message: the row and the chat show it while the worktree is made. A second
    // send finds this one and starts nothing; only a failed setup can be sent again (the retry).
    const setup = beginSetup(get().pendingSetup, {
      id: localId("setup"),
      text: firstMessage,
      title: titleFrom(firstMessage, 80) || "New thread",
      cwd,
      worktree,
    });
    if (!setup) {
      const failed = get().pendingSetup?.failure;
      if (failed) toast(failed.text, "error");
      else toast("Setting up the worktree: the thread appears when it is ready.");
      return null;
    }
    set({ pendingSetup: setup });
    try {
      // The title the row already shows is the title the thread starts with, so it does not blink
      // back to "New thread" when the row becomes real.
      id = await createSession({ ...options, pendingId: setup.id, title: setup.title });
    } catch (error) {
      const text = errorText(error);
      // The setup stays, failed, with the message it holds; the composer gets the text back.
      set((st) =>
        st.pendingSetup?.id === setup.id ? { pendingSetup: setupFailed(st.pendingSetup, text) } : {},
      );
      return null;
    }
  }
  // The access level chosen on the new-thread composer belongs to the thread it started.
  const pendingMode = get().newThreadPermissionMode;
  if (pendingMode) {
    setPermissionMode(id, pendingMode);
    set({ newThreadPermissionMode: null });
  }
  set({ newWorktree: { on: wt.on, branch: "", edited: false } });
  return id;
}

/** Gives up a failed setup: its row and its error leave, and the new-thread screen stays. */
export function dismissPendingSetup() {
  set({ pendingSetup: dismissSetup() });
}

// ---------------------------------------------------------------- thread actions

export function askConfirm(confirm: NonNullable<AppState["confirm"]>) {
  set({ confirm });
}

export function closeConfirm() {
  set({ confirm: null });
}

/** Whether the thread can be taken back to one of its messages. */
export function canRewind(thread: ThreadState): boolean {
  return !!get().agent?.rewind && (thread.harness?.capabilities.rewind ?? true) && !thread.running;
}

async function rewindTo(id: string, itemId: string, text: string): Promise<boolean> {
  const thread = get().threads[id];
  if (!thread) return false;
  const userIndex = userIndexOf(thread, itemId);
  if (userIndex < 0) return false;
  try {
    await engine().request(Method.rewind, { sessionId: id, userIndex, text }, 10_000);
  } catch (error) {
    toast(`Could not rewind: ${errorText(error)}`, "error");
    return false;
  }
  updateThread(id, (t) => truncateBefore(t, itemId));
  return true;
}

/** Retry: the thread goes back to just before the message and sends it again (after a confirm when later messages go too). */
export function retryMessage(id: string, itemId: string) {
  const thread = get().threads[id];
  const item = thread?.items.find((i) => i.id === itemId);
  if (!thread || item?.kind !== "user") return;
  const go = async () => {
    if (canRewind(thread)) {
      if (await rewindTo(id, itemId, item.text)) void runPrompt(id, item.text);
    } else void submit(item.text);
  };
  const later = messagesAfter(thread, itemId);
  if (later > 0 && canRewind(thread)) {
    askConfirm({
      title: "Retry this message?",
      message:
        later === 1
          ? "The message after it and its reply will be removed."
          : `The ${later} messages after it and their replies will be removed.`,
      action: "Retry",
      destructive: true,
      run: () => void go(),
    });
  } else void go();
}

/** Edit: the message goes back to the composer; sending it rewinds the thread to it first. */
export function editMessage(id: string, itemId: string) {
  const thread = get().threads[id];
  const item = thread?.items.find((i) => i.id === itemId);
  if (!thread || item?.kind !== "user") return;
  const text = splitImageMarkers(item.text).text;
  set((s) => ({
    drafts: { ...s.drafts, [id]: text },
    editing: canRewind(thread)
      ? { ...s.editing, [id]: { itemId, userIndex: userIndexOf(thread, itemId), original: item.text } }
      : s.editing,
  }));
}

export function cancelEdit(key: string) {
  set((s) => {
    const { [key]: _gone, ...editing } = s.editing;
    return { editing };
  });
}

/**
 * Pin or unpin threads: the app's list first, so the sidebar answers at once, then the engine, one
 * request each. A failure puts back only the threads the engine refused, and says so once for the
 * whole selection rather than once per thread (ui/Sidebar.tsx acts on a multi-selection).
 */
export function setPinned(ids: string[], on: boolean) {
  const before = get().pinned;
  const wanted = new Set(ids);
  const next = on
    ? [...ids.filter((id) => !before.includes(id)), ...before]
    : before.filter((id) => !wanted.has(id));
  set({ pinned: next });
  store(STORAGE.pinned, JSON.stringify(next));
  const many = ids.length > 1;
  if (!get().features.pins || !connection) return;
  const link = connection;
  void Promise.all(
    ids.map((id) =>
      link.request(V2Method.sessionPin, { session_id: id, pinned: on }, 10_000).then(
        () => undefined,
        (error: unknown) => (error instanceof RpcError && error.isMethodNotFound ? undefined : { id, error }),
      ),
    ),
  ).then((results) => {
    const failed = results.filter((r): r is { id: string; error: unknown } => !!r);
    if (!failed.length) return;
    const refused = new Set(failed.map((f) => f.id));
    // Unpinning: keep what was pinned before, less the ones the engine took. Pinning: drop the
    // ones it refused and leave the rest.
    const pinned = on
      ? get().pinned.filter((id) => !refused.has(id))
      : before.filter((id) => !wanted.has(id) || refused.has(id));
    set({ pinned });
    store(STORAGE.pinned, JSON.stringify(pinned));
    const first = errorText(failed[0]!.error);
    toast(
      many
        ? `Could not ${on ? "pin" : "unpin"} ${failed.length} of ${ids.length} threads: ${first}`
        : `Could not ${on ? "pin" : "unpin"} the thread: ${first}`,
      "error",
    );
  });
}

export function togglePin(id: string) {
  setPinned([id], !get().pinned.includes(id));
}

export function setArchived(id: string, on: boolean) {
  const archived = { ...get().archived };
  if (on) {
    if (get().threads[id]?.running) {
      toast("Stop the running turn before archiving this thread.", "info");
      return;
    }
    archived[id] = Date.now();
  } else delete archived[id];
  set({ archived });
  store(STORAGE.archived, JSON.stringify(archived));

  if (on && get().selectedId === id) startNewThread();
}

export async function renameThread(id: string, title: string): Promise<boolean> {
  try {
    const saved = await shellCall<string>("thread_rename", { id, title });
    set((s) => ({ sessions: s.sessions.map((x) => (x.id === id ? { ...x, title: saved } : x)) }));
    updateThread(id, (t) => ({ ...t, title: saved }));
    return true;
  } catch (error) {
    toast(`Could not rename the thread: ${errorText(error)}`, "error");
    return false;
  }
}

/** Move a thread's folder to the Trash, closing it in the engine first, as the SwiftUI app does. */
export async function trashThread(id: string): Promise<boolean> {
  if (get().threads[id]?.running) {
    toast("Stop the running turn before moving this thread to the Trash.", "info");
    return false;
  }
  if (get().threads[id] && get().agent?.closeSession) await closeThread(id);
  try {
    await shellCall("thread_trash", { id });
  } catch (error) {
    toast(errorText(error), "error");
    return false;
  }
  const { [id]: _gone, ...threads } = get().threads;
  const archived = { ...get().archived };
  delete archived[id];
  const pinned = get().pinned.filter((x) => x !== id);
  set((s) => {
    const peeking = s.peekId === id;
    return {
      threads,
      archived,
      pinned,
      sessions: s.sessions.filter((x) => x.id !== id),
      links: forget(s.links, id),
      // A thread in the panel's Thread tab that no longer exists is not shown,
      // and the panel does not stay open on the tab it was in.
      peekId: peeking ? null : s.peekId,
      ...(peeking && s.panel === "peek" ? { panel: null } : {}),
    };
  });
  // Links into the trashed thread failed: their parents may now have something to hear.
  wakeTick();
  store(STORAGE.archived, JSON.stringify(archived));
  store(STORAGE.pinned, JSON.stringify(pinned));

  if (get().selectedId === id) startNewThread();
  return true;
}

// ---------------------------------------------------------------- turns

export function setDraft(key: string, text: string) {
  const next = withDraft(get().drafts, key, text);
  if (next !== get().drafts) set({ drafts: next });
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
}

/** The harness a thread runs on, or a new thread would start on. */
export function threadHarness(thread?: ThreadState) {
  const s = get();
  const id = thread
    ? (thread.harness?.id ?? "rustcode")
    : (s.newThread?.harness ?? s.harnesses?.default.harness);
  return s.harnesses?.harnesses.find((h) => h.id === id);
}

/** Whether the thread's harness takes image blocks. */
export function acceptsImages(thread?: ThreadState): boolean {
  const caps = thread?.harness?.capabilities ?? threadHarness(thread)?.capabilities;
  if (caps && !(threadHarness(thread)?.builtin ?? false)) return caps.images;
  return get().agent?.promptImages ?? true;
}

/** ⏎: send, or queue behind a running turn. False when no thread could start (the draft stays). */
/**
 * Commands the app answers itself, by the action the engine advertises with
 * each one (`_meta.rustcode.action`): nothing goes to the engine as a prompt.
 * Returns whether the command was one of them.
 */
export function runCommandAction(text: string, from: string | null): boolean {
  const s = get();
  const command = commandsFor(s, from).find((c) => c.name === text);
  if (!command || command.action === "execute") return false;
  const thread = from ? s.threads[from] : undefined;
  switch (command.action) {
    case "new_session": {
      // A fresh thread on the same project, harness, model and effort, selected and empty; in a
      // new thread it keeps the folder and the choice already made there.
      const choice = newThreadChoice(s);
      const model = thread ? selectOption(thread.configOptions, "model")?.current : choice?.model;
      const effort = thread ? selectOption(thread.configOptions, "effort")?.current : choice?.effort;
      const harness =
        thread?.harness?.id ?? (from ? s.sessions.find((x) => x.id === from)?.harness : choice?.harness);
      startNewThread(thread?.cwd ?? s.projectCwd);
      if (harness) setNewThreadChoice({ harness, model: model ?? null, effort: effort ?? null });
      return true;
    }
    case "model_picker":
      set((s) => ({ modelPickerAsk: s.modelPickerAsk + 1 }));
      return true;
    case "session_picker":
      setSwitcher(true);
      return true;
    case "copy_response": {
      const reply = [...(thread?.items ?? [])].reverse().find((i) => i.kind === "assistant");
      if (reply?.kind === "assistant" && reply.text.trim()) {
        void navigator.clipboard.writeText(reply.text).then(
          () => void toast("Copied the last reply", "success"),
          (error) => void toast(`Could not copy: ${errorText(error)}`, "error"),
        );
      } else toast("There is no reply to copy yet");
      return true;
    }
    default:
      return false;
  }
}

export async function submit(text: string, extras: ComposeExtras = emptyExtras): Promise<boolean> {
  // A worktree still setting up owns the new-thread screen: sending again must not make a second.
  const trimmed = text.trim();
  if (!trimmed && !hasExtras(extras)) return false;
  // A command the app answers itself, typed alone or left as a bare token.
  const bare = !hasExtras(extras)
    ? trimmed
    : !trimmed && extras.command && !extras.links.length && !extras.attachments.length
      ? extras.command.name
      : null;
  if (bare && runCommandAction(bare, get().selectedId)) return true;
  let id = get().selectedId;
  try {
    if (!id) {
      const started = await createThread(trimmed);
      if (!started) return false;
      id = started;
    }
  } catch (error) {
    toast(`Could not start the thread: ${errorText(error)}`, "error");
    return false;
  }
  const thread = get().threads[id];
  if (!thread) return false;
  if (trimmed === "/cancel" && !hasExtras(extras)) {
    await cancel(id);
    return true;
  }
  const edit = get().editing[id];
  if (edit && !thread.running) {
    cancelEdit(id);
    const later = messagesAfter(thread, edit.itemId);
    const go = async () => {
      if (await rewindTo(id!, edit.itemId, edit.original)) void runPrompt(id!, trimmed, extras);
    };
    if (later > 0) {
      askConfirm({
        title: "Send the edited message?",
        message: `The thread goes back to the message you edited; ${later === 1 ? "the message after it and its reply" : `the ${later} messages after it and their replies`} will be removed.`,
        action: "Send",
        destructive: true,
        run: () => void go(),
      });
    } else void go();
    return true;
  }
  if (thread.running) {
    updateThread(id, (t) => ({
      ...t,
      queue: [
        ...t.queue,
        { id: localId("queued"), text: trimmed, extras: hasExtras(extras) ? extras : undefined },
      ],
    }));
    return true;
  }
  if (trimmed === "/fork" && !hasExtras(extras)) {
    void forkThread(id);
    return true;
  }
  // Messages a stopped turn held keep their place: a new one goes behind them and Resumes the queue,
  // so nothing the user wrote is skipped or reordered.
  if (thread.queue.length) {
    updateThread(id, (t) => ({
      ...t,
      queue: [
        ...t.queue,
        { id: localId("queued"), text: trimmed, extras: hasExtras(extras) ? extras : undefined },
      ],
    }));
    void resumeQueue(id);
    return true;
  }
  void runPrompt(id, trimmed, extras);
  return true;
}

/**
 * `/fork` copies the chat into a new thread and leaves this one open (the
 * engine's rule). The reply does not name the new thread, so the list is
 * compared before and after, and a toast offers to open it.
 */
export async function forkThread(id: string): Promise<void> {
  const before = new Set(get().sessions.map((x) => x.id));
  await runPrompt(id, "/fork");
  await refreshSessions();
  const fork = get().sessions.find((x) => !before.has(x.id));
  if (!fork) return;
  toast(`Forked into "${fork.title || "a new thread"}"`, "success", {
    action: { label: "Open", run: () => selectThread(fork.id) },
  });
}

/**
 * ⌘⏎: hand the running turn a message to read after its current step, else
 * send. It follows `steerability`, the one rule the queued row's Send now
 * uses too: plain text steers, pictures steer where the harness takes them
 * mid-turn, and a command, links or a file queue whole for the next turn.
 */
export async function steerOrSubmit(text: string, extras: ComposeExtras = emptyExtras): Promise<boolean> {
  const id = get().selectedId;
  const thread = id ? get().threads[id] : undefined;
  const trimmed = text.trim();
  // ⌘⏎ always tries: a harness learns it can steer when its first thread starts, so a thread
  // started before that still reads "none" though the engine can steer it now.
  if (!id || !thread?.running || !get().agent?.steer) return submit(text, extras);
  // Nothing typed and nothing attached: take what waits first, by the row's own rule.
  if (!trimmed && !hasExtras(extras)) {
    const next = thread.queue[0];
    if (next) return sendQueuedNow(id, next.id);
    return submit(text, extras);
  }
  // ⌘⏎ reads what the message holds, not what the thread says about steering: a harness learns it
  // can steer when its first thread starts, so a thread started before that still reads "none"
  // though the engine can steer it now. The engine's answer decides. Pictures keep the harness
  // check, because the built-in engine refuses a steer that carries an image block and `images` is
  // the agent's own capability; a command, a link or a file waits either way.
  const verdict = steerability(extras, { ...steerConditions(thread), live: true });
  if (verdict.ok && (await steer(id, trimmed, extras))) return true;
  if (hasExtras(extras)) {
    const sent = await submit(text, extras);
    // It waits for the next turn with the reason the row's arrow would give.
    if (sent) toast(verdict.ok ? "Queued: the turn can't take it now" : STEER_REFUSAL[verdict.reason]);
    return sent;
  }
  updateThread(id, (t) => ({ ...t, queue: [...t.queue, { id: localId("queued"), text: trimmed }] }));
  // The real harness decides the words: a live one refused this time, a non-live one cannot.
  toast(
    canSteer(thread)
      ? "Queued: the turn is waiting for you"
      : `Queued: ${threadHarness(thread)?.name ?? "this harness"} can’t take a message mid-turn`,
  );
  return true;
}

export function canSteer(thread: ThreadState): boolean {
  if (!get().agent?.steer) return false;
  return !thread.harness || thread.harness.capabilities.steer === "live";
}

/**
 * The harness-level half of the steer rule (`canSteer`, `acceptsImages`, the built-in harness), read
 * once for ⌘⏎, Send now and the queued row. `ui/Transcript.tsx` passes the same three down to a row,
 * so the row never decides anything the store does not.
 */
export function steerConditions(thread?: ThreadState): SteerConditions {
  return {
    live: !!thread && canSteer(thread),
    images: acceptsImages(thread),
    builtin: threadHarness(thread)?.builtin ?? true,
  };
}

async function steer(id: string, text: string, extras?: ComposeExtras): Promise<boolean> {
  const thread = get().threads[id];
  const conditions = steerConditions(thread);
  const prompt = extras?.attachments.length
    ? promptBlocks(text, extras, {
        images: conditions.images,
        builtin: conditions.builtin,
        cwd: thread?.cwd ?? "",
      })
    : undefined;
  try {
    const result = (await engine().request(
      Method.steer,
      prompt ? { sessionId: id, text, prompt } : { sessionId: id, text },
      10_000,
    )) as JsonObject | null;
    if (result?.accepted !== true) return false;
    updateThread(id, (t) => appendSteer(t, text, Date.now(), extras && sentAttachments(extras)));
    // The steered row in the transcript says it was read after the current step.
    return true;
  } catch {
    return false;
  }
}

/**
 * Send now: steer a queued message into the running turn, by the same rule ⌘⏎ follows. One the rule
 * refuses stays queued and says why; one the engine refuses stays queued too.
 */
export async function sendQueuedNow(id: string, queuedId: string): Promise<boolean> {
  const thread = get().threads[id];
  const item = thread?.queue.find((q) => q.id === queuedId);
  if (!item || !thread) return false;
  const verdict = steerability(item.extras, steerConditions(thread));
  if (!verdict.ok) {
    toast(STEER_REFUSAL[verdict.reason]);
    return false;
  }
  if (await steer(id, item.text, item.extras)) {
    removeQueued(id, queuedId);
    return true;
  }
  toast("The turn can't take it now. It stays queued");
  return false;
}

/**
 * Resume: the first message a stopped or failed turn held starts a turn of its own. The rest follow
 * it, each when the turn before completes; a new message sent meanwhile waits behind them.
 */
export async function resumeQueue(id: string): Promise<boolean> {
  const thread = get().threads[id];
  if (!thread || thread.running) return false;
  const next = nextQueued(thread.queue, { running: false });
  updateThread(id, (t) => ({ ...t, queueHeld: undefined }));
  if (!next) return false;
  removeQueued(id, next.id);
  await runPrompt(id, next.text, next.extras, {
    fromView: next.fromView,
    link: next.link,
    wake: next.wake,
    wakeLinks: next.wakeLinks,
    wakeAsking: next.wakeAsking,
  });
  return true;
}

/**
 * Open a queued message in the composer (its pencil, or ↑ at the start of an empty one). The item
 * stays where it waits, marked as being edited, and whatever the composer held comes back when the
 * edit ends. A message another thread sent cannot be edited: sending it again would lose its link.
 */
export function startQueuedEdit(id: string, queuedId: string): boolean {
  const item = get().threads[id]?.queue.find((q) => q.id === queuedId);
  if (!item || item.link || item.wake) return false;
  const before = { text: get().drafts[id] ?? "", extras: get().extras[id] ?? emptyExtras };
  set({ queuedEdit: { threadId: id, queuedId, ...before } });
  setDraft(id, item.text);
  setExtras(id, () => item.extras ?? emptyExtras);
  return true;
}

/** ⏎ or the check button on a queued message being edited: saved where it waits, never sent. */
export function saveQueuedEdit(): boolean {
  const edit = get().queuedEdit;
  if (!edit) return false;
  updateQueued(edit.threadId, edit.queuedId, get().drafts[edit.threadId] ?? "", get().extras[edit.threadId]);
  restoreComposer(edit);
  return true;
}

/** Esc or Cancel: the queued message stays as it was, and the composer gets its own message back. */
export function cancelQueuedEdit(): boolean {
  const edit = get().queuedEdit;
  if (!edit) return false;
  restoreComposer(edit);
  return true;
}

/**
 * The item left the queue while it was being edited (it started, or was removed): the edit ends. What
 * was typed stays in the composer when it was empty before the edit, and is dropped when it was not.
 */
export function endQueuedEdit(): void {
  const edit = get().queuedEdit;
  if (!edit) return;
  if (!edit.text.trim() && !hasExtras(edit.extras)) {
    set({ queuedEdit: null });
    return;
  }
  restoreComposer(edit);
}

function restoreComposer(edit: NonNullable<AppState["queuedEdit"]>): void {
  set({ queuedEdit: null });
  setDraft(edit.threadId, edit.text);
  setExtras(edit.threadId, () => edit.extras);
}

export function removeQueued(id: string, queuedId: string) {
  const link = get().threads[id]?.queue.find((q) => q.id === queuedId)?.link;
  updateThread(id, (t) => ({ ...t, queue: t.queue.filter((q) => q.id !== queuedId) }));
  if (link) queuedLinkDropped(link);
  // Editing a message that just left the queue (it started, or it was removed): the edit ends.
  if (get().queuedEdit?.queuedId === queuedId) endQueuedEdit();
}

/** A queued message edited where it waits; its attachments are replaced with the composer's. */
export function updateQueued(id: string, queuedId: string, text: string, extras?: ComposeExtras) {
  const trimmed = text.trim();
  updateThread(id, (t) => {
    const item = t.queue.find((q) => q.id === queuedId);
    if (!item) return t;
    const next = extras ?? item.extras;
    // Its attachments stay; one with attachments survives an emptied text.
    if (!trimmed && !hasExtras(next)) return { ...t, queue: t.queue.filter((q) => q.id !== queuedId) };
    return {
      ...t,
      queue: t.queue.map((q) => (q.id === queuedId ? { ...q, text: trimmed, extras: next } : q)),
    };
  });
}

/** Move a queued message one place earlier (-1) or later (1). */
export function moveQueued(id: string, queuedId: string, by: -1 | 1) {
  updateThread(id, (t) => {
    const at = t.queue.findIndex((q) => q.id === queuedId);
    const to = at + by;
    if (at < 0 || to < 0 || to >= t.queue.length) return t;
    const queue = t.queue.slice();
    [queue[at], queue[to]] = [queue[to]!, queue[at]!];
    return { ...t, queue };
  });
}

export async function runPrompt(
  id: string,
  text: string,
  extras: ComposeExtras = emptyExtras,
  options: {
    fromView?: boolean;
    link?: string;
    wake?: boolean;
    /** The links a queued wake row reports: marked as heard when its turn starts (host.ts). */
    wakeLinks?: string[];
    /** The links that row named as waiting on the user: a question, not a finish (host.ts). */
    wakeAsking?: string[];
    automatic?: boolean;
    /** The turn that continues after a limit: its wait is settled when it ends. */
    limitResume?: boolean;
  } = {},
): Promise<void> {
  const conn = engine();
  const thread = get().threads[id];
  // A prompt no agent asked for is the user's: the thread is theirs now, even
  // when it was started by an agent (state/topLevel.ts). A resume after a
  // restart is not a message the user sent.
  if (!options.link && !options.wake && !options.automatic) markMessaged(id);
  // Whoever starts a turn now moots the wait, and if this one hits the limit too it waits again.
  if (!options.limitResume && !thread?.running) dropLimitWait(id);
  // Never a second prompt into a running turn: an external harness takes it as the next turn and can
  // interrupt the tools it is still running (a parent's wait_session). It waits in the queue, like a
  // message typed meanwhile, and starts when this turn ends.
  if (thread?.running) {
    updateThread(id, (t) => ({
      ...t,
      queue: [
        ...t.queue,
        {
          id: localId("queued"),
          text,
          extras: hasExtras(extras) ? extras : undefined,
          fromView: options.fromView,
          link: options.link,
          wake: options.wake,
          wakeLinks: options.wakeLinks,
          wakeAsking: options.wakeAsking,
        },
      ],
    }));
    return;
  }
  const startIndex = thread?.items.length ?? 0;
  if (options.link) linkStarted(options.link);
  // A queued wake reports its links now, as its turn starts: late enough that a restart before this
  // point brings the wake back, early enough that nothing in the turn reports it again.
  if (options.wake && options.wakeLinks?.length) wakeDelivered(options.wakeLinks, options.wakeAsking ?? []);
  if (id in get().snoozed) snoozeThread(id, null);
  const harness = threadHarness(thread);
  const prompt = promptBlocks(text, extras, {
    images: acceptsImages(thread),
    builtin: harness?.builtin ?? true,
    cwd: thread?.cwd ?? "",
  });
  const shown = [extras.command?.name, ...extras.links, text].filter(Boolean).join(" ");
  updateThread(id, (t) =>
    beginTurn(t, shown, Date.now(), sentAttachments(extras), options.fromView, options.wake),
  );
  // A new prompt starts a new report, if it is one.
  if (get().report?.threadId === id) set({ report: null });
  set((s) => ({
    sessions: byActivity(
      s.sessions.map((x) =>
        x.id === id
          ? { ...x, updatedAt: Date.now(), title: x.title ?? (titleFrom(shown, 80) || "Image") }
          : x,
      ),
    ),
  }));
  let outcome: "completed" | "cancelled" | "failed" = "completed";
  let failure: string | undefined;
  let unused: string[] = [];
  try {
    const result = (await conn.request(Method.sessionPrompt, {
      sessionId: id,
      prompt,
    })) as JsonObject | null;
    if (str(result?.stopReason) === "cancelled") outcome = "cancelled";
    const meta = (result?._meta as JsonObject | undefined)?.rustcode as JsonObject | undefined;
    unused = Array.isArray(meta?.unusedSteers)
      ? meta.unusedSteers.filter((x): x is string => typeof x === "string")
      : [];
  } catch (error) {
    outcome = "failed";
    failure = errorText(error);
  }
  updateThread(id, (t) => requeueSteers(finishTurn(t, outcome, Date.now(), failure), unused));
  if (outcome === "completed") markFinished(id);
  else if (outcome === "failed") markFinished(id, Date.now(), true);
  const waiting = settleLimitWait(id, outcome, failure, startIndex);
  // A report command that answered in the panel leaves no turn behind: its message and footer go.
  const reported = get().report?.threadId === id;
  if (reported && outcome === "completed" && isReportCommand(id, text, extras)) {
    updateThread(id, (t) => ({ ...t, items: t.items.slice(0, startIndex) }));
  }
  if (options.link)
    linkFinished(options.link, id, outcome, failure, get().threads[id]?.items.slice(startIndex) ?? []);
  // Every finished turn of a thread matters to a parent waiting on it: a middle thread's real
  // answer is the one after the work it started (state/host.ts threadTurnEnded).
  threadTurnEnded(id, !!options.wake, outcome, failure, get().threads[id]?.items.slice(startIndex) ?? []);
  void refreshSessions();
  void refreshPlanUsage(id);
  // A window's numbers move with the turn, and so does the account's own row:
  // the thread on screen asks again as it ends.
  if (id === get().selectedId) void refreshPlanCache();
  void refreshAgents(id);
  void refreshCheckpoints(id);
  if ((outcome === "completed" || outcome === "failed") && id !== get().selectedId) {
    const name = get().sessions.find((s) => s.id === id)?.title || "A thread";
    toast(
      waiting
        ? `“${name}” hit its limit and continues at ${limitTime(waiting.until)}`
        : outcome === "failed"
          ? `“${name}” failed`
          : `“${name}” finished`,
      waiting ? "info" : outcome === "failed" ? "error" : "success",
      { action: { label: "Open", run: () => selectThread(id) } },
    );
  }
  // A queued message starts at once, so "working" stays up across the hand-off. A turn that was
  // stopped or failed leaves the queue held instead (thread.ts `holdsQueue`): the rows read Held and
  // Resume starts them.
  // Nor does one go out into a limit: it waits for the turn that continues.
  const after = get().threads[id];
  const next = after && !waiting ? nextQueued(after.queue, after) : undefined;
  if (next) {
    removeQueued(id, next.id);
    await runPrompt(id, next.text, next.extras, {
      fromView: next.fromView,
      link: next.link,
      wake: next.wake,
      wakeLinks: next.wakeLinks,
      wakeAsking: next.wakeAsking,
    });
  }
}

export async function cancel(id = get().selectedId): Promise<void> {
  if (!id || !connection) return;
  const thread = get().threads[id];
  // Stopping a thread stops the work it gave other threads (M1); turns the user started there go on.
  void stopLinksFrom(id);
  if (!thread?.running) return;
  try {
    connection.notify(Method.sessionCancel, { sessionId: id });
    // ACP: after `session/cancel` the client answers outstanding permission requests as cancelled.
    for (const request of thread.permissions) connection.respond(request.rpcId, permissionCancelled);
    updateThread(id, (t) => ({ ...t, permissions: [] }));
  } catch (error) {
    toast(errorText(error), "error");
  }
}

export async function setConfigOption(id: string, configId: string, value: string | boolean): Promise<void> {
  const params: JsonObject =
    typeof value === "boolean"
      ? { sessionId: id, configId, type: "boolean", value }
      : { sessionId: id, configId, value };
  try {
    const result = (await engine().request(
      Method.sessionSetConfigOption,
      params,
      15_000,
    )) as JsonObject | null;
    if (result?.configOptions) {
      updateThread(id, (t) => ({ ...t, configOptions: parseConfigOptions(result.configOptions) }));
    }
    // A model or effort picked in a thread is also its harness's next new-thread choice; the
    // engine keeps it (docs/harnesses.md), the app only mirrors it.
    const thread = get().threads[id];
    if (thread && (configId === "model" || configId === "effort")) {
      const current = (option: string) => thread.configOptions.find((o) => o.id === option)?.current ?? null;
      noteChoice(thread.harness?.id ?? "rustcode", { model: current("model"), effort: current("effort") });
    }
  } catch (error) {
    toast(`Could not change ${configId}: ${errorText(error)}`, "error");
  }
}

// ---------------------------------------------------------------- approvals

/**
 * Answer exactly this request; it leaves the queue before the reply, so a double click cannot
 * answer the next. A question also leaves its answer in the transcript, where the reader can see
 * what was asked and what they said (state/thread.ts `questionItem`).
 */
function reply(request: PermissionRequest, result: JsonObject, answer?: string) {
  updateThread(request.sessionId, (t) => ({
    ...t,
    permissions: t.permissions.filter((p) => p.rpcId !== request.rpcId),
    items: answer ? [...t.items, questionItem(request, answer)] : t.items,
  }));
  connection?.respond(request.rpcId, result);
}

export function answerPermission(request: PermissionRequest, option: PermissionOption | null) {
  reply(
    request,
    option ? permissionSelected(option.id) : permissionCancelled,
    request.question ? (option?.name ?? "Declined") : undefined,
  );
}

/** Approve every approval the thread waits on (not questions), each with its "allow once" option. */
export function approveAll(threadId: string) {
  const pending = get().threads[threadId]?.permissions.filter((p) => !p.question) ?? [];
  for (const request of pending) {
    const option =
      request.options.find((o) => o.kind === "allow_once") ??
      request.options.find((o) => o.kind === "allow_always");
    if (option) reply(request, permissionSelected(option.id));
  }
}

// ---------------------------------------------------------------- permission mode

export function permissionModeOf(s: AppState, threadId: string | undefined): PermissionMode {
  if (!threadId) return s.newThreadPermissionMode ?? s.defaultPermissionMode;
  return s.permissionModes[threadId] ?? s.defaultPermissionMode;
}

/**
 * Settings › General › Default access: what a new thread may do without asking. Threads already
 * listed keep the access they had, so lowering the default does not silently tighten one of them.
 */
export function setDefaultPermissionMode(mode: PermissionMode) {
  const { defaultPermissionMode: before, permissionModes: known, sessions, threads } = get();
  const modes = { ...known };
  for (const id of [...sessions.map((x) => x.id), ...Object.keys(threads)]) modes[id] ??= before;
  set({ defaultPermissionMode: mode, permissionModes: modes });
  store(STORAGE.permissionMode, mode);
  store(STORAGE.permissionModes, JSON.stringify(modes));
}

/**
 * Set what `threadId` may do without asking, from the composer's pill: only that thread, or (for a
 * thread not started yet) the next one. The default for new threads is the Settings row, so a
 * thread's choice never changes it. Requests already waiting that the mode now allows are answered.
 */
export function setPermissionMode(threadId: string | undefined, mode: PermissionMode) {
  if (!threadId) {
    set({ newThreadPermissionMode: mode });
    return;
  }
  const modes = { ...get().permissionModes, [threadId]: mode };
  set({ permissionModes: modes });
  store(STORAGE.permissionModes, JSON.stringify(modes));
  for (const request of get().threads[threadId]?.permissions ?? []) {
    const option = autoAnswer(request, mode);
    if (option) reply(request, permissionSelected(option.id));
  }
}

export function answerQuestionOptions(request: PermissionRequest, options: PermissionOption[]) {
  if (!options.length) return;
  reply(
    request,
    options.length === 1 ? permissionSelected(options[0]!.id) : questionOptionsAnswer(options),
    options.map((o) => o.name).join(", "),
  );
}

export function answerQuestionText(request: PermissionRequest, text: string) {
  const carrier = request.options.find(
    (o) => o.kind === "allow_once" || o.kind === "allow_always" || !o.kind,
  );
  if (!carrier || !text.trim()) return;
  reply(request, questionTextAnswer(text.trim(), carrier), text.trim());
}

// ---------------------------------------------------------------- harnesses

export async function refreshHarnesses(): Promise<HarnessSnapshot | null> {
  const conn = connection;
  if (!conn) return null;
  try {
    const snapshot = parseHarnessSnapshot(await conn.request(Method.harnesses, {}, 15_000));
    set({ harnesses: snapshot });
    if (!snapshot.onboarded) set({ onboardingOpen: true });
    return snapshot;
  } catch (error) {
    // An engine without the harness layer: the built-in harness only, no onboarding.
    const snapshot = fallbackHarnessSnapshot();
    if (!(error instanceof RpcError && error.isMethodNotFound)) {
      toast(`Could not read harnesses: ${errorText(error)}`, "error");
    }
    set({ harnesses: snapshot });
    return snapshot;
  }
}

async function harnessCall(method: string, params: JsonObject): Promise<HarnessSnapshot | null> {
  try {
    const snapshot = parseHarnessSnapshot(await engine().request(method, params, 15_000));
    set({ harnesses: snapshot });
    return snapshot;
  } catch (error) {
    toast(errorText(error), "error");
    return null;
  }
}

export async function setHarnessEnabled(id: string, enabled: boolean): Promise<HarnessSnapshot | null> {
  // The switch in Settings shows the new state; only a failure needs a word.
  return harnessCall(Method.harnessSet, { id, enabled });
}
/** The account id a harness was last given for new threads; null is its default login. */
export function lastAccountFor(harness: string): string | null {
  try {
    const raw = obj(JSON.parse(stored(STORAGE.harnessAccountChoices) ?? "{}")) ?? {};
    return str(raw[harness]) ?? null;
  } catch {
    return null;
  }
}

function storeAccount(harness: string, account: string | null) {
  let all: JsonObject;
  try {
    all = obj(JSON.parse(stored(STORAGE.harnessAccountChoices) ?? "{}")) ?? {};
  } catch {
    all = {};
  }
  if (account) all[harness] = account;
  else delete all[harness];
  store(STORAGE.harnessAccountChoices, JSON.stringify(all));
}

/** The named accounts of `harness`, as the snapshot lists them. */
export function accountsOf(s: Pick<AppState, "harnesses">, harness: string | undefined): HarnessAccount[] {
  return harness ? (s.harnesses?.harnesses.find((h) => h.id === harness)?.accounts ?? []) : [];
}

/** The domain part of an account name, as the account's id must read. */
export function accountSlug(name: string): string {
  return name
    .toLowerCase()
    .replace(/[^a-z0-9_.-]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 64);
}

/** The user picked an account for new threads on the current harness. */
export function pickNewThreadAccount(account: string | null) {
  const harness = newThreadChoice(get())?.harness;
  set({ newThreadAccount: account });
  if (harness) storeAccount(harness, account);
  // The login picked may never have been read: its own meter numbers come now.
  void refreshPlanCache();
}

export const addHarnessAccount = (harness: string, id: string, name: string) =>
  harnessCall(Method.harnessAccountAdd, { harness, id, name });
export const removeHarnessAccount = (harness: string, id: string) =>
  harnessCall(Method.harnessAccountRemove, { harness, id });

/** Reads each account's own credentials again (nothing is started, no token is read). */
export async function checkHarnessAccounts(): Promise<HarnessSnapshot | null> {
  const snapshot = await harnessCall(Method.harnessAccountCheck, {});
  // Claude Code signs in with its own credentials, which that snapshot does not
  // report (`auth_probe: None`), and its plan read pauses once it finds the
  // default login signed out or cannot read it (a denied Keychain prompt), so
  // neither asks again and again. An explicit check is the user asking to look
  // now: read the default login again, or it would stay as it was found after
  // signing in or out.
  if (claudeUsageAllowed()) void refreshPlanCache(true);
  return snapshot;
}

export const addHarness = (id: string, name: string, command: string[]) =>
  harnessCall(Method.harnessSet, { id, name, kind: "acp", command, enabled: true });
export const removeHarness = (id: string) => harnessCall(Method.harnessRemove, { id });
export const setDefaultHarness = (choice: NewThreadChoice) =>
  harnessCall(Method.harnessDefault, { harness: choice.harness, model: choice.model, effort: choice.effort });
export const completeOnboarding = async () => {
  const snapshot = await harnessCall(Method.onboardingComplete, {});
  if (snapshot) set({ onboardingOpen: false });
};

/** "Test": ask the harness for its models. Throws the harness's error so the card can show it. */
export async function fetchHarnessModels(id: string): Promise<void> {
  const result = (await engine().request(Method.harnessModels, { harness: id }, 60_000)) as JsonObject | null;
  const models = parseHarnessModels(result?.models as Json);
  set((s) =>
    s.harnesses
      ? {
          harnesses: {
            ...s.harnesses,
            harnesses: s.harnesses.harnesses.map((h) =>
              h.id === id ? { ...h, models, auth: h.auth === "unknown" ? "ok" : h.auth, error: null } : h,
            ),
          },
        }
      : s,
  );
  // The engine may have learned more (auth, version); take its snapshot too.
  void refreshHarnesses();
}

/** Model listings in flight (ensureHarnessModels), one per harness. */
const modelsAsked = new Map<string, Promise<void>>();

/**
 * An external harness lists its models only when asked (`_rustcode/harnesses/models`;
 * the engine caches them across restarts, but a harness never listed has none
 * yet). The model picker of a new thread on it, and a restored thread on it,
 * ask, so the model list, effort and Fast mode are there before the harness
 * runs. A failure stays quiet (the picker says the harness picks its default)
 * and is not kept: the next ask, such as opening the picker, tries again.
 */
export function ensureHarnessModels(id: string): Promise<void> {
  const harness = get().harnesses?.harnesses.find((h) => h.id === id);
  if (!harness || harness.builtin || harness.models.length || get().status !== "ready")
    return Promise.resolve();
  let asked = modelsAsked.get(id);
  if (!asked) {
    asked = fetchHarnessModels(id)
      .catch(() => undefined)
      .finally(() => modelsAsked.delete(id));
    modelsAsked.set(id, asked);
  }
  return asked;
}

export function setNewThreadChoice(choice: NewThreadChoice | null) {
  set({ newThread: choice });
}

/** What the app keeps per harness when the engine does not remember choices. */
export function localChoices(): Record<string, HarnessLast> {
  try {
    const raw = obj(JSON.parse(stored(STORAGE.harnessChoices) ?? "{}")) ?? {};
    const out: Record<string, HarnessLast> = {};
    for (const [id, value] of Object.entries(raw)) {
      const last = parseHarnessLast(value);
      if (last) out[id] = last;
    }
    return out;
  } catch {
    return {};
  }
}

/** The app's own record of choices, only for an engine that keeps none. */
function appChoices(snapshot: HarnessSnapshot): Record<string, HarnessLast> | undefined {
  return snapshot.remembers ? undefined : localChoices();
}

/** Mirror a harness's last choice: in the snapshot, and in app storage when the engine does not keep it. */
function noteChoice(harness: string, last: HarnessLast) {
  const value = last.model || last.effort ? last : null;
  set((s) =>
    s.harnesses
      ? {
          harnesses: {
            ...s.harnesses,
            harnesses: s.harnesses.harnesses.map((h) => (h.id === harness ? { ...h, last: value } : h)),
          },
        }
      : s,
  );
  if (get().harnesses?.remembers) return;
  const all = localChoices();
  if (value) all[harness] = value;
  else delete all[harness];
  store(STORAGE.harnessChoices, JSON.stringify(all));
}

/**
 * The user picked a model or effort in the new-thread picker: it is the choice, and the harness's
 * remembered one (`_rustcode/harnesses/remember`, else app storage), so switching harness and back,
 * or a restart, starts there again.
 */
export function pickNewThreadChoice(choice: NewThreadChoice) {
  set({ newThread: choice });
  const last = { model: choice.model, effort: choice.effort };
  noteChoice(choice.harness, last);
  if (!get().harnesses?.remembers || !connection) return;
  void engine()
    .request(Method.harnessRemember, { harness: choice.harness, ...last }, 15_000)
    .catch(() => undefined);
}

/** Switch the new-thread picker to `harness`, on the model and effort it was last left on. */
export function switchNewThreadHarness(harness: string) {
  const snapshot = get().harnesses;
  if (!snapshot) return;
  const known = snapshot.harnesses.find((h) => h.id === harness)?.accounts ?? [];
  const remembered = lastAccountFor(harness);
  set({
    newThread: restoreChoice(snapshot, harness, appChoices(snapshot)),
    newThreadAccount: known.some((a) => a.id === remembered) ? remembered : null,
  });
  // Another harness means another login, whose meter may have no numbers yet.
  void refreshPlanCache();
}

/**
 * Move an idle thread to another account of its harness: the new account's
 * process starts, the conversation moves as a handoff summary with the next
 * message, and the thread keeps its history. Never while a turn runs.
 */
export async function switchThreadAccount(id: string, account: string | null): Promise<boolean> {
  const thread = get().threads[id];
  if (!thread || thread.running) return false;
  const harness = thread.harness?.id;
  if (!harness) return false;
  const named = accountsOf(get(), harness).find((a) => a.id === account);
  try {
    const result = (await engine().request(
      Method.sessionHarness,
      { sessionId: id, harness, account },
      30_000,
    )) as JsonObject | null;
    const next = parseSessionHarness(result) ?? thread.harness;
    updateThread(id, (t) => ({
      ...t,
      harness: next,
      configOptions: parseConfigOptions(result?.configOptions),
    }));
    set((s) => ({
      sessions: s.sessions.map((x) => (x.id === id ? { ...x, account: next?.account } : x)),
    }));
    toast(
      named
        ? `This thread now runs on ${named.name}. The conversation so far moves over as a summary with your next message.`
        : "This thread now runs on the default Claude login, with the conversation so far as a summary.",
      "success",
    );
    void refreshPlanUsage(id);
    // The account switched to may never have been read: its meter numbers come now.
    void refreshPlanCache();
    return true;
  } catch (error) {
    toast(`Could not switch account: ${errorText(error)}`, "error");
    return false;
  }
}

/**
 * Move a thread to another harness: the
 * harness starts fresh and its first prompt carries the conversation so far
 * (`_rustcode/session/harness`). It starts on the model and effort it was
 * last left on; the thread shows a divider where it switched.
 */
export async function switchThreadHarness(id: string, harness: string): Promise<boolean> {
  const thread = get().threads[id];
  if (!thread || thread.running) return false;
  const last = get().harnesses?.harnesses.find((h) => h.id === harness)?.last;
  try {
    const result = (await engine().request(
      Method.sessionHarness,
      { sessionId: id, harness, model: last?.model ?? null, effort: last?.effort ?? null },
      30_000,
    )) as JsonObject | null;
    const listed = get().harnesses?.harnesses.find((h) => h.id === harness);
    const next =
      parseSessionHarness(result) ?? (listed && { id: harness, capabilities: listed.capabilities });
    updateThread(id, (t) => ({
      ...t,
      harness: next,
      configOptions: parseConfigOptions(result?.configOptions),
    }));
    set((s) => ({
      sessions: s.sessions.map((x) => (x.id === id ? { ...x, harness, account: undefined } : x)),
    }));
    const opened = get().threads[id];
    if (opened && lacksModels(opened)) void ensureHarnessModels(harness);
    void refreshPlanUsage(id);
    void refreshPlanCache();
    return true;
  } catch (error) {
    toast(`Could not switch harness: ${errorText(error)}`, "error");
    return false;
  }
}

// ---------------------------------------------------------------- subagents and usage

export function setResumeAfterRestart(on: boolean) {
  set({ resumeAfterRestart: on });
  store(STORAGE.resumeAfterRestart, on ? "1" : "0");
}

export function setRoundedFont(on: boolean) {
  set({ roundedFont: on });
  store(STORAGE.roundedFont, on ? "1" : "0");
}

/** A command's shortcut as the app shows it ("⌘B"), or "" when it has none. */
export function shortcutHint(id: string): string {
  const s = get();
  return commandHint(
    bindingOf(s.shortcuts, id),
    APP_COMMANDS.find((c) => c.id === id),
  );
}

/** The command a keydown runs, if any (state/shortcuts.ts runs it). */
export function shortcutFor(event: KeyEventLike): string | null {
  return commandFor(get().shortcuts, event);
}

/** What a command is bound to now, for a component that must follow a remap. */
export function useShortcutHint(id: string): string {
  return useApp((s) =>
    commandHint(
      bindingOf(s.shortcuts, id),
      APP_COMMANDS.find((c) => c.id === id),
    ),
  );
}

/** Rebinds a command; the caller has already checked the binding (lib/keys.ts). */
export function setShortcut(id: string, binding: string) {
  const next = { ...get().shortcuts, [id]: binding };
  set({ shortcuts: next });
  store(STORAGE.shortcuts, JSON.stringify(next));
}

/** Drops a command's override: its default binding is back. */
export function resetShortcut(id: string) {
  const next = { ...get().shortcuts };
  delete next[id];
  set({ shortcuts: next });
  store(STORAGE.shortcuts, JSON.stringify(next));
}

export function setAnimateStreaming(on: boolean) {
  set({ animateStreaming: on });
  store(STORAGE.animateStreaming, on ? "1" : "0");
}

export function setChatWidth(width: ChatWidth) {
  set({ chatWidth: width });
  store(STORAGE.chatWidth, width);
}

/**
 * The header's buttons, in the order they show. The ids this build knows are
 * saved beside them, so a button added later shows by default (headerButtons.ts).
 */
export function setHeaderButtons(shown: HeaderButtonId[]) {
  set({ headerButtons: shown });
  store(STORAGE.headerButtons, JSON.stringify({ shown, known: HEADER_BUTTON_IDS }));
}

export function setReadAcrossProjects(on: boolean) {
  set({ readAcrossProjects: on });
  store(STORAGE.readAcrossProjects, on ? "1" : "0");
}

export function setOrchestration(on: boolean) {
  set((s) => ({
    orchestration: on,
    orchestrationPending: s.status === "ready" ? !s.orchestrationPending : s.orchestrationPending,
  }));
  store(STORAGE.orchestration, on ? "1" : "0");
}

/** The number of distinct threads one thread may run at once, kept to 2..16. */
export function setThreadLimit(limit: number) {
  const n = Math.min(MAX_ACTIVE_LIMIT, Math.max(MIN_ACTIVE_LINKS, Math.round(limit)));
  set({ threadLimit: n });
  store(STORAGE.threadLimit, String(n));
}

// ---------------------------------------------------------------- status and views

/**
 * What a thread is doing, for chips and the board: running or needs input
 * from the live turn, else review or done as the engine (or the app, on an
 * older engine) keeps it.
 */
// The thread list by id, built once per list: selectors in every sidebar row run on each store
// change (60 times a second while a reply streams), so a scan of 500 sessions per row adds up.
const sessionIndex = new WeakMap<SessionSummary[], Map<string, SessionSummary>>();
export function sessionById(s: AppState, id: string): SessionSummary | undefined {
  let index = sessionIndex.get(s.sessions);
  if (!index) {
    index = new Map();
    for (const x of s.sessions) if (!index.has(x.id)) index.set(x.id, x); // First wins, as `find` did.
    sessionIndex.set(s.sessions, index);
  }
  return index.get(id);
}

export function statusOf(s: AppState, id: string): ThreadStatus {
  const t = s.threads[id];
  if (t?.permissions.length) return "needs_input";
  if (t?.running) return "running";
  const listed = sessionById(s, id)?.status;
  const mark = s.statusMarks[id];
  if (listed === "done" || listed === "review") return mark ?? listed;
  return mark ?? "idle";
}

function saveMarks(marks: Record<string, ThreadStatus>) {
  set({ statusMarks: marks });
  store(STORAGE.statusMarks, JSON.stringify(marks));
}

export function clearStatusMark(id: string) {
  const { [id]: _gone, ...rest } = get().statusMarks;
  if (_gone) saveMarks(rest);
}

/** Mark a thread for review, done, or back to idle; stored by the engine when it can, else by the app. */
export function setThreadStatus(id: string, status: "review" | "done" | "idle"): Promise<number> {
  return setThreadsStatus([id], status);
}

/**
 * The same for several threads at once: the app's list first, then the engine one request each.
 * Whatever fails is reported in one toast for the selection, not one per thread.
 */
export async function setThreadsStatus(ids: string[], status: "review" | "done" | "idle"): Promise<number> {
  const wanted = new Set(ids);
  set((s) => ({ sessions: s.sessions.map((x) => (wanted.has(x.id) ? { ...x, status } : x)) }));
  if (status === "idle") for (const id of ids) clearStatusMark(id);
  else saveMarks({ ...get().statusMarks, ...Object.fromEntries(ids.map((id) => [id, status])) });
  if (!get().features.status || !connection) return 0;
  const link = connection;
  const errors: unknown[] = [];
  for (const id of ids) {
    try {
      await link.request(V2Method.sessionStatus, { session_id: id, status }, 10_000);
      // The engine keeps it now; the app's mark only covers the gap until the list says so.
    } catch (error) {
      if (!(error instanceof RpcError && error.isMethodNotFound)) errors.push(error);
    }
  }
  if (errors.length) {
    const first = errorText(errors[0]);
    toast(
      ids.length === 1
        ? `Could not save the status: ${first}`
        : `Could not save the status of ${errors.length} of ${ids.length} threads: ${first}`,
      "error",
    );
  }
  return errors.length;
}

/**
 * Settles threads (marks them Done) because the user asked: one Undo toast puts
 * each back the way it was (review stays review, the rest go back to idle).
 *
 * Settling the thread on screen in the chat leaves it, with the panel, for the
 * new-thread screen in the same folder; the board, review and compare have no
 * new-thread screen, so settling there stays on that view. A caller that is
 * not the user — an agent's `organize`, a fan-out's cleanup, a thread merged on
 * the engine — calls `setThreadStatus` instead, which never moves the screen.
 */
export function settleThreads(ids: string[]): void {
  const state = get();
  const before = ids.map((id) => ({ id, status: statusOf(state, id) })).filter((x) => x.status !== "done");
  if (!before.length) {
    toast("Already settled", "info");
    return;
  }
  void setThreadsStatus(
    before.map((x) => x.id),
    "done",
  );
  if (state.view.kind === "chat" && state.selectedId && before.some((x) => x.id === state.selectedId)) {
    startNewThread();
  }
  const title =
    before.length === 1
      ? sessionById(state, before[0]!.id)?.title || get().threads[before[0]!.id]?.title || "New thread"
      : "";
  toast(before.length === 1 ? `Settled "${title}"` : `Settled ${before.length} threads`, "info", {
    action: {
      label: "Undo",
      run: () => {
        const review = before.filter((x) => x.status === "review").map((x) => x.id);
        const rest = before.filter((x) => x.status !== "review").map((x) => x.id);
        if (review.length) void setThreadsStatus(review, "review");
        if (rest.length) void setThreadsStatus(rest, "idle");
      },
    },
  });
}

/** Settles one thread, with the same Undo and the same rule about leaving the screen. */
export function settleThread(id: string): void {
  settleThreads([id]);
}

export function setView(view: AppState["view"]) {
  dismissOverlays();
  set({ view, switcherOpen: false, settings: null });
}

/** The worktree a thread runs in, from the thread or the list. */
export function worktreeOf(s: AppState, id: string): Worktree | undefined {
  return s.threads[id]?.worktree ?? sessionById(s, id)?.worktree;
}

/**
 * Remove a thread's worktree, guarded: the confirm says when it has
 * uncommitted changes or commits on no remote, and the engine refuses those
 * unless the user removes it anyway.
 */
export async function removeWorktree(id: string): Promise<void> {
  const wt = worktreeOf(get(), id);
  if (!wt || !connection) return;
  if (get().threads[id]?.running) {
    toast("Stop the running turn before removing its worktree.", "info");
    return;
  }
  let info: Worktree | undefined;
  try {
    const listed = (await connection.request(V2Method.worktreeList, { repo: wt.repo }, 15_000)) as Json;
    info = parseWorktreeList(listed).find((w) => w.path === wt.path);
  } catch {
    // The list is advice only; the engine checks again when removing.
  }
  const reasons = [
    info?.dirty ? "it has uncommitted changes" : "",
    info?.ahead ? `${info.ahead} commit${info.ahead === 1 ? " is" : "s are"} on no remote branch` : "",
  ].filter(Boolean);
  const go = async (force: boolean) => {
    try {
      await engine().request(V2Method.worktreeRemove, { path: wt.path, force }, 30_000);
    } catch (error) {
      const refusal = error instanceof RpcError ? removeRefusal(error.data) : undefined;
      if (refusal && !force) {
        askConfirm({
          title: "Remove it anyway?",
          message:
            refusal === "dirty"
              ? `${wt.branch} has uncommitted changes. Removing the worktree deletes them.`
              : `${wt.branch} has commits that are on no remote branch. Removing the worktree may lose them.`,
          action: "Remove Anyway",
          destructive: true,
          run: () => void go(true),
        });
        return;
      }
      toast(`Could not remove the worktree: ${errorText(error)}`, "error");
      return;
    }
    set((s) => ({
      sessions: s.sessions.map((x) => (x.id === id ? { ...x, worktree: undefined } : x)),
    }));
    updateThread(id, (t) => ({ ...t, worktree: undefined }));
  };
  askConfirm({
    title: `Remove the worktree of ${wt.branch}?`,
    message: reasons.length
      ? `Careful: ${reasons.join(", and ")}. The folder ${displayPath(wt.path)} goes; the thread stays.`
      : `The folder ${displayPath(wt.path)} goes; the thread and its history stay. The branch is deleted only when it is merged.`,
    action: "Remove Worktree",
    destructive: true,
    run: () => void go(false),
  });
}

// ---------------------------------------------------------------- checkpoints

export async function refreshCheckpoints(id: string): Promise<void> {
  if (!connection || !get().features.checkpoints) return;
  try {
    const list = parseCheckpoints(
      await connection.request(V2Method.checkpointList, { session_id: id }, 15_000),
    );
    set((s) => ({ checkpoints: { ...s.checkpoints, [id]: list } }));
  } catch {
    // A thread outside a git repository has none.
  }
}

/** What one turn changed: its checkpoint against the next (or the working tree). */
export async function turnDiff(id: string, turn: number): Promise<PatchFile[]> {
  return parseTurnDiff(await engine().request(V2Method.checkpointDiff, { session_id: id, turn }, 30_000));
}

/**
 * Put the files back to before a turn; with `rewind`, the conversation too
 * when the harness can rewind (else the engine keeps it and says so).
 */
export async function restoreCheckpoint(id: string, turn: number, rewind: boolean): Promise<boolean> {
  try {
    const result = (await engine().request(
      V2Method.checkpointRestore,
      { session_id: id, turn, rewind },
      60_000,
    )) as JsonObject | null;
    const kept = result?.conversation === "kept";
    // The restored files and the reloaded conversation are the feedback.
    if (rewind && !kept) {
      // The conversation changed under the app: load it again.
      updateThread(id, (t) => ({ ...t, loaded: false }));
      set((s) => {
        const { [id]: _gone, ...threads } = s.threads;
        return { threads };
      });
      await openThread(id);
    }
    void refreshCheckpoints(id);
    return true;
  } catch (error) {
    toast(`Could not restore: ${errorText(error)}`, "error");
    return false;
  }
}

export function setNewWorktree(change: Partial<AppState["newWorktree"]>) {
  set((s) => ({ newWorktree: { ...s.newWorktree, ...change } }));
}

export function setSmartUi(on: boolean) {
  set((s) => ({ smartUi: on, smartUiPending: s.status === "ready" ? !s.smartUiPending : s.smartUiPending }));
  store(STORAGE.smartUi, on ? "1" : "0");
}

/** A message a Smart UI view sends (`rustcode.send`), as the user's next message, tagged "from view". */
export function sendFromView(id: string, text: string) {
  const thread = get().threads[id];
  if (!thread || !connection) return;
  if (thread.running) {
    updateThread(id, (t) => ({ ...t, queue: [...t.queue, { id: localId("queued"), text, fromView: true }] }));
    toast("Queued: the view's message goes when this turn ends");
    return;
  }
  void runPrompt(id, text, emptyExtras, { fromView: true });
}

export function setTerminalOpen(open: boolean) {
  set({ terminalOpen: open });
}

/** The panel last shown, so ⌘⌥B brings it back. */
let lastPanel: NonNullable<AppState["panel"]> = "changes";

/**
 * The view to show: the one last shown, when this project or thread offers it
 * (a project without pull requests has no Pull requests tab, and the Thread
 * tab needs a thread to show).
 */
function rememberedPanel(s: AppState): NonNullable<AppState["panel"]> {
  if (lastPanel === "pulls" && !s.agent?.pullRequests) return "changes";
  if (lastPanel === "peek" && !s.peekId) return "changes";
  return lastPanel;
}

/**
 * The panel a thread opens with: the view last shown, unless the user closed
 * the panel on this very thread; a new thread has none.
 */
function panelForThread(s: AppState, id: string | null): AppState["panel"] {
  if (!id || s.panelClosedFor === id) return null;
  return rememberedPanel(s);
}

/**
 * The state a thread opens with: selected, on the panel it last showed, and
 * the close it was given by hand kept on it alone (another thread opens the
 * panel again).
 */
function viewingThread(
  s: AppState,
  id: string | null,
): Pick<AppState, "selectedId" | "panel" | "panelClosedFor" | "peekId"> {
  // The Thread tab shows another thread beside this one: opening the thread it
  // shows ends the peek, so the panel goes back to Changes.
  const shown = id && s.peekId === id ? { ...s, peekId: null } : s;
  return {
    selectedId: id,
    panel: panelForThread(shown, id),
    panelClosedFor: s.panelClosedFor === id ? id : null,
    peekId: shown.peekId,
  };
}

/** Opens `panel`, or closes it when it is the one open (the header's buttons). */
export function setPanel(panel: AppState["panel"]) {
  if (panel) lastPanel = panel;
  set((s) => {
    const next = s.panel === panel ? null : panel;
    // Closing by hand is kept for the thread on screen, so the next auto-open
    // (picking the same thread again) leaves it closed.
    return { panel: next, panelClosedFor: next ? null : s.selectedId };
  });
}

/** Shows a thread in the side panel, next to the one you are in. */
export function peekThread(id: string) {
  set({ peekId: id });
  showPanel("peek");
  void openThread(id);
}

/** Shows `panel` (the panel's tabs): never closes. */
export function showPanel(panel: NonNullable<AppState["panel"]>) {
  lastPanel = panel;
  set({ panel, panelClosedFor: null });
}

/** ⌘⌥B: the right panel open or closed, with what it showed last. */
export function toggleRightPanel() {
  set((s) => ({
    panel: s.panel ? null : rememberedPanel(s),
    panelClosedFor: s.panel ? s.selectedId : null,
  }));
}

/** Whether the thread's harness lists subagents (`_rustcode/agents`). */
export function hasAgents(thread?: ThreadState): boolean {
  return !!get().agent?.agents && (thread?.harness?.capabilities.subagents ?? true);
}

export async function refreshAgents(id: string): Promise<SubagentInfo[]> {
  const thread = get().threads[id];
  if (!connection || !hasAgents(thread)) return [];
  try {
    const agents = parseAgentList(
      await connection.request(Method.agents, { sessionId: id, action: "list" }, 10_000),
    );
    set((s) => ({ agents: { ...s.agents, [id]: agents } }));
    return agents;
  } catch {
    return get().agents[id] ?? [];
  }
}

/** A child's conversation, folded like a replayed thread. */
export async function agentHistory(id: string, agentId: number): Promise<ThreadState> {
  const result = (await engine().request(
    Method.agents,
    { sessionId: id, action: "history", id: agentId },
    15_000,
  )) as JsonObject | null;
  let state = emptyThread(`${id}#${agentId}`, get().threads[id]?.cwd ?? "");
  for (const update of Array.isArray(result?.updates) ? result.updates : []) {
    state = applyUpdate(state, parseSessionUpdate({ sessionId: id, update }));
  }
  return finishLoad(state);
}

let agentTimer: ReturnType<typeof setInterval> | null = null;

/** Poll the shown thread's agents while its turn runs or a child works, as the SwiftUI app does. */
export function startAgentPolling() {
  if (agentTimer) return;
  agentTimer = setInterval(() => {
    const s = get();
    const id = s.selectedId;
    const thread = id ? s.threads[id] : undefined;
    if (!id || !thread || s.status !== "ready" || !hasAgents(thread)) return;
    const known = s.agents[id];
    if (thread.running || known?.some(agentActive) || (known === undefined && thread.loaded)) {
      void refreshAgents(id);
    }
  }, 2000);
}

const planUsageInFlight = new Set<string>();

/**
 * The harness a thread runs on, as far as the app knows: its own record once
 * opened, else the summary from the list. With no thread, the one the next
 * thread will start on.
 */
export function harnessOf(
  s: Pick<AppState, "threads" | "sessions" | "newThread" | "harnesses">,
  threadId?: string,
): string | undefined {
  if (!threadId) return s.newThread?.harness ?? s.harnesses?.default.harness ?? "rustcode";
  return s.threads[threadId]?.harness?.id ?? s.sessions.find((x) => x.id === threadId)?.harness;
}

/**
 * The plan-cache key of a thread's login: its harness with the account id
 * when it runs on a named one. A new thread uses the account picked for it.
 */
export function planKeyFor(
  s: Pick<
    AppState,
    "threads" | "sessions" | "newThread" | "newThreadAccount" | "harnesses" | "signedOutLogins"
  >,
  threadId?: string,
): string | undefined {
  const harness = harnessOf(s, threadId);
  if (!harness) return undefined;
  return harnessPlanKey(harness, accountOn(s, threadId));
}

/**
 * The named account a thread runs on, null for its harness's default login: its
 * own record once opened, else the list's summary of it. With no thread, the
 * account the next thread will start on.
 */
export function accountOn(
  s: Pick<
    AppState,
    "threads" | "sessions" | "newThread" | "newThreadAccount" | "harnesses" | "signedOutLogins"
  >,
  threadId?: string,
): string | null {
  if (!threadId) {
    const harness = harnessOf(s);
    const known = s.harnesses?.harnesses.find((h) => h.id === harness);
    return newThreadAccountOn(known, s.newThreadAccount, s.signedOutLogins);
  }
  const own = s.threads[threadId]?.harness?.account;
  if (own !== undefined) return own ?? null;
  return s.sessions.find((x) => x.id === threadId)?.account ?? null;
}

/** The state `commandsFor` reads. */
type CommandsSource = Pick<
  AppState,
  "threads" | "sessions" | "newThread" | "harnesses" | "baseCommands" | "harnessCommands" | "sessionCommands"
>;

/**
 * The slash commands a composer offers: an open thread reads its session's own list, a new one the
 * menu of the harness picked there. Until a thread's session has advertised (or for a new thread),
 * the menu is the engine's catalog with that harness's last list over it, so switching between
 * them never leaves another harness's commands behind.
 */
export function commandsFor(s: CommandsSource, threadId?: string | null): SlashCommand[] {
  const own = threadId ? s.sessionCommands[threadId] : undefined;
  if (own) return own;
  const harness = harnessOf(s, threadId ?? undefined);
  const known = s.harnesses?.harnesses.find((h) => h.id === harness);
  return menuCommands({
    base: s.baseCommands,
    seen: harness ? s.harnessCommands[harness] : undefined,
    // Before the harnesses are listed the engine answers the thread, so its catalog applies.
    builtin: known?.builtin ?? (!s.harnesses || harness === undefined || harness === "rustcode"),
  });
}

/** How often the app asks for every plan while its window is visible. */
export const PLAN_CACHE_POLL_MS = 180_000;

/** True when a harness's numbers are missing or older than the poll interval. */
export function planIsStale(plan: CachedPlan | undefined, now = Date.now()): boolean {
  return !plan?.fetchedAt || now - plan.fetchedAt >= PLAN_CACHE_POLL_MS;
}

/** True when anything in the cache wants a refresh: an entry past the poll, or none at all. */
export function planCacheStale(cache: Record<string, CachedPlan>, now = Date.now()): boolean {
  const entries = Object.values(cache);
  return !entries.length || entries.some((p) => planIsStale(p, now));
}

/** The persisted cache without entries from more than a day ago. */
export function prunePlans(cache: Record<string, CachedPlan>, now = Date.now()): Record<string, CachedPlan> {
  const kept = Object.entries(cache).filter(([, p]) => now - (p.fetchedAt ?? 0) < PLAN_CACHE_MAX_AGE_MS);
  return kept.length === Object.keys(cache).length ? cache : Object.fromEntries(kept);
}

/** The cache without the harnesses a plans answer proved signed out or not installed. */
export function forgetPlans(
  cache: Record<string, CachedPlan>,
  harnesses: string[],
): Record<string, CachedPlan> {
  const gone = harnesses.filter((h) => cache[h]);
  if (!gone.length) return cache;
  const next = { ...cache };
  for (const h of gone) delete next[h];
  return next;
}

/**
 * A window whose reset passed since these numbers were read as reset: nothing
 * used, and the stale reset time dropped, until the next read says otherwise.
 */
export function windowNow(w: UsageWindow, now = Date.now()): UsageWindow {
  return w.resetsAt && w.resetsAt <= now ? { usedPercent: 0, minutes: w.minutes } : w;
}

/** The plan with every passed reset folded in; undefined when it has no windows. */
export function planNow(plan: PlanUsage | null | undefined, now = Date.now()): PlanUsage | undefined {
  if (!plan?.windows.length) return undefined;
  const windows = plan.windows.map((w) => windowNow(w, now));
  return windows.some((w, i) => w !== plan.windows[i]) ? { ...plan, windows } : plan;
}

/**
 * Folds fresh numbers into the cache, one entry per harness: the newer of the
 * two wins, so a slow plans answer never overwrites a thread's newer limits,
 * and an answer without windows (a harness with no plan) never erases one.
 */
export function mergePlans(
  cache: Record<string, CachedPlan>,
  incoming: Record<string, CachedPlan>,
): Record<string, CachedPlan> {
  let next = cache;
  for (const [harness, plan] of Object.entries(incoming)) {
    if (!plan.windows.length) continue;
    const known = next[harness];
    if (known && (known.fetchedAt ?? 0) >= (plan.fetchedAt ?? 0)) continue;
    if (next === cache) next = { ...cache };
    next[harness] = plan;
  }
  return next;
}

/** The plans answer as cache entries, for each signed-in harness that reported windows. */
export function plansToCache(plans: UsagePlan[], fetchedAt = Date.now()): Record<string, CachedPlan> {
  const out: Record<string, CachedPlan> = {};
  for (const p of plans) {
    if (p.status !== "ok" || !p.windows.length) continue;
    const harness = p.harness ?? planHarness(p.id);
    out[planKey(p)] = {
      source: harness,
      windows: p.windows.map((w) => ({
        usedPercent: w.usedPercent,
        minutes: w.minutes,
        resetsAt: w.resetsAt,
      })),
      fetchedAt,
      name: p.name,
      plan: p.plan,
      harness,
      account: p.account ?? null,
    };
  }
  return out;
}

/**
 * The one plan in the cache, for a thread whose harness the engine did not
 * report: with a single subscription signed in, it is the one in use. With
 * several, guessing would show another harness's numbers, so none is used.
 */
export function onlyCachedPlan(cache: Record<string, CachedPlan>): CachedPlan | undefined {
  const plans = Object.values(cache).filter((p) => p.windows.length);
  return plans.length === 1 ? plans[0] : undefined;
}

/**
 * The plan the composer's meter shows. A thread's own limits are for the account
 * it runs on, so they win whenever it has any; the harness's cache (marked
 * `cached`) fills in where the engine answers none, as it does for Claude Code.
 */
export function shownPlan(
  own: PlanUsage | null | undefined,
  cached: CachedPlan | undefined,
  now = Date.now(),
): { plan: PlanUsage | undefined; cached: boolean } {
  const live = planNow(own, now);
  if (live) return { plan: live, cached: false };
  const known = planNow(cached, now);
  return { plan: known, cached: !!known };
}

/** Writes a cache into the store and app storage; no-op when it is the same map. */
function setPlanCache(next: Record<string, CachedPlan>) {
  if (next === get().planCache) return;
  set({ planCache: next });
  store(STORAGE.planCache, JSON.stringify(next));
  schedulePlanReset(next);
}

/** Writes merged numbers into the store and app storage; no-op when nothing is newer. */
function commitPlans(incoming: Record<string, CachedPlan>) {
  setPlanCache(mergePlans(get().planCache, incoming));
}

/**
 * Asks the engine for the plan's usage windows. One request per thread at a
 * time; a failed one keeps the last good numbers (marked stale), and only an
 * answer without windows (the harness has no plan usage) clears them.
 */
export async function refreshPlanUsage(id: string): Promise<void> {
  if (!connection || planUsageInFlight.has(id)) return;
  planUsageInFlight.add(id);
  try {
    const usage = parsePlanUsage(await connection.request(Method.usageLimits, { sessionId: id }, 10_000));
    const fetchedAt = Date.now();
    set((s) => ({ planUsage: { ...s.planUsage, [id]: usage && { ...usage, fetchedAt } } }));
    // The same numbers serve every thread on that harness; a thread that has
    // none of its own (Claude Code's limits answer null) reads it from the cache.
    // A native thread's numbers belong to the provider it runs on, which the
    // harness id does not name (`rustcode`): they stay on the thread, so no
    // other native thread reads them as its own.
    const harness = harnessOf(get(), id);
    if (usage && harness && harness !== "rustcode") {
      // The account's own row, so it never overwrites another login's numbers.
      const key = harnessPlanKey(harness, accountOn(get(), id));
      commitPlans({
        [key]: { ...usage, fetchedAt, source: harness, harness },
      });
    }
  } catch {
    set((s) => {
      const last = s.planUsage[id];
      return last ? { planUsage: { ...s.planUsage, [id]: { ...last, stale: true } } } : s;
    });
  } finally {
    planUsageInFlight.delete(id);
  }
}

let plansInFlight: PlansAsk | null = null;
/** Asks that arrived while one was in flight: run after it, coalesced (state/plans.ts). */
let plansQueued: PlansAsk[] = [];
/**
 * The Claude logins the background read leaves alone: the default login, and
 * each account whose Keychain read was denied (or which is signed out), so a
 * Deny never prompts every three minutes. Settings' own look clears them. An
 * expired sign-in is not here: Claude Code renews it, and the background read
 * must notice that without the user opening Settings.
 */
let planPauses: PlanPauses = NO_PAUSES;

/** The consent Settings › Usage keeps before Claude's sign-in is read from the Keychain. */
export const CLAUDE_USAGE_KEY = "rustcode.claudeUsage";

/** Off until the user says yes: reading the Keychain is theirs to allow. */
export const CLAUDE_USAGE_DEFAULT = false;

export function claudeUsageAllowed(): boolean {
  const raw = stored(CLAUDE_USAGE_KEY);
  return raw === null ? CLAUDE_USAGE_DEFAULT : raw === "1";
}

/**
 * Turns that consent on or off. Off forgets the numbers it read: the composer
 * must not keep showing a plan the user asked the app to stop reading.
 */
export function setClaudeUsageAllowed(on: boolean): void {
  store(CLAUDE_USAGE_KEY, on ? "1" : "0");
  planPauses = NO_PAUSES;
  if (on) return;
  // Every Claude login, the default one and each account's own row.
  setPlanCache(
    Object.fromEntries(Object.entries(get().planCache).filter(([key]) => !key.startsWith("claude-code"))),
  );
  // What a read found about a login is only kept while it is read: with the
  // meter off nothing would clear it after the user signed in again.
  set({
    signedOutLogins: Object.fromEntries(
      Object.entries(get().signedOutLogins).filter(([key]) => !key.startsWith("claude-code")),
    ),
  });
}

/**
 * Folds a plans answer into the cache: a signed-in harness's windows go in, one
 * that is signed out or gone loses its entry, and a Claude login that could not
 * be read stops the background poll asking for it again (a Deny in the Keychain
 * must not prompt every three minutes). An answer may hold only some rows, so
 * nothing it did not read is touched. `asked` is Settings' own call, which is
 * what unpauses them.
 */
export function commitPlansAnswer(answers: UsagePlan[], asked = false): void {
  if (asked) planPauses = NO_PAUSES;
  const gone = answers.filter((p) => p.status === "signedOut" || p.status === "notInstalled").map(planKey);
  const kept = forgetPlans(get().planCache, gone);
  const learned = plansToCache(answers);
  setPlanCache(mergePlans(kept, learned));
  // Which logins are signed out, for the places that must not offer one: the
  // picker's "Default login" and a new thread's account (state/logins.ts).
  set({ signedOutLogins: signedOutFrom(answers, get().signedOutLogins) });
  // A login whose read was denied, or that is signed out, pauses its own row
  // only: the default login's failure must not stop a named account's meter,
  // and an account without a sign-in must not stop the default login's.
  planPauses = pausesAfter(answers, planPauses);
}

/**
 * What a refresh asks the engine for: the default login and the account of the
 * thread on screen, each only with consent and while it was not paused.
 */
function currentPlansAsk(explicit: boolean): PlansAsk {
  const s = get();
  const threadId = s.selectedId ?? undefined;
  return plansAskFor({
    allowed: claudeUsageAllowed(),
    harness: harnessOf(s, threadId),
    account: accountOn(s, threadId),
    pauses: planPauses,
    explicit,
  });
}

/**
 * Asks for every subscription's plan (`_rustcode/usage/plans`, no thread
 * needed) and folds the answer into the cache. Claude is asked for only with
 * consent: the default login, and the account of the thread on screen so its
 * meter has numbers without a visit to Settings › Usage, one Keychain item at a
 * time. One request at a time: anything asked for while one is in flight runs
 * after it, coalesced, so a hover during the poll is not dropped. A failed one
 * keeps the last numbers for the next poll. An explicit check (`again`) bypasses
 * the engine's minute-long cache, so a sign-in renewed in Claude Code shows at
 * once instead of after that minute.
 */
export async function refreshPlanCache(again = false): Promise<void> {
  if (!connection || get().status !== "ready") return;
  const ask = currentPlansAsk(again);
  // An explicit check asks again even after an answer paused a login.
  if (again) planPauses = NO_PAUSES;
  if (plansInFlight) {
    plansQueued = queueAsks(plansQueued, ask);
    return;
  }
  let next: PlansAsk | undefined = ask;
  while (next && connection) {
    plansInFlight = next;
    try {
      const result = await connection.request(
        Method.usagePlans,
        {
          claude: next.claude,
          ...(next.account ? { claudeAccount: next.account } : {}),
          // An explicit look reads the Keychain and Anthropic again rather
          // than answer from the engine's minute of cache (state/plans.ts).
          ...(next.force ? { force: true } : {}),
        },
        20_000,
      );
      commitPlansAnswer(parsePlans(result));
    } catch {
      // Cached numbers stay; the next poll tries again.
    } finally {
      plansInFlight = null;
    }
    next = plansQueued.shift();
  }
}

let planTimer: ReturnType<typeof setInterval> | null = null;
let planResetTimer: ReturnType<typeof setTimeout> | null = null;
let planResetDue = 0;
let plansOnVisible: (() => void) | null = null;

/**
 * Asks once more a few seconds after the soonest cached window resets, when
 * that would otherwise be missed until the poll after next: a new window shows
 * as it starts. Every poll re-arms it, and an earlier deadline already armed is
 * kept, so a poll never pushes the reset back.
 */
function schedulePlanReset(cache: Record<string, CachedPlan>) {
  const now = Date.now();
  const windows = Object.values(cache).flatMap((p) => p.windows);
  const delay = nextPlanRefresh({ source: "plan", windows }, now);
  if (delay >= PLAN_CACHE_POLL_MS) return;
  const due = now + delay;
  if (planResetTimer && planResetDue <= due) return;
  if (planResetTimer) clearTimeout(planResetTimer);
  planResetDue = due;
  planResetTimer = setTimeout(() => {
    planResetTimer = null;
    planResetDue = 0;
    if (typeof document !== "undefined" && document.hidden) return;
    void refreshPlanCache();
  }, delay);
}

/**
 * One poller for the app, not one per thread: every three minutes while the
 * window is visible, and at once when it comes back after being hidden for
 * longer than that. A hidden window asks for nothing.
 */
export function startPlanPolling() {
  if (planTimer) return;
  planTimer = setInterval(() => {
    if (typeof document !== "undefined" && document.hidden) return;
    void refreshPlanCache();
  }, PLAN_CACHE_POLL_MS);
  if (typeof document === "undefined") return;
  plansOnVisible = () => {
    if (document.hidden) return;
    // Back after a while: catch up now rather than at the next tick.
    if (planCacheStale(get().planCache)) void refreshPlanCache();
  };
  document.addEventListener("visibilitychange", plansOnVisible);
}

function stopPlanPolling() {
  if (planTimer) clearInterval(planTimer);
  planTimer = null;
  if (planResetTimer) clearTimeout(planResetTimer);
  planResetTimer = null;
  planResetDue = 0;
  planPauses = NO_PAUSES;
  if (plansOnVisible) document.removeEventListener("visibilitychange", plansOnVisible);
  plansOnVisible = null;
  plansInFlight = null;
  plansQueued = [];
}

// ---------------------------------------------------------------- UI

/**
 * Opens Settings on a page (null closes it). An id from an older build still
 * lands somewhere sensible, so a caller that names Harnesses or Engine works.
 */
export function openSettings(section: SettingsSection | string | null) {
  dismissOverlays();
  set({ settings: settingsSection(section), switcherOpen: false });
}

export function setSwitcher(open: boolean) {
  set({ switcherOpen: open });
}

export function setOnboarding(open: boolean) {
  set({ onboardingOpen: open, settings: open ? null : get().settings });
}

export function toggleSidebar() {
  const hidden = !get().sidebarHidden;
  set({ sidebarHidden: hidden });
  store(STORAGE.sidebar, hidden ? "1" : "0");
}

export function setTheme(theme: AppState["theme"]) {
  set({ theme });
  store(STORAGE.theme, theme);
}

/** Generic request for Settings pages (capabilities, connections). */
export async function request<T = Json>(
  method: string,
  params: JsonObject = {},
  timeoutMs = 15_000,
): Promise<T> {
  return engine().request<T>(method, params, timeoutMs);
}

/** Test hook: reset to a clean slate. */
export function resetForTests() {
  // The module's remembered panel view is state too: a test starts from the default.
  lastPanel = "changes";
  clearTimeout(slowTimer);
  if (limitTimer) clearInterval(limitTimer);
  limitTimer = null;
  clearPageCache();
  stopPlanPolling();
  resetWaits();
  engineWatch?.();
  engineWatch = undefined;
  crashed = false;
  modelsAsked.clear();
  connection = null;
  transport = null;
  setShellTransport(null);
  loading.clear();
  set({
    status: "connecting",
    engineLog: [],
    sessions: [],
    sessionsLoaded: false,
    sessionsCapped: false,
    threads: {},
    selectedId: null,
    projectCwd: "",
    newThread: null,
    harnesses: null,
    baseCommands: [],
    harnessCommands: {},
    sessionCommands: {},
    drafts: {},
    extras: {},
    settings: null,
    switcherOpen: false,
    onboardingOpen: false,
    toasts: [],
    toastsPaused: false,
    limitWaits: {},
    continueAfterLimit: true,
    agent: undefined,
    pinned: [],
    archived: {},
    editing: {},
    confirm: null,
    panel: null,
    panelClosedFor: null,
    agents: {},
    planUsage: {},
    planCache: {},
    signedOutLogins: {},
    orchestration: false,
    orchestrationPending: false,
    readAcrossProjects: false,
    links: { links: [], grants: {}, counters: {}, fanOuts: [] },
    grant: null,
    features: {},
    themes: null,
    themeActive: "rustcode",
    statusMarks: {},
    snoozed: {},
    snoozeAsk: null,
    defaultPermissionMode: "ask",
    permissionModes: {},
    newThreadPermissionMode: null,
    view: { kind: "chat" },
    checkpoints: {},
    newWorktree: { on: false, branch: "", edited: false },
    pendingSetup: null,
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

// ---------------------------------------------------------------- continuing after a limit

function storeLimitWaits(next: Record<string, LimitWait>) {
  set({ limitWaits: next });
  store(STORAGE.limitWaits, JSON.stringify(next));
  syncLimitTimer();
}

/** The user's call, or something they did that makes the wait moot: no turn goes out for it. */
export function dropLimitWait(id: string) {
  const { [id]: gone, ...rest } = get().limitWaits;
  if (gone) storeLimitWaits(rest);
}

/** The time a wait ends, as the row and the toast say it. */
export function limitTime(until: number, now = Date.now()): string {
  const at = new Date(until);
  const time = at.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
  if (new Date(now).toDateString() === at.toDateString()) return time;
  return `${at.toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric" })}, ${time}`;
}

/**
 * A turn ended: on a limit it waits for the reset, on anything else the wait is
 * over. The limit is read from the failure or, as Claude Code says it, from the
 * reply. A stop by the user always ends it. Returns the wait a limit started.
 */
function settleLimitWait(
  id: string,
  outcome: "completed" | "cancelled" | "failed",
  failure: string | undefined,
  startIndex: number,
): LimitWait | null {
  const s = get();
  const previous = s.limitWaits[id];
  if (!s.continueAfterLimit || outcome === "cancelled") {
    dropLimitWait(id);
    return null;
  }
  const reply = s.threads[id]?.items.slice(startIndex).findLast((i) => i.kind === "assistant");
  const said = failure ?? (reply?.kind === "assistant" ? reply.text : undefined);
  const key = planKeyFor(s, id);
  const windows = (key ? s.planCache[key]?.windows : undefined) ?? [];
  const now = Date.now();
  const until = limitReset(said, now, windows);
  if (until === null) {
    dropLimitWait(id);
    return null;
  }
  const wait = nextWait(previous, { until, harness: harnessOf(s, id), account: accountOn(s, id) });
  if (!wait) {
    dropLimitWait(id);
    return null;
  }
  storeLimitWaits({ ...get().limitWaits, [id]: wait });
  return wait;
}

let limitTimer: ReturnType<typeof setInterval> | null = null;

/** One slow timer while anything waits: a Mac that slept wakes past the reset and still continues. */
function syncLimitTimer() {
  const waiting = Object.keys(get().limitWaits).length > 0;
  if (waiting && !limitTimer) limitTimer = setInterval(() => void continueDueThreads(), 15_000);
  else if (!waiting && limitTimer) {
    clearInterval(limitTimer);
    limitTimer = null;
  }
}

let continuing = false;

/** Each thread whose limit has reset carries on, unless it now runs on something else. */
export async function continueDueThreads(now = Date.now()): Promise<void> {
  if (continuing || get().status !== "ready") return;
  continuing = true;
  try {
    for (const [id, wait] of Object.entries(get().limitWaits)) {
      if (!get().sessions.some((x) => x.id === id) || get().archived[id]) {
        if (get().sessionsLoaded) dropLimitWait(id);
        continue;
      }
      const holds = () => {
        const s = get();
        return waitHolds(wait, { harness: harnessOf(s, id), account: accountOn(s, id) });
      };
      // Switched already: another harness or login is not this limit, so nothing starts on its own.
      if (!holds()) {
        dropLimitWait(id);
        continue;
      }
      if (!waitDue(wait, now)) continue;
      await openThread(id);
      for (let i = 0; i < 100 && !get().threads[id]?.loaded; i++) {
        await new Promise((r) => setTimeout(r, 100));
      }
      const thread = get().threads[id];
      if (!thread?.loaded || thread.running) continue;
      if (!holds()) {
        dropLimitWait(id);
        continue;
      }
      void runPrompt(id, LIMIT_RESUME_TEXT, undefined, { automatic: true, limitResume: true });
    }
  } finally {
    continuing = false;
  }
}

export function setContinueAfterLimit(on: boolean) {
  set({ continueAfterLimit: on });
  store(STORAGE.continueAfterLimit, on ? "1" : "0");
  if (!on) storeLimitWaits({});
}

// ---------------------------------------------------------------- resuming after a restart

/** What an interrupted thread is told when it continues. */
export const RESUME_TEXT =
  "The app restarted while you were working, so your last turn stopped. Continue where you left off.";
/** At most this many threads continue at once after a restart. */
const RESUME_MAX = 8;

const runningIds = (s: AppState) =>
  Object.values(s.threads)
    .filter((t) => t.running)
    .map((t) => t.id)
    .sort();

// The running threads are written while the engine is up, so a quit, a crash, an update or an
// engine restart leaves the list of turns it cut short. A turn that ends takes itself off it.
let lastRunning = "";
useApp.subscribe((s) => {
  if (s.status !== "ready") return;
  const ids = runningIds(s).join("\n");
  if (ids === lastRunning) return;
  lastRunning = ids;
  store(STORAGE.runningThreads, JSON.stringify(ids ? ids.split("\n") : []));
});

/** The threads a restart cut off: the saved list, plus any this app still showed running. */
function interruptedThreads(): string[] {
  const saved = storedJson<unknown>(STORAGE.runningThreads, []);
  const ids = new Set([
    ...(Array.isArray(saved) ? saved.filter((x): x is string => typeof x === "string") : []),
    ...runningIds(get()),
  ]);
  return [...ids];
}

/**
 * Each thread a restart cut off is opened and sent {@link RESUME_TEXT}, so it carries on
 * without the user pressing anything. Settings › General turns it off.
 */
async function resumeInterrupted(ids: string[]): Promise<void> {
  store(STORAGE.runningThreads, "[]");
  lastRunning = "";
  if (!get().resumeAfterRestart || !ids.length) return;
  const known = new Set(get().sessions.map((x) => x.id));
  const resumable = ids.filter((id) => known.has(id)).slice(0, RESUME_MAX);
  for (const id of resumable) {
    await openThread(id);
    // The selected thread may already be loading from the handshake: wait for it.
    for (let i = 0; i < 100 && !get().threads[id]?.loaded; i++) await new Promise((r) => setTimeout(r, 100));
    const thread = get().threads[id];
    if (!thread?.loaded || thread.running) continue;
    void runPrompt(id, RESUME_TEXT, undefined, { automatic: true });
  }
  if (resumable.length) {
    toast(
      resumable.length === 1
        ? "A thread the restart interrupted is continuing."
        : `${resumable.length} threads the restart interrupted are continuing.`,
      "success",
    );
  }
}
