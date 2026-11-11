// App state (Zustand) and the engine controller that drives it over ACP.
// Components read slices with `useApp(selector)` and call the actions below.

import { create } from "zustand";
import { AcpConnection, type Inbound, type Json, type JsonObject, obj, RpcError, str } from "../acp/jsonrpc";
import { parseRemoteStatus, RemoteMethod, type RemoteStatus } from "../acp/remote";
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
  delegateParent,
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
  type SessionUsage,
  type SlashCommand,
  type ThreadRelationship,
} from "../acp/types";
import {
  type Checkpoint,
  checkpointsByTurn,
  type PatchFile,
  parseCheckpoints,
  parseRestoreAnswer,
  parseTurnDiff,
  parseWorktree,
  parseWorktreeList,
  type RestoreAnswer,
  removeRefusal,
  statusInUpdate,
  type ThreadStatus,
  V2Method,
  type Worktree,
} from "../acp/v2";
import { approvalSubject, optionScope, resolvedLine } from "../lib/approval";
import { CHAT_WIDTH_DEFAULT, type ChatWidth, parseChatWidth } from "../lib/chatWidth";
import { capServerLogLine } from "../lib/diagnostics";
import { afterPaint } from "../lib/frame";
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
import { type Attempt, splitAtTurn, transcriptTurns } from "./attempts";
import {
  type AutoCompactMode,
  type AutoCompactSettings,
  autoCompactDecision,
  autoCompactingText,
  autoCompactMark,
  autoCompactPolicy,
  automaticCompactionText,
  CONTINUE_AFTER_COMPACTION_TEXT,
  clampAutoCompactThreshold,
  DEFAULT_AUTO_COMPACT,
  parseAutoCompactSettings,
  shouldContinueAfterCompaction,
} from "./autoCompact";
import { createBatcher, frameQueue } from "./batch";
import {
  type HarnessCommands,
  menuCommands,
  parseHarnessCommands,
  rememberCommands,
  serializeHarnessCommands,
  withSkills,
} from "./commands";
import {
  type Attachment,
  type ComposeExtras,
  carriesExtras,
  composedText,
  emptyExtras,
  hasExtras,
  promptBlocks,
  type SentAttachment,
  sentAttachments,
  splitImageMarkers,
} from "./compose";
import type { ConnectCode } from "./connectRequests";
import { contextPercent } from "./details";
import { DRAFTS_KEY, parseDrafts, persistable, withDraft } from "./drafts";
import { parseGrants, type TimedGrant } from "./grants";
import {
  type EffortMemory,
  normalizeDefault,
  preferredDefault,
  restoreChoice,
  selectOption,
  unavailableReason,
} from "./harness";
import {
  DEFAULT_HEADER_BUTTONS,
  HEADER_BUTTON_IDS,
  type HeaderButtonId,
  sanitizeHeaderButtons,
} from "./headerButtons";
import {
  answerOrchestration,
  interruptedLinks,
  linkFinished,
  linkStarted,
  linksFinished,
  linksOf,
  linksResumed,
  linkWaitEnded,
  linkWaitStarted,
  loadBook,
  orchestrationCallDropped,
  queuedLinkDropped,
  resetWaits,
  restoreQueuedThreads,
  stopLinksFrom,
  syncLinkBook,
  threadTurnEnded,
  wakeDelivered,
  wakeTick,
} from "./host";
import type { InboxItem } from "./inbox";
import { dismissRows, type InboxDismissals, restoreRows } from "./inboxDismissed";
import { LIMIT_RESUME_TEXT, type LimitWait, limitReset, nextWait, waitDue, waitHolds } from "./limitResume";
import { newThreadAccountOn, type SignedOutLogins, signedOutFrom } from "./logins";
import { type NotifyEvent, notifyThread, startNotifications, wakeEvent } from "./notify";
import {
  CAPABILITY_VERSION,
  DEFAULT_DELEGATES_PER_THREAD,
  DEFAULT_DELEGATES_TOTAL,
  failure,
  failureJson,
  forget,
  type GrantAsk,
  isActive,
  type LinkBook,
  MAX_DELEGATES_PER_THREAD,
  MAX_DELEGATES_TOTAL,
  MIN_DELEGATES_PER_THREAD,
  MIN_DELEGATES_TOTAL,
} from "./orchestration";
import { clearPageCache } from "./pageCache";
import { autoAnswer, effectiveMode, type PermissionMode, parsePermissionMode } from "./permissions";
import { NO_PAUSES, type PlanPauses, type PlansAsk, pausesAfter, plansAskFor, queueAsks } from "./plans";
import { projectChoice } from "./projectBook";
import { matchProject, projectAt, projectsOn } from "./projects";
import { deliverPullWatch, PULL_WATCH, subscribePullWatches } from "./pullWatch";
import {
  nextQueued,
  QUEUE_REASON,
  type QueueReason,
  type SendDecision,
  type SteeringCapabilities,
  sendDecision,
  type TurnState,
} from "./queue";
import { parseRecapHours, RECAP_HOURS_DEFAULT } from "./recap";
import { challengeIsValid, makeChallenge, newPairedDevice, type PairingChallenge } from "./remotePairing";
import { beginSetup, dismissSetup, type PendingSetup, setupFailed, setupSucceeded } from "./setup";
import { keepThreads, type SidebarOrder, withoutThreads } from "./sidebarOrder";
import { dueSnoozes } from "./snooze";
import {
  appendSteer,
  applyUpdate,
  beginQuietTurn,
  beginTurn,
  departQueued,
  dropDeparted,
  emptyThread,
  finishLoad,
  finishTurn,
  foldingBatch,
  localId,
  messagesAfter,
  type QueuedPrompt,
  questionItem,
  requeueSteers,
  settleCompaction,
  settledWait,
  settleEveryWait,
  startCompaction,
  type ThreadState,
  type TranscriptItem,
  truncateBefore,
  userIndexOf,
  waitingOn,
  wakeTurn,
  withContinueMark,
} from "./thread";
import { isRemoteThread, LOCAL_HOST, splitThreadKey, threadKey } from "./threadKeys";
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
import { userTypedPrompt } from "./topLevel";
import { readableTurnError } from "./turnError";

export type { Toast, ToastTone } from "./toasts";

export const APP_VERSION = "0.1.0";

export type SettingsSection =
  | "general"
  | "chat"
  | "agents"
  | "connections"
  | "capabilities"
  | "remote"
  | "machines"
  | "appearance"
  | "keyboard"
  | "usage"
  | "statistics"
  | "advanced"
  | "diagnostics"
  | "about";

/** Every Settings page, in the order the panel's nav lists them. */
export const SETTINGS_SECTIONS: readonly SettingsSection[] = [
  "general",
  "chat",
  "agents",
  "connections",
  "capabilities",
  "remote",
  "machines",
  "appearance",
  "keyboard",
  "usage",
  "statistics",
  "advanced",
  "diagnostics",
  "about",
];

/**
 * What an older build's section id opens: the Harnesses page keeps its old
 * `agents` id, so a stored `harnesses` opens it, and Engine became Advanced.
 * An id no build knows opens General.
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
  /**
   * When the numbers were read (ms) when they are the last ones RustCode
   * recorded for the login rather than a read made now: its access token
   * lapsed, so `fetchedAt` is when the app heard them and this is when they
   * were true (`list_harnesses` reports it as `as_of`).
   */
  asOf?: number;
}

export interface AppState {
  status: "connecting" | "ready" | "closed";
  statusDetail?: string;
  transport?: Transport["name"];
  engineLog: string[];
  agent?: AgentInfo;
  sessions: SessionSummary[];
  /** Each paired machine's name, by host id: the sidebar, the header and a notice say where. */
  machineNames: Record<string, string>;
  /** The paired machines whose connection is open now: their threads can be driven, not only read. */
  connectedHosts: string[];
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
  /**
   * The computer the next new thread runs on: `LOCAL_HOST` for this one, or a paired
   * machine's id (`state/machines.ts`). The composer's **Run on** sets it, and a new
   * thread started from a machine's group in the sidebar starts there
   * (docs/desktop.md › Several machines).
   */
  newThreadHost: string;
  /** The sidebar device being browsed; null is local, "devices" is the picker. */
  sidebarDevice: string | null;
  /**
   * Each paired machine's harnesses, models, accounts and defaults, by host id, read over
   * its own connection. A machine's snapshot is read when the **Run on** picker moves
   * there, and kept while it is saved, so its harness list does not blink between threads.
   */
  remoteHarnesses: Record<string, HarnessSnapshot>;
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
  /**
   * The order each sidebar section draws its rows in, by section key, as the user
   * arranged it (state/sidebarOrder.ts). Kept in app storage beside `pinned`.
   */
  sidebarOrder: SidebarOrder;
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
   * The panel each thread opens with, by thread id: the view it was last
   * showing, or null when it was closed by hand. A thread never seen has no
   * entry and opens closed, and nothing opens the panel on its own.
   */
  panelByThread: Record<string, PanelKind | null>;
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
  /** Threads can start and message other threads (Settings › Harnesses); the engine learns it at initialize. */
  orchestration: boolean;
  /** How many delegates one thread may run at once (Settings › Harnesses). */
  threadLimit: number;
  /** How many delegates may run at once across this app (Settings › Harnesses). */
  totalLimit: number;
  /** Agents may read threads of every project, not only the calling thread's (Settings › Harnesses). */
  readAcrossProjects: boolean;
  /** Threads that were running when the app quit or the engine restarted continue on their own. */
  resumeAfterRestart: boolean;
  /** A system notification when a thread needs you or finishes (Settings › General). */
  systemNotifications: boolean;
  /** Those notifications only while the app is in the background (Settings › General). */
  notificationsBackgroundOnly: boolean;
  /** A recap of what happened while the app was idle (Settings › Notifications). */
  recapWhileAway: boolean;
  /** How long the app must be idle before that recap is due, in hours. */
  recapHours: number;
  /** When the user left, while a recap is on screen; null when there is none. */
  recapSince: number | null;
  /** The recap was dismissed until the next break. */
  recapDismissed: boolean;
  /** A thread stopped by its plan's limit continues when the limit resets. */
  continueAfterLimit: boolean;
  /** The threads waiting on that, by thread id (state/limitResume.ts). */
  limitWaits: Record<string, LimitWait>;
  /** The interface in SF Pro Rounded (the default) or the standard system font. */
  roundedFont: boolean;
  /** A streaming reply's new words fade in as the model writes them. */
  animateStreaming: boolean;
  /** Glass is solid on every OS (Settings › Appearance); the system's own request does the same. */
  reduceTransparency: boolean;
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
  /** How RustCode compacts a thread on its own (Settings › General › Threads). */
  autoCompact: AutoCompactSettings;
  /** Threads whose menu turned auto-compaction off, by thread id. */
  autoCompactOff: Record<string, boolean>;
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
  /**
   * Rows put away from "Needs you" without an answer, kept in app storage: the thread is
   * untouched, and the row comes back when it asks again (state/inboxDismissed.ts).
   */
  inboxDismissed: InboxDismissals;
  /** Timed approvals still in force ("Allow for 1 hour"), from `_rustcode/permissions`. */
  grants: TimedGrant[];
  /**
   * The approval each thread just answered, by thread id: its card folds into
   * this one settled line for a moment instead of vanishing (ui/AttentionCards.tsx).
   */
  resolvedApprovals: Record<string, ResolvedApproval>;
  /**
   * Remote access for this machine (`docs/remote.md`): whether it is on, the
   * gateway's address, the paired devices and a pairing challenge while one
   * is on screen. Read from `_rustcode/remote`.
   */
  remote: RemoteStatus | null;
  remoteChallenge: PairingChallenge | null;
  remotePaired: { name: string } | null;
  /** Each open thread's own share, so its header can say a device is attached. */
  remoteByThread: Record<string, RemoteStatus>;
  /**
   * Why the last read of remote access failed, when it did. The last good
   * answer stays in `remote`, so a dropped request does not make the switch
   * look off.
   */
  remoteError: string | null;
  /**
   * The code shown after accepting a phone that asked to connect, until the phone pairs or it
   * runs out (`state/connectRequests.ts`). The only copy: never persisted.
   */
  connectCode: ConnectCode | null;
  /** What the main column shows instead of the chat. */
  view:
    | { kind: "chat" }
    | { kind: "inbox" }
    | { kind: "board" }
    | { kind: "review"; threadId: string }
    | { kind: "compare"; fanOut: string };
  /** Each thread's per-turn checkpoints (`_rustcode/checkpoints/list`). */
  checkpoints: Record<string, Checkpoint[]>;
  /**
   * The turns each thread's rewinds abandoned, newest first: drawn faded under a divider below the
   * live transcript (`state/attempts.ts`). In memory only, like a question's row: the engine's
   * history no longer holds those turns, so a reload reads only what is left.
   */
  attempts: Record<string, Attempt[]>;
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

/** How many threads keep their panel; past this the oldest entry is dropped. */
export const MAX_PANELS = 500;

/**
 * The views the panel can show. Declared before the store: `parsePanels` reads it while the store
 * is created, and a `const` further down is not initialized yet then (a blank window on start).
 */
const PANELS: readonly PanelKind[] = ["details", "changes", "pulls", "peek"];

const STORAGE = {
  smartUi: "rustcode.smartUi",
  pinned: "rustcode.pinned",
  sidebarOrder: "rustcode.sidebarOrder",
  archived: "rustcode.archived",
  messaged: "rustcode.messaged",
  inboxDismissed: "rustcode.inboxDismissed",
  project: "rustcode.projectCwd",
  theme: "rustcode.theme",
  themeActive: "rustcode.themeActive",
  pinsMigrated: "rustcode.pinsMigrated",
  archiveMigrated: "rustcode.archiveMigrated",
  sidebar: "rustcode.sidebarHidden",
  orchestration: "rustcode.orchestration",
  threadLimit: "rustcode.threadLimit",
  totalLimit: "rustcode.totalLimit",
  readAcrossProjects: "rustcode.readAcrossProjects",
  resumeAfterRestart: "rustcode.resumeAfterRestart",
  systemNotifications: "rustcode.systemNotifications",
  notificationsBackgroundOnly: "rustcode.notificationsBackgroundOnly",
  recapWhileAway: "rustcode.recapWhileAway",
  recapHours: "rustcode.recapHours",
  /** When the app was last used, so a return after hours knows how long the break was. */
  lastActive: "rustcode.lastActiveAt",
  continueAfterLimit: "rustcode.continueAfterLimit",
  limitWaits: "rustcode.limitWaits",
  /** The ids of the threads running a turn, kept while the engine is up. */
  runningThreads: "rustcode.runningThreads",
  roundedFont: "rustcode.roundedFont",
  animateStreaming: "rustcode.animateStreaming",
  reduceTransparency: "rustcode.reduceTransparency",
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
  /** Auto-compaction's mode and threshold (Settings › General › Threads). */
  autoCompact: "rustcode.autoCompact",
  /** Threads whose auto-compaction is off, by thread id. */
  autoCompactOff: "rustcode.autoCompactOff",
  /** Each harness's plan windows, so the composer's meter is there on launch. */
  planCache: "rustcode.planCache",
  /** Bindings the user changed, by command id; the table in lib/keys.ts holds the rest. */
  shortcuts: SHORTCUTS_KEY,
  /** Each thread's panel, so opening it returns to the view it had (state/store.ts `panelForThread`). */
  panelByThread: "rustcode.panelByThread",
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
  reduceTransparency: false,
  chatWidth: CHAT_WIDTH_DEFAULT,
  headerButtons: DEFAULT_HEADER_BUTTONS,
  resumeAfterRestart: true,
  systemNotifications: true,
  notificationsBackgroundOnly: true,
  recapWhileAway: true,
  recapHours: RECAP_HOURS_DEFAULT,
  continueAfterLimit: true,
  orchestration: true,
  threadLimit: DEFAULT_DELEGATES_PER_THREAD,
  totalLimit: DEFAULT_DELEGATES_TOTAL,
  readAcrossProjects: true,
  smartUi: true,
  autoCompact: DEFAULT_AUTO_COMPACT,
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
function storedNumber(key: string, fallback: number, min: number, max: number): number {
  const raw = stored(key);
  const n = raw === null ? Number.NaN : Number.parseInt(raw, 10);
  if (!Number.isFinite(n)) return fallback;
  return Math.min(max, Math.max(min, n));
}

/** A thread's own delegate limit, kept to 2..32. */
function storedThreadLimit(key: string, fallback: number): number {
  return storedNumber(key, fallback, MIN_DELEGATES_PER_THREAD, MAX_DELEGATES_PER_THREAD);
}

/**
 * The app instance's delegate total, kept to 2..64. Before it existed the one
 * limit was a thread's own, and its whole delegation tree could hold twice
 * that; a stored old value raises the total to the same ceiling, so a custom
 * limit does not quietly get smaller. The result is written back, so the
 * migration happens once: moving the per-thread setting later does not move
 * the total, and a fresh install starts at the default.
 */
export function loadTotalLimit(): number {
  if (stored(STORAGE.totalLimit) !== null) {
    return storedNumber(
      STORAGE.totalLimit,
      DEFAULT_DELEGATES_TOTAL,
      MIN_DELEGATES_TOTAL,
      MAX_DELEGATES_TOTAL,
    );
  }
  const total = Math.min(
    MAX_DELEGATES_TOTAL,
    Math.max(
      DEFAULT_DELEGATES_TOTAL,
      storedNumber(
        STORAGE.threadLimit,
        DEFAULT_DELEGATES_PER_THREAD,
        MIN_DELEGATES_PER_THREAD,
        MAX_DELEGATES_PER_THREAD,
      ) * 2,
    ),
  );
  store(STORAGE.totalLimit, String(total));
  return total;
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
  // On unless turned off: reading knows no project boundary, so an agent asked
  // about the user's work is not answered with a fragment of it.
  readAcrossProjects: storedFlag(STORAGE.readAcrossProjects, SETTINGS_DEFAULTS.readAcrossProjects),
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
  roundedFont: storedFlag(STORAGE.roundedFont, SETTINGS_DEFAULTS.roundedFont),
  animateStreaming: storedFlag(STORAGE.animateStreaming, SETTINGS_DEFAULTS.animateStreaming),
  reduceTransparency: storedFlag(STORAGE.reduceTransparency, SETTINGS_DEFAULTS.reduceTransparency),
  chatWidth: parseChatWidth(stored(STORAGE.chatWidth)),
  headerButtons: sanitizeHeaderButtons(storedJson<unknown>(STORAGE.headerButtons, null)),
  shortcuts: parseOverrides(stored(STORAGE.shortcuts)),
  orchestrationPending: false,
  links: loadBook(),
  grant: null,
  features: {},
  statusMarks: storedJson<Record<string, ThreadStatus>>(STORAGE.statusMarks, {}),
  autoCompact: parseAutoCompactSettings(stored(STORAGE.autoCompact)),
  autoCompactOff: storedJson<Record<string, boolean>>(STORAGE.autoCompactOff, {}),
  snoozed: storedJson<Record<string, number>>(STORAGE.snoozed, {}),
  snoozeAsk: null,
  defaultPermissionMode: parsePermissionMode(stored(STORAGE.permissionMode)),
  permissionModes: storedJson<Record<string, PermissionMode>>(STORAGE.permissionModes, {}),
  newThreadPermissionMode: null,
  finished: {},
  failed: {},
  grants: [],
  resolvedApprovals: {},
  remote: null,
  remoteChallenge: null,
  remotePaired: null,
  remoteByThread: {},
  remoteError: null,
  connectCode: null,
  view: { kind: "chat" },
  checkpoints: {},
  attempts: {},
  newWorktree: { on: false, branch: "", edited: false },
  pendingSetup: null,
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
  status: "connecting",
  engineLog: [],
  sessions: [],
  machineNames: {},
  connectedHosts: [],
  sessionsLoaded: false,
  sessionsCapped: false,
  threads: {},
  selectedId: null,
  projectCwd: stored(STORAGE.project) ?? "",
  defaultCwd: stored(STORAGE.project) ?? "",
  newThread: null,
  newThreadAccount: null,
  newThreadHost: LOCAL_HOST,
  sidebarDevice: null,
  remoteHarnesses: {},
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
/** `state/machines.ts` `resetMachinesForTests`, once its module is loaded. */
let machineReset: (() => void) | null = null;

/** The connection a thread's operations go over: this computer's engine, or that machine's own. */
export function connectionFor(host: string): AcpConnection | null {
  return host ? (machineConnections.get(host) ?? null) : connection;
}

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
      const names = Object.fromEntries(state.machines.map((machine) => [machine.id, machine.name]));
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
          ...(s.sidebarDevice && s.sidebarDevice !== "devices" && !names[s.sidebarDevice]
            ? { sidebarDevice: null }
            : {}),
          ...(named ? { newThreadHost: LOCAL_HOST, projectCwd: "" } : {}),
        };
      });
      mergeMachineLists(state.sessions);
    };
    machineMirrorOff = machines.useMachines.subscribe((state, prev) => {
      if (state.sessions !== prev.sessions || state.machines !== prev.machines) mirror();
    });
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
          if (t.running)
            return [key, finishTurn(t, "failed", Date.now(), "The machine disconnected during this turn.")];
          return [key, t.permissions.length ? { ...t, permissions: [] } : t];
        }),
      ),
    }));
    return;
  }
  machineConnections.set(id, next);
  void subscribePullWatches(next, id).catch(() => {});
  machineOff.set(
    id,
    next.onInbound((message) => void handleInbound(scopeInbound(message, id), next, id)),
  );
  // Back: a harness or login added there while it was away is in the list a second read gives.
  if (get().remoteHarnesses[id]) void loadMachineHarnesses(id, { refresh: true });
  // Back: the copy on screen is from before it went away, so the thread it shows is read again.
  // Requests of the old connection did not survive the drop (they died with it), so there is
  // nothing to keep here: the reload is the machine's whole answer about the thread.
  set((s) => ({
    connectedHosts: [...machineConnections.keys()],
    threads: Object.fromEntries(
      Object.entries(s.threads).map(([key, t]) =>
        splitThreadKey(key).host === id ? [key, { ...t, loaded: false, running: false }] : [key, t],
      ),
    ),
  }));
  const selected = get().selectedId;
  if (selected && splitThreadKey(selected).host === id) void openThread(selected);
}

/** A machine's ids reach the store as keys: `‹host›/‹sessionId›`. */
function scopeInbound(message: Inbound, host: string): Inbound {
  if (message.type === "closed") return message;
  const sessionId = str(message.params.sessionId);
  if (!sessionId) return message;
  return { ...message, params: { ...message.params, sessionId: threadKey(host, sessionId) } };
}

/** Replaces the sessions of every machine with the list it last gave (this computer's are kept). */
function mergeMachineLists(lists: Record<string, readonly SessionSummary[]>): void {
  set((s) => {
    // A thread the app just started there has the title it was sent with until the machine titles
    // it from its own history: `session/new` carries no title, and `thread_rename` is this shell's.
    const known = new Map(s.sessions.map((session) => [session.id, session]));
    const remote = Object.entries(lists).flatMap(([host, items]) =>
      items.map((session) => {
        const id = threadKey(host, session.id);
        const title = session.title || known.get(id)?.title;
        return { ...session, id, host, title };
      }),
    );
    return {
      sessions: byActivity([...s.sessions.filter((x) => !x.host), ...remote]),
      snoozed: {
        ...Object.fromEntries(
          Object.entries(s.snoozed).filter(
            ([id]) => !remote.some((x) => x.id === id && x.snoozedUntil !== undefined),
          ),
        ),
        ...Object.fromEntries(
          remote
            .filter(
              (x) => x.snoozedUntil !== undefined && x.snoozedUntil !== null && x.snoozedUntil > Date.now(),
            )
            .map((x) => [x.id, x.snoozedUntil as number]),
        ),
      },
    };
  });
}

/** Ignore machine-status reads that started before a newer answer changed remote state. */
let remoteStatusRevision = 0;
let remoteStatusReadId = 0;
let latestRemoteStatusReadId = 0;
/** Sessions whose `session/load` replay is arriving. */
const loading = new Set<string>();
/** The shared `session/load` for each thread; callers must not prompt before its replay finishes. */
const opening = new Map<string, Promise<string | null>>();

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
    if (crashed && prev) toast("The server restarted", "success", { id: ENGINE_TOAST });
    else dismissToast(ENGINE_TOAST);
    crashed = false;
    return;
  }
  if (s.status === "closed") {
    crashed = true;
    toast(s.statusDetail ?? "The server stopped.", "error", {
      id: ENGINE_TOAST,
      action: { label: "Restart", run: () => void restartEngine() },
    });
    return;
  }
  // Starting: an error toast from before goes at once; a slow start gets its own.
  if (get().toasts.some((t) => t.id === ENGINE_TOAST && t.tone === "error")) dismissToast(ENGINE_TOAST);
  slowTimer = setTimeout(() => {
    if (get().status !== "connecting") return;
    toast(crashed ? "Restarting the server…" : "Starting the server…", "info", {
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
    // The paired computers' supervisors are the shell's too: follow the connections they open.
    void startMachineRouting();
  } catch (error) {
    set({ status: "closed", statusDetail: errorText(error) });
  }
}

async function handshake(): Promise<void> {
  connection?.close("The server restarted.");
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
      refreshGrants(conn),
    ]);
    // The composer's meter is ready before the first thread opens.
    void refreshPlanCache();
    if (connection === conn) await syncPinsAndSettled(conn);
    // Queued messages and wakes come back from app storage, and the threads that kept one open, so a
    // row's own turn marks what it reports before any wake is worked out below.
    restoreQueuedThreads();
    void subscribePullWatches(conn).catch(() => {});
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
  crashed = true;
  bootProgress = 0.15;
  set({ status: "connecting", statusDetail: "Restarting the server…" });
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
  if (state !== "running") scheduleAutoCompact(id);
}

async function handleInbound(message: Inbound, conn: AcpConnection | null = connection, host = LOCAL_HOST) {
  switch (message.type) {
    case "closed":
      if (host) {
        // One machine's channel closed: this computer's engine is untouched. Its threads stay
        // listed and read-only (`watchMachineConnection`), and a turn it ran ended with it.
        return;
      }
      set((s) => ({
        status: "closed",
        statusDetail: message.reason,
        threads: Object.fromEntries(
          Object.entries(s.threads).map(([id, t]) => [
            id,
            t.running ? finishTurn(t, "failed", Date.now(), "The server stopped during this turn.") : t,
          ]),
        ),
      }));
      return;
    case "notification":
      if (message.method === Method.sessionUpdate) onSessionUpdate(message.params);
      else if (message.method === Method.sessionActivity) onSessionActivity(message.params);
      else if (message.method === Method.threadCreated) threadCreated(message.params);
      else if (message.method === V2Method.orchestrationCancelled) orchestrationCallDropped(message.params);
      else if (message.method === PULL_WATCH && conn)
        void deliverPullWatch(conn, message.params, host).catch(() => {});
      else if (message.method === ThemeMethod.changed) onThemesChanged(message.params);
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
        if (!get().threads[parsed.sessionId]?.loaded) await openThread(parsed.sessionId);
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
  if (update.kind === "compaction" && update.phase !== "started" && autoCompactInFlight.has(sessionId)) {
    autoCompactCardSeen.add(sessionId);
    autoCompactPendingNotice.delete(sessionId);
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
    set((s) => ({ sessions: s.sessions.map((x) => (x.id === sessionId ? { ...x, status } : x)) }));
    clearStatusMark(sessionId);
  }
  if (update.kind === "title") {
    set((s) => ({
      sessions: s.sessions.map((x) => (x.id === sessionId ? { ...x, title: update.title } : x)),
    }));
  }
  updateThread(sessionId, (t) => applyUpdate(t, update));
}

// ---------------------------------------------------------------- sessions

/**
 * A thread the engine started for a paired device (Remote access, `docs/remote.md`): the thread
 * has no row in the engine's list before its first turn, so the engine names it here and the
 * sidebar shows it as any other. Nothing is opened: the device is using it.
 */
export function threadCreated(params: unknown) {
  const p = obj(params);
  const id = str(p?.sessionId);
  const cwd = str(p?.cwd);
  if (!id || !cwd) return;
  set((s) => {
    if (s.sessions.some((x) => x.id === id)) return {};
    const summary: SessionSummary = {
      id,
      cwd,
      updatedAt: Date.now(),
      title: str(p?.title) || undefined,
      harness: str(p?.harness) || undefined,
      model: str(p?.model) || undefined,
    };
    return { sessions: [summary, ...s.sessions] };
  });
}

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
    // The engine's list is the newer truth about a status.
    reconcileStatusMarks(all);
    for (const x of all)
      if (x.snoozedUntil !== undefined)
        applySnooze(x.id, x.snoozedUntil !== null && x.snoozedUntil > Date.now() ? x.snoozedUntil : null);
    // Another computer's list is its own read: ask each connected machine again, too.
    void import("./machines").then((m) => m.refreshMachineThreads());
    set((s) => {
      // Threads started here that the engine does not list yet (it lists a session once it saved history).
      const listed = new Set(all.map((x) => x.id));
      const started = s.sessions.filter((x) => !x.host && !listed.has(x.id) && s.threads[x.id]);
      // Another machine's threads are listed by that machine (state/machines.ts), kept as they are.
      const remote = s.sessions.filter((x) => x.host);
      const sessions = byActivity([...started, ...all, ...remote]);
      return {
        sessions,
        sessionsLoaded: true,
        sessionsCapped: capped,
        // The new-thread screen starts in this computer's most recent project: another machine's
        // folder is not a folder here (**Run on** moves the thread there, with its own project).
        projectCwd: s.projectCwd || sessions.find((x) => !x.host)?.cwd || "",
      };
    });
    // A thread the engine no longer lists keeps no place in the arrangement. A capped list is not
    // the whole list, so it prunes nothing: the rows of the rest are still there.
    if (!capped) pruneSidebarOrder(get().sessions.map((x) => x.id));
  } catch (error) {
    set({ sessionsLoaded: true });
    toast(`Could not list threads: ${errorText(error)}`, "error");
  }
}

/**
 * A turn ended: flag the thread unless you are looking at it. The row says Done (or Failed) until you open the thread.
 *
 * A thread that sleeps after a snooze is marked all the same — the row reads Done or Failed when its
 * snooze ends — but nothing is said about it now: the notification gate keeps quiet for a sleeping
 * thread, and `wakeSnoozed` says its one thing then.
 */
export function markFinished(id: string, now = Date.now(), failed = false) {
  const looking = get().selectedId === id && typeof document !== "undefined" && document.hasFocus();
  if (looking) return;
  set((s) =>
    failed
      ? { failed: { ...s.failed, [id]: now } }
      : { finished: { ...s.finished, [id]: now }, failed: without(s.failed, id) },
  );
  // A limit already said what stopped it, in the words the composer's notice uses.
  if (!failed || !get().limitWaits[id]) {
    // A delegate finishing is not news for the system either: the wake message reaches its parent
    // and the strip above that parent's composer already says it. A failure still is.
    const delegated = linksOf(get().links, id).drivenBy !== undefined && get().messaged[id] === undefined;
    if (!delegated) {
      notifyThread(
        { threadId: id, kind: failed ? "failed" : "finished", key: String(now), title: notifyTitle(id) },
        get(),
      );
    }
  }
}

/** A thread's title, for what the system shows of it. */
function threadName(id: string): string {
  const s = get();
  return sessionById(s, id)?.title || s.threads[id]?.title || "A thread";
}

/**
 * A notification's title: the thread's, and the computer it runs on when that
 * is not this one, so two machines' "Run the tests" are told apart.
 */
function notifyTitle(id: string): string {
  const machine = machineOfThread(id);
  return machine ? `${threadName(id)} · ${machine}` : threadName(id);
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

/** Take a thread out of the list until `at`, or bring it back now with `null`: the write alone. */
export async function setSnoozed(id: string, at: number | null): Promise<boolean> {
  if (!threadIsRemote(id)) applySnooze(id, at);
  const conn = connectionForThread(id);
  if (conn || threadIsRemote(id)) {
    try {
      if (!conn) throw new Error("That computer is not connected.");
      await conn.request(
        "_rustcode/session/snooze",
        { session_id: splitThreadKey(id).id, until: at },
        10_000,
      );
    } catch (error) {
      if (threadIsRemote(id) || !(error instanceof RpcError && error.isMethodNotFound)) {
        toast(`Could not snooze the thread: ${errorText(error)}`, "error");
        return false;
      }
    }
  }
  applySnooze(id, at);
  return true;
}

function applySnooze(id: string, at: number | null) {
  const { [id]: _, ...rest } = get().snoozed;
  const snoozed = at === null ? rest : { ...rest, [id]: at };
  set({ snoozed });
  store(STORAGE.snoozed, JSON.stringify(snoozed));
}

/**
 * The user wrote in this thread, so a snoozed one comes back now (`submit`, `steerOrSubmit`):
 * writing in a thread is what asks for it, whichever way the message goes out — its own turn, the
 * queue behind a running one, or a steer into it. A message the app sends instead is not a write
 * and leaves the snooze alone (`runPrompt`).
 */
function wakeOnWrite(id: string) {
  if (id in get().snoozed) setSnoozed(id, null);
}

/**
 * Snoozes a thread because the user asked — the sidebar's menu, the command
 * palette, the custom dialog — or wakes one with `null`.
 *
 * Snoozing the thread on screen in the chat leaves it, with the panel, for the
 * new-thread screen in the same folder, the way settling or archiving it does;
 * the board, review and compare have no new-thread screen, so snoozing there
 * stays on that view. Snoozing another thread changes nothing on screen, and
 * waking a thread never navigates. A caller that is not the user — an agent's
 * `update_session`, a wake whose time has come — calls `setSnoozed` instead,
 * which never moves the screen.
 */
export async function snoozeThread(id: string, at: number | null) {
  if (!(await setSnoozed(id, at))) return;
  const s = get();
  if (at !== null && s.view.kind === "chat" && s.selectedId === id) startNewThread();
}

export function askCustomSnooze(id: string | null) {
  set({ snoozeAsk: id });
}

/**
 * What a thread that slept has to say now its snooze is over: one thing at most, when it waits on
 * the user (an approval or a question) or its last turn ended unseen. Null for a thread with
 * nothing new, which simply takes its place in the list again.
 */
export function snoozeWakeEvent(id: string): NotifyEvent | null {
  const s = get();
  return wakeEvent({
    threadId: id,
    title: threadName(id),
    request: s.threads[id]?.permissions[0],
    finishedAt: s.finished[id],
    failedAt: s.failed[id],
    // The same read `markFinished` makes: a delegate's own ending is its parent's news, not a
    // notification of its own with a row the user cannot find.
    delegated: linksOf(s.links, id).drivenBy !== undefined && s.messaged[id] === undefined,
  });
}

/**
 * Brings back the threads whose time has come, quietly. Nothing a sleeping thread did reached the
 * list, the badge, the inbox or a notification (state/notify.ts, state/inbox.ts, state/sidebarRows.ts),
 * so this is where it is heard once: a request that waits or a turn that ended unseen says itself, at
 * most one thing, and a thread with nothing to say just returns with the state it reached.
 */
export function wakeSnoozed(now = Date.now()) {
  for (const id of dueSnoozes(get().snoozed, now)) {
    applySnooze(id, null);
    const event = snoozeWakeEvent(id);
    if (event) notifyThread(event, get());
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

/** The shortest gap between two window-focus list refreshes, see below. */
const FOCUS_LIST_GAP_MS = 5_000;
let lastFocusListAt = 0;

/**
 * A window focus re-lists the threads, at most once per 5 s: a change made outside this window
 * (`rustcode threads settle` in a terminal, another app) reaches the list with no turn to carry it,
 * and flapping between windows should not read 500 threads each time. Off until connected, and
 * `now` is a parameter so it can be felt with a clock in tests.
 */
export async function refreshSessionsOnFocus(now = Date.now()): Promise<void> {
  if (!connection || get().status !== "ready") return;
  if (now - lastFocusListAt < FOCUS_LIST_GAP_MS) return;
  lastFocusListAt = now;
  await refreshSessions();
}

if (typeof window !== "undefined") {
  window.addEventListener("focus", () => {
    clearFinished(get().selectedId);
    void refreshSessionsOnFocus();
  });
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
  if (id && harnessOf(get(), id) === CLAUDE_CODE) claudePlansStarted = true;
  // The account this thread runs on may never have been read: its meter has no
  // numbers until it is, and it appears without a visit to Settings › Usage.
  // The list's summary names the account before the thread itself is opened.
  void refreshPlanCache();
  if (id) void openThread(id);
}

export function setSidebarDevice(device: string | null) {
  set({ sidebarDevice: device });
}

export function startNewThread(cwd?: string, host?: string) {
  const s = get();
  // An explicit machine wins, then the device being browsed for New thread / ⌘N.
  // A folder picked on this disk still starts here (docs/desktop.md › Several machines).
  const browsing = s.sidebarDevice && s.machineNames[s.sidebarDevice] ? s.sidebarDevice : LOCAL_HOST;
  const target = host ?? (cwd === undefined ? browsing : LOCAL_HOST);
  // Where it starts: a folder picked for this one wins, then the default folder (Settings › General,
  // ⌘⇧O), so the sidebar's button, ⌘N and the command palette land where the reader said even while
  // another thread is open; then the folder of the thread they were in, then the last one used.
  // Picking a folder here does not change the default. A thread on a machine starts in that
  // machine's folders: the same project there when it has one (state/projects.ts), else its most
  // recent project. A folder of this computer is not a folder there.
  const selected = s.selectedId ? s.threads[s.selectedId] : undefined;
  const selectedCwd = selected && splitThreadKey(selected.id).host === target ? selected.cwd : undefined;
  const mine = projectsOn(s.sessions, target);
  const wasOn = projectAt(s.sessions, s.projectCwd, s.newThreadHost);
  const preferred =
    target === LOCAL_HOST
      ? s.defaultCwd || selectedCwd || matchProject(wasOn, mine)?.path
      : mine.find((project) => project.path === selectedCwd)?.path || mine[0]?.path;
  // The last screen's folder is kept only by that same screen: host and folder are written
  // together, and a worktree is under no project path (`hostOfProject`), so the host is the
  // only trustworthy half. Without it, a machine's worktree folder starts a thread here.
  const remembered =
    s.newThreadHost === target && hostOfProject(s, s.projectCwd) === target ? s.projectCwd : "";
  const projectCwd = cwd ?? preferred ?? remembered;
  dismissOverlays();
  set({
    selectedId: null,
    projectCwd,
    newThreadHost: target,
    // The panel belongs to a thread: the new-thread screen has none, and a
    // thread picked next restores the panel it had.
    panel: null,
    // The Thread tab shows a thread beside this one; a new thread has none.
    peekId: null,
    settings: null,
    switcherOpen: false,
    // The picker's choice is the session's draft: leaving the new-thread screen and coming back
    // shows what was picked, not the default again. `newThread` is only empty on a fresh start,
    // where the remembered (or default) choice takes over.
    newThreadPermissionMode: null,
    view: { kind: "chat" },
    newWorktree: { on: s.newWorktree.on, branch: "", edited: false },
  });
  // A machine's harnesses are its own, read before the composer's picker can draw them.
  if (target) void loadMachineHarnesses(target);
}

/** The folder of the new thread being written (⌘O there, the composer footer's folder menu). */
export function setProjectCwd(cwd: string) {
  set({ projectCwd: cwd });
}

/**
 * The computer a folder belongs to: this one when it has a project there, else a paired machine
 * with it, else this one (a folder this app has not seen). A folder inside a project counts as
 * that project (a worktree, a subfolder a thread ran in).
 */
function hostOfProject(s: Pick<AppState, "sessions" | "machineNames">, path: string): string {
  if (!path) return LOCAL_HOST;
  for (const host of [LOCAL_HOST, ...Object.keys(s.machineNames)]) {
    const found = projectsOn(s.sessions, host).some(
      (project) => project.path === path || path.startsWith(`${project.path}/`),
    );
    if (found) return host;
  }
  return LOCAL_HOST;
}

/** Whether a machine's **Run on** is being changed, so a slow answer for an earlier one is dropped. */
let runOnSeeks = 0;

/**
 * **Run on**: the computer the next new thread runs on. The folder follows: the same project
 * here, found again on that machine by path, then by git remote, then by folder name
 * (state/projects.ts), else that machine's most recent project. Its harnesses, models and
 * logins are read over its own connection, as the local picker reads this computer's.
 */
export async function setNewThreadHost(host: string): Promise<void> {
  const seek = ++runOnSeeks;
  const s = get();
  if (host === s.newThreadHost) return;
  const previous = s.newThreadHost;
  const wasOn = projectAt(s.sessions, s.projectCwd, previous);
  const projects = projectsOn(s.sessions, host);
  const match = matchProject(wasOn, projects);
  set({ newThreadHost: host, projectCwd: match?.path ?? projects[0]?.path ?? "" });
  if (!host) return;
  // A fresh read: a harness or login added on the machine while Run on was elsewhere shows.
  const snapshot = await loadMachineHarnesses(host, { report: false, refresh: true });
  if (seek !== runOnSeeks || get().newThreadHost !== host) return;
  if (!snapshot) {
    // The machine went away before it answered: the choice goes back, so the composer keeps a
    // thread it can start rather than a machine that cannot answer.
    // Never the folder the failed move just chose: it belongs to the machine that went away.
    set({ newThreadHost: previous, projectCwd: wasOn?.path ?? "" });
    toast(`${getName(host)} is not connected. The thread will run on this computer.`, "error");
    return;
  }
  // A machine that listed nothing when the picker moved may have answered by now.
  const listed = projectsOn(get().sessions, host);
  if (!get().projectCwd && listed.length) set({ projectCwd: listed[0]!.path });
}

/**
 * Read one machine's harnesses, models, accounts and defaults over its own connection, the call
 * the local picker gets from `_rustcode/harnesses`. Kept per machine; `refresh` reads it again
 * (a machine's default harness may have changed on it).
 */
export async function loadMachineHarnesses(
  host: string,
  options: { report?: boolean; refresh?: boolean } = {},
): Promise<HarnessSnapshot | null> {
  if (!host) return get().harnesses;
  const cached = get().remoteHarnesses[host];
  if (cached && !options.refresh) return cached;
  const conn = connectionFor(host);
  if (!conn) {
    if (options.report) toast(`${getName(host)} is not connected.`, "error");
    return cached ?? null;
  }
  try {
    const snapshot = parseHarnessSnapshot(await conn.request(Method.harnesses, {}, 15_000));
    // It dropped while the answer was coming: a list from a gone machine is not shown.
    if (connectionFor(host) !== conn) return cached ?? null;
    set((st) => ({ remoteHarnesses: { ...st.remoteHarnesses, [host]: snapshot } }));
    return snapshot;
  } catch (error) {
    // A machine on an older RustCode has no `_rustcode/harnesses`: it runs the builtin harness,
    // as this computer's own client says of an engine before harnesses (`fallbackHarnessSnapshot`).
    if (error instanceof RpcError && error.isMethodNotFound) {
      const fallback = fallbackHarnessSnapshot();
      set((st) => ({ remoteHarnesses: { ...st.remoteHarnesses, [host]: fallback } }));
      return fallback;
    }
    if (options.report && !cached) {
      toast(`Could not read ${getName(host)}'s agents: ${errorText(error)}`, "error");
    }
    return cached ?? null;
  }
}

/** A paired machine's name, as the composer and its toasts say it. */
function getName(host: string): string {
  return get().machineNames[host] ?? "That computer";
}

/** Where new threads start by default (⌘⇧O, Settings › General); a new thread being written follows. */
export function setDefaultCwd(cwd: string) {
  // The default folder is this computer's, so a new thread being written goes back to it.
  set((s) => ({
    defaultCwd: cwd,
    projectCwd: s.selectedId ? s.projectCwd : cwd,
    ...(s.selectedId ? {} : { newThreadHost: LOCAL_HOST }),
  }));
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
    toast("This server cannot move a thread to another folder. Update RustCode.", "error");
    return false;
  }
  if (thread && get().agent?.closeSession) await closeThread(id);
  try {
    await connectionForThread(id)?.request(
      Method.sessionMove,
      { sessionId: splitThreadKey(id).id, cwd },
      10_000,
    );
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

/**
 * Loads a thread's history (the replay) if it is not loaded yet. Answers the
 * open error when it could not be opened, so a caller can say why instead of
 * reporting an empty thread.
 */
export function openThread(id: string): Promise<string | null> {
  const inFlight = opening.get(id);
  if (inFlight) return inFlight;
  // Another computer's thread loads over that machine's connection (`state/threadKeys.ts`); an
  // offline one is not opened at all, and the chat says so instead of a conversation.
  const conn = connectionForThread(id);
  const { id: sessionId } = splitThreadKey(id);
  const existing = get().threads[id];
  // A thread on a machine that is away is still a thread: its row stays and it opens read-only,
  // with the chat and the composer saying so (docs/desktop.md › Several machines). Without this it
  // would draw as a new thread, with this computer's folder and controls.
  if (!conn && threadIsRemote(id)) {
    if (!existing) {
      const summary = get().sessions.find((x) => x.id === id);
      set((s) => ({
        threads: {
          ...s.threads,
          [id]: { ...emptyThread(id, summary?.cwd ?? "", summary?.title), loaded: false },
        },
      }));
    }
    return Promise.resolve(null);
  }
  // A copy whose open failed is no copy: opening it again tries again, so a thread that could not
  // be opened once (the engine was restarting, its harness was down) is not refused for good.
  if (!conn || (existing?.loaded && !existing.openFailed) || existing?.loading || loading.has(id))
    return Promise.resolve(null);
  const summary = get().sessions.find((x) => x.id === id);
  const cwd = existing?.cwd ?? summary?.cwd ?? get().projectCwd;
  let pending: Promise<string | null>;
  pending = Promise.resolve().then(async () => {
    try {
      const result = await conn.request(Method.sessionLoad, { sessionId, cwd, mcpServers: [] }, 60_000);
      updateThread(id, (t) => ({
        ...finishLoad(t),
        configOptions: parseConfigOptions((result as JsonObject | null)?.configOptions),
        harness: parseSessionHarness(result) ?? t.harness,
      }));
      const opened = get().threads[id];
      if (id === get().selectedId && opened?.harness?.id === CLAUDE_CODE) {
        claudePlansStarted = true;
        void refreshPlanCache();
      }
      if (opened && lacksModels(opened))
        void ensureHarnessModels(opened.harness!.id, splitThreadKey(id).host);
      void refreshPlanUsage(id);
      void refreshCheckpoints(id);
      // A thread opened after it went idle can still be before its cache deadline.
      scheduleAutoCompact(id);
      return null;
    } catch (error) {
      const message = `Could not open this thread: ${errorText(error)}`;
      updateThread(id, (t) => ({ ...finishLoad(t), error: message, openFailed: true }));
      return message;
    } finally {
      loading.delete(id);
      if (opening.get(id) === pending) opening.delete(id);
    }
  });
  opening.set(id, pending);
  loading.add(id);
  set((s) => ({
    threads: { ...s.threads, [id]: { ...emptyThread(id, cwd, summary?.title), loading: true } },
  }));
  return pending;
}

/**
 * Repair Thread: the engine checks a closed thread the way opening it does and
 * puts right what it safely can (a harness record pointing at another folder).
 * The thread is then opened again; what is left (a folder that is gone) is said.
 */
export async function repairThread(id: string): Promise<void> {
  const conn = connectionForThread(id);
  if (!conn || !get().agent?.sessionRepair) {
    toast("This server cannot repair threads. Update RustCode.", "error");
    return;
  }
  if (get().threads[id]?.running) {
    toast("Stop the running turn before repairing this thread.");
    return;
  }
  await closeThread(id);
  let answer: JsonObject;
  try {
    answer = ((await conn.request(Method.sessionRepair, { sessionId: splitThreadKey(id).id }, 10_000)) ??
      {}) as JsonObject;
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
    await connectionForThread(id)?.request(Method.sessionClose, { sessionId: splitThreadKey(id).id }, 10_000);
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
  /** The thread an agent starts this one from (`state/host.ts` `newThread`), or the user opens it beside. */
  parent?: string;
  /** Why it has that parent; a delegate when omitted, which is what an agent's thread is. */
  relationship?: ThreadRelationship;
  title?: string;
  select?: boolean;
  /** The setup this session finishes (state/setup.ts): it is listed and the setup cleared together. */
  pendingId?: string;
  /**
   * The computer it runs on (`state/machines.ts`): its own connection is asked for
   * `session/new`, and the thread is keyed with its host (`state/threadKeys.ts`).
   * Omitted is this computer.
   */
  host?: string;
}

/** `session/new` with the harness, model, effort and worktree in `_meta.rustcode`; the thread is added to the list. */
export async function createSession(options: NewSession): Promise<string> {
  const host = options.host ?? LOCAL_HOST;
  // A thread on another computer is made there, over that computer's own connection: its
  // folder, its harness and its worktree are on that machine, never here.
  const conn = host ? connectionFor(host) : engine();
  if (!conn) throw new Error(host ? "That computer is not connected." : "The server is not connected.");
  const s = get();
  const params: JsonObject = { cwd: options.cwd, mcpServers: [] };
  const meta: JsonObject = {};
  if (options.harness && !harnessesOn(s, host)?.unsupported) {
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
  if (options.parent) {
    // The machine's own session id, not a key that names it here (`state/threadKeys.ts`).
    meta.parent = splitThreadKey(options.parent).id;
    if (options.relationship && options.relationship !== "delegate") meta.relationship = options.relationship;
  }
  if (Object.keys(meta).length) params._meta = { rustcode: meta };
  const result = (await conn.request(Method.sessionNew, params, 120_000)) as JsonObject;
  const raw = str(result.sessionId);
  if (!raw) throw new Error("The server did not return a thread id.");
  const id = threadKey(host, raw);
  const worktree: Worktree | undefined = parseWorktree(
    ((result._meta as JsonObject | undefined)?.rustcode as JsonObject | undefined)?.worktree,
  );
  const repo = str(((result._meta as JsonObject | undefined)?.rustcode as JsonObject | undefined)?.repo);
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
    // The sidebar groups it under the machine it runs on (`ui/Sidebar.tsx`); this computer's
    // threads carry no host, as they always did.
    host: host || undefined,
    cwd,
    updatedAt: Date.now(),
    harness: thread.harness?.id ?? options.harness,
    account: thread.harness?.account ?? options.account ?? undefined,
    worktree,
    repo,
    title: options.title,
    // A thread an agent started is nested from its first frame: without the parent here the
    // sidebar draws it as a row of its own until the engine lists it with the same record, then
    // moves it under its parent (`state/topLevel.ts`).
    parent: options.parent
      ? {
          id: options.parent,
          title: sessionById(s, options.parent)?.title,
          relationship: options.relationship ?? "delegate",
        }
      : undefined,
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
          // Selecting the new thread keeps the picker's choice as the session's draft, so the
          // new-thread screen comes back on it (state/store.ts `startNewThread`).
          ...(options.select ? viewingThread(st, id) : {}),
          sessions: [summary, ...st.sessions],
          pendingSetup: setupSucceeded(st.pendingSetup, options.pendingId),
        }),
  }));
  // Saved, not only shown: otherwise the thread's first message replaces it at the next list.
  // A pending setup lets its rename run beside it: the message starts the moment the thread is listed.
  // A thread on another computer is titled by that machine's own history: only this computer's
  // folder is renamed from here (`thread_rename` is the shell's).
  if (options.title && !host) {
    const saved = renameThread(id, options.title);
    if (!pending) await saved;
  }
  // A model or effort picked before the thread existed applies now, when the thread did not take it
  // from `session/new` (an older engine, or a harness that ignores it there).
  const current = (configId: string) => thread.configOptions.find((o) => o.id === configId)?.current;
  const has = (configId: string) => thread.configOptions.some((o) => o.id === configId);
  if (options.model && current("model") !== options.model && has("model")) {
    await setConfigOption(id, "model", options.model, { remember: false });
  }
  if (options.effort && current("effort") !== options.effort && has("effort")) {
    await setConfigOption(id, "effort", options.effort, { remember: false });
  }
  if (pending) {
    // Listed now, with the setup gone in the same update: the row the message was sent to becomes
    // the thread, in place, and the chat goes on from the message it already shows.
    set((st) => ({
      sessions: [summary, ...st.sessions],
      ...(options.select && !st.selectedId ? viewingThread(st, id) : {}),
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
export function newThreadChoice(
  s: { newThread: NewThreadChoice | null; harnesses: HarnessSnapshot | null } & {
    newThreadHost?: string;
    remoteHarnesses?: Record<string, HarnessSnapshot>;
    /** The folder a new thread would start in: its project default comes first. */
    projectCwd?: string;
  },
): NewThreadChoice | null {
  // The computer the thread will run on chooses the harnesses: a machine's default harness,
  // models and logins are its own (`state/machines.ts`).
  const snapshot = harnessesOn(s, s.newThreadHost);
  if (!snapshot) return null;
  if (!s.newThread) {
    // A folder whose project names a harness and model starts on them, so the picker shows
    // exactly what ⏎ starts (`startSetup` reads the same two functions).
    return (
      projectDefaultChoice(s, s.projectCwd) ??
      restoreChoice(snapshot, snapshot.default.harness, appChoices(snapshot), effortMemory())
    );
  }
  // A kept draft can name a harness that is gone, or one that cannot run now (turned off, signed
  // out in Settings): then the screen falls back to the harness a new thread would start on, never
  // to this draft. One that can run is normalized as the picker normalizes it.
  const harness = snapshot.harnesses.find((h) => h.id === s.newThread?.harness);
  if (!harness || unavailableReason(harness)) return preferredDefault(snapshot);
  const choice = normalizeDefault(snapshot, s.newThread);
  // A machine's `_rustcode/harnesses` lists only the models a harness has already learned there,
  // and `_rustcode/harnesses/models` is not on its gateway: with none listed, keep no model, so
  // the machine's own harness default applies rather than a model picked on this computer.
  if (s.newThreadHost && !harness.models.length) return { ...choice, model: null, effort: null };
  return choice;
}

/**
 * The new-thread choice a project set for `cwd`: a harness that can run now,
 * on the project's model when that harness offers it. A folder with no
 * project default, or one whose harness is turned off or signed out, starts a
 * thread the way every other folder does; a harness the user picked wins.
 */
export function projectThreadChoice(
  s: Pick<AppState, "newThread" | "harnesses">,
  cwd: string,
): NewThreadChoice | null {
  if (s.newThread) return null;
  return projectDefaultChoice(s, cwd);
}

/**
 * The project's own default, when it can run here: this computer's folders
 * only, since a machine's folders and harnesses are its own. The project book
 * holds one harness/model pair per folder (`state/projectBook.ts`).
 */
function projectDefaultChoice(
  s: { harnesses: HarnessSnapshot | null; newThreadHost?: string },
  cwd: string | null | undefined,
): NewThreadChoice | null {
  if (s.newThreadHost || !s.harnesses || !cwd) return null;
  const { harness, model } = projectChoice(cwd);
  if (!harness) return null;
  const found = s.harnesses.harnesses.find((h) => h.id === harness);
  if (!found || unavailableReason(found)) return null;
  const restored = restoreChoice(s.harnesses, harness);
  const offered = !!model && (!found.models.length || found.models.some((m) => m.id === model));
  return { ...restored, model: offered ? model : restored.model, effort: restored.effort };
}

/**
 * Creates a thread with the new-thread choice (or the default harness) and worktree toggle, and
 * selects it. `session/new` starts a harness there (and a worktree, when the toggle is on), which
 * takes a moment: the setup, with the message and its attachments, is shown before it is asked for,
 * so ⏎ on the new-thread screen is answered at once. A second send while it runs starts nothing; a
 * setup that failed is sent again with `retryPendingSetup`.
 */
async function createThread(firstMessage = "", extras: ComposeExtras = emptyExtras): Promise<string | null> {
  const s = get();
  if (!s.projectCwd) throw new Error("Choose a project folder for this thread first.");
  const setup = beginSetup(get().pendingSetup, {
    id: localId("setup"),
    text: firstMessage,
    extras: carriesExtras(extras) ? extras : undefined,
    title: titleFrom(firstMessage, 80) || (hasExtras(extras) ? "Image" : "New thread"),
    cwd: s.projectCwd,
    // The computer it belongs to: its group in the sidebar, its harnesses, its `session/new`.
    host: s.newThreadHost,
    worktree: newWorktreeFor(s, s.projectCwd, firstMessage),
  });
  if (!setup) {
    const failed = get().pendingSetup?.failure;
    if (failed) toast(failed.text, "error");
    else toast("Starting the thread: it appears here when it is ready.");
    return null;
  }
  // One update switches the screen and shows the message: nothing waits for the engine.
  set({ pendingSetup: setup });
  // That update gets its own painted frame before `session/new` is asked for. An in-process harness
  // answers in a microtask, so both updates would otherwise land in one task and one frame: the
  // pending screen's message would be drawn for the first time by the thread's own transcript, and
  // its entry (`rc-send-in`) would never get a frame to play in.
  await afterPaint();
  return await startSetup(setup);
}

/** The worktree the new-thread composer asked for, or none. */
function newWorktreeFor(
  s: Pick<AppState, "newWorktree" | "features" | "newThreadHost">,
  cwd: string,
  firstMessage: string,
): PendingSetup["worktree"] {
  const wt = s.newWorktree;
  // A worktree is cut on the machine the thread runs on, by that machine's own `session/new`; the
  // toggle is this computer's, so it is only offered for threads that start here.
  if (s.newThreadHost || !wt.on || !s.features.worktrees) return undefined;
  return { repo: cwd, branch: wt.branch.trim() || undefined, title: titleFrom(firstMessage, 80) };
}

/**
 * `session/new` for a setup, with the choices the new-thread screen holds. Its title is the one the
 * row already shows, so nothing blinks back to "New thread" when the row becomes real. On failure
 * the setup keeps the message, its attachments and the reason, for a retry.
 */
async function startSetup(setup: PendingSetup): Promise<string | null> {
  const s = get();
  const host = setup.host ?? LOCAL_HOST;
  // The harness choice belongs to the machine the setup runs on, which may not be the one the
  // composer shows now (the user moved **Run on** after sending). A project's own default harness
  // and model are this machine's folders, so they only shape a local setup.
  const choice =
    (host === LOCAL_HOST ? projectThreadChoice(s, setup.cwd) : null) ??
    newThreadChoice({
      newThread: s.newThread,
      harnesses: s.harnesses,
      remoteHarnesses: s.remoteHarnesses,
      newThreadHost: host,
      projectCwd: setup.cwd,
    });
  // The login follows the choice `session/new` is given, not a draft it fell back from.
  const harness = harnessesOn(s, host)?.harnesses.find((h) => h.id === choice?.harness);
  let id: string;
  try {
    id = await createSession({
      cwd: setup.cwd,
      host,
      harness: choice?.harness,
      // The account shown for the new thread: with named accounts, always one of them.
      account: newThreadAccountOn(harness, s.newThreadAccount, s.signedOutLogins),
      model: choice?.model,
      effort: choice?.effort,
      worktree: setup.worktree,
      select: true,
      pendingId: setup.id,
      title: setup.title,
    });
  } catch (error) {
    const text = errorText(error);
    set((st) =>
      st.pendingSetup?.id === setup.id ? { pendingSetup: setupFailed(st.pendingSetup, text) } : {},
    );
    return null;
  }
  // The access level chosen on the new-thread composer belongs to the thread it started.
  const pendingMode = get().newThreadPermissionMode;
  if (pendingMode) {
    setPermissionMode(id, pendingMode);
    set({ newThreadPermissionMode: null });
  }
  set({ newWorktree: { on: get().newWorktree.on, branch: "", edited: false } });
  return id;
}

/**
 * Retry: a failed setup sends its message again, with the attachments and the place it held, and the
 * prompt goes out the moment the session exists. Its own row stays where it is while it runs.
 */
export async function retryPendingSetup(): Promise<boolean> {
  const failed = get().pendingSetup;
  if (!failed?.failure) return false;
  const setup = beginSetup(failed, {
    id: localId("setup"),
    text: failed.text,
    extras: failed.extras,
    title: failed.title,
    cwd: failed.cwd,
    host: failed.host,
    worktree: failed.worktree,
  });
  if (!setup) return false;
  set({ pendingSetup: setup });
  const id = await startSetup(setup);
  if (!id) return false;
  // Its bubble is already on screen, from the setup it keeps through the retry: it does not fly in.
  void runPrompt(id, setup.text, setup.extras ?? emptyExtras, { alreadyShown: true });
  return true;
}

/**
 * Gives up a failed setup: its row and its error leave, and its message goes back to the field it
 * was typed in, with the pictures and files that went with it, so nothing that was written is lost.
 */
export function dismissPendingSetup() {
  const pending = get().pendingSetup;
  set({ pendingSetup: dismissSetup() });
  if (!pending?.failure) return;
  if (pending.text.trim()) setDraft("new", pending.text);
  const extras = pending.extras;
  if (extras) setExtras("new", () => ({ ...extras, attachments: [...extras.attachments] }));
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
    const conn = connectionForThread(id);
    if (!conn) throw new Error("That computer is not connected.");
    await conn.request(Method.rewind, { sessionId: splitThreadKey(id).id, userIndex, text }, 10_000);
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
  const extras = { ...emptyExtras, attachments: pastesOf(item.attachments, item.text) };
  const go = async () => {
    if (canRewind(thread)) {
      if (await rewindTo(id, itemId, item.text)) void runPrompt(id, item.text, extras);
    } else void submit(item.text, extras);
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

/**
 * Put rows away without answering them: a failure nobody will retry, a request the
 * user would rather answer in the thread itself. Only the inbox row and its count
 * change — a failed turn still shows in the transcript and a request still waits in
 * its chat — and the row comes back if the thread asks again (state/inboxDismissed.ts).
 * One Undo toast puts back every row of this call.
 */
export function dismissInbox(items: readonly InboxItem[]): void {
  if (!items.length) return;
  const keys = items.map((item) => item.key);
  const next = dismissRows(get().inboxDismissed, items);
  set({ inboxDismissed: next });
  store(STORAGE.inboxDismissed, JSON.stringify(next));
  const title = items.length === 1 ? items[0]!.title : "";
  toast(items.length === 1 ? `Dismissed "${title}"` : `Dismissed ${items.length} items`, "info", {
    id: items.length === 1 ? `inbox-dismiss:${keys[0]}` : "inbox-dismiss",
    action: { label: "Undo", run: () => restoreInbox(keys) },
  });
}

/** Undo: the rows go back in the inbox exactly as they were. */
export function restoreInbox(keys: readonly string[]): void {
  const before = get().inboxDismissed;
  const next = restoreRows(before, keys);
  if (next === before) return;
  set({ inboxDismissed: next });
  store(STORAGE.inboxDismissed, JSON.stringify(next));
}

/** Retry a thread's failed turn: the inbox's action for a failure. The thread is
 * opened first when the app has not read it, because the message to send again
 * lives in its transcript; its last message is the one the failure answered.
 */
export async function retryThread(id: string): Promise<void> {
  if (!get().threads[id]?.loaded) await openThread(id);
  const thread = get().threads[id];
  if (!thread) return;
  for (let i = thread.items.length - 1; i >= 0; i--) {
    const item = thread.items[i]!;
    if (item.kind !== "user") continue;
    // Retried: the failure is answered, so the inbox and the row stop saying Failed.
    clearFinished(id);
    retryMessage(id, item.id);
    return;
  }
  // Nothing to send again (an empty transcript): at least show the thread.
  selectThread(id);
}

/**
 * The pastes a sent message kept, back in a message that is sent again: a retry or an edit must
 * carry the log the message was about, or the model is asked about text it cannot see. Only a
 * message sent before pastes went into the text has these; one written since carries the text
 * itself, so `at` puts them back at its end, where the chip used to send them.
 */
function pastesOf(attachments: SentAttachment[] | undefined, text: string): Attachment[] {
  return (attachments ?? []).flatMap((a) =>
    a.kind === "text" && a.text
      ? [{ id: localId("paste"), kind: "text" as const, text: a.text, at: text.length }]
      : [],
  );
}

/** Edit: the message goes back to the composer; sending it rewinds the thread to it first. */
export function editMessage(id: string, itemId: string) {
  const thread = get().threads[id];
  const item = thread?.items.find((i) => i.id === itemId);
  if (!thread || item?.kind !== "user") return;
  const text = splitImageMarkers(item.text).text;
  // What the message held past the text comes back with it, as the chips it was written as. The
  // composer's own paste chips go: the draft is this message now, not the one edited before it.
  const pastes = pastesOf(item.attachments, text);
  setExtras(id, (e) => ({
    ...e,
    attachments: [...e.attachments.filter((a) => a.kind !== "text"), ...pastes],
  }));
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
  if (!get().features.pins) return;
  // A pin is kept by the computer the thread runs on: a machine's threads go over that machine's
  // connection, with the machine's own id (`state/threadKeys.ts`). A local thread with no engine
  // keeps the app's pin, as before.
  const groups = new Map<AcpConnection, string[]>();
  const unreachable: string[] = [];
  for (const id of ids) {
    const conn = connectionForThread(id);
    if (conn) groups.set(conn, [...(groups.get(conn) ?? []), id]);
    else if (threadIsRemote(id)) unreachable.push(id);
  }
  if (!groups.size && !unreachable.length) return;
  type PinResult = { id: string; ok: true } | { id: string; ok: false; error: unknown };
  void Promise.all<PinResult>([
    ...[...groups].flatMap(([conn, group]) =>
      group.map((id) =>
        conn.request(V2Method.sessionPin, { session_id: splitThreadKey(id).id, pinned: on }, 10_000).then(
          (): PinResult => ({ id, ok: true }),
          (error: unknown): PinResult =>
            error instanceof RpcError && error.isMethodNotFound ? { id, ok: true } : { id, ok: false, error },
        ),
      ),
    ),
    ...unreachable.map(
      (id): PinResult => ({ id, ok: false, error: new Error("That computer is not connected.") }),
    ),
  ]).then((results) => {
    const failed = results.filter((r): r is { id: string; ok: false; error: unknown } => !r.ok);
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

/**
 * Arrange one sidebar section (`state/sidebarOrder.ts`): the ids in the order the user left them,
 * kept in app storage beside `pinned`, so a drag or ⌥↑/↓ outlives the window.
 */
export function setSectionOrder(key: string, ids: string[]) {
  const sidebarOrder = { ...get().sidebarOrder, [key]: ids };
  set({ sidebarOrder });
  store(STORAGE.sidebarOrder, JSON.stringify(sidebarOrder));
}

/** The arrangement, written only when it changed (`state/sidebarOrder.ts`). */
function saveSidebarOrder(next: SidebarOrder, before: SidebarOrder) {
  if (next === before) return;
  set({ sidebarOrder: next });
  store(STORAGE.sidebarOrder, JSON.stringify(next));
}

/**
 * A thread that is gone keeps no place in any section's arrangement: without this a trashed thread
 * would leave its id in app storage for good. A no-op when it had no place.
 */
export function forgetSidebarOrder(ids: readonly string[]) {
  saveSidebarOrder(withoutThreads(get().sidebarOrder, ids), get().sidebarOrder);
}

/**
 * The arrangement limited to the threads the engine still lists: a thread trashed from another app,
 * or before this field existed, loses its place here. Callers pass the whole list — a capped one is
 * not the whole list and prunes nothing (state/store.ts `refreshSessions`).
 */
export function pruneSidebarOrder(known: readonly string[]) {
  const ids = new Set(known);
  saveSidebarOrder(
    keepThreads(get().sidebarOrder, (id) => ids.has(id)),
    get().sidebarOrder,
  );
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
  // A thread's name lives on the computer that owns it, and renaming one is that computer's
  // folder (`thread_rename` is this shell's): another machine's thread is not renamed from here.
  if (threadIsRemote(id)) {
    toast("A thread on another computer is renamed on that computer.", "info");
    return false;
  }
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
  // Its delegates stop with it: forgetting their links would free slots they still hold.
  await stopLinksFrom(id);
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
  const autoCompactOff = { ...get().autoCompactOff };
  delete autoCompactOff[id];
  forgetAutoCompact(id);
  set((s) => {
    const peeking = s.peekId === id;
    return {
      threads,
      archived,
      pinned,
      autoCompactOff,
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
  // Its place in the sidebar's arrangement goes with it, or the stored order keeps an id forever.
  forgetSidebarOrder([id]);
  store(STORAGE.archived, JSON.stringify(archived));
  store(STORAGE.pinned, JSON.stringify(pinned));
  store(STORAGE.autoCompactOff, JSON.stringify(autoCompactOff));

  if (get().selectedId === id) startNewThread();
  return true;
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

/** The harness a thread runs on, or a new thread would start on. */
export function threadHarness(thread?: ThreadState) {
  const s = get();
  // A thread's harness is the one its own machine lists; a new thread's is the one the computer
  // **Run on** names (docs/desktop.md › Several machines).
  const snapshot = harnessesOn(s, thread ? splitThreadKey(thread.id).host : s.newThreadHost);
  const id = thread
    ? (thread.harness?.id ?? "rustcode")
    : (s.newThread?.harness ?? snapshot?.default.harness);
  return snapshot?.harnesses.find((h) => h.id === id);
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
      // `/new` in a thread starts on that thread's computer, in its folder, even before it is
      // open; in a new thread, on the current Run on.
      const listed = from ? s.sessions.find((x) => x.id === from) : undefined;
      startNewThread(
        thread?.cwd ?? listed?.cwd ?? (from ? undefined : s.projectCwd),
        from ? splitThreadKey(from).host : s.newThreadHost,
      );
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
  // The thread is made for this message, so the new-thread screen already draws its row: it must not
  // fly in again when the thread takes the screen over (lib/sendFlight.ts).
  const madeHere = !id;
  try {
    if (!id) {
      const started = await createThread(text, extras);
      // A setup that failed keeps the message and its attachments on the thread's own bubble, with
      // Retry: the field stays clear rather than holding a second copy. One that could not start at
      // all (another is still going) leaves the draft in the composer, as it was.
      if (!started) return !!get().pendingSetup?.failure;
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
      if (await rewindTo(id!, edit.itemId, edit.original)) {
        unsettleThreads([id!], { quiet: true });
        void runPrompt(id!, text, extras);
      }
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
  if (trimmed !== "/fork" || hasExtras(extras)) {
    unsettleThreads([id], { quiet: true });
    // Its message may wait behind the running turn: the thread comes back all the same.
    wakeOnWrite(id);
  }
  if (thread.running) {
    updateThread(id, (t) => ({
      ...t,
      queue: [
        ...t.queue,
        // The text as it was written: a paste's offset is an offset in it, and the row composes the
        // two when it draws and when the turn runs (`composedText`).
        { id: localId("queued"), text, extras: carriesExtras(extras) ? extras : undefined },
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
        { id: localId("queued"), text, extras: carriesExtras(extras) ? extras : undefined },
      ],
    }));
    void resumeQueue(id);
    return true;
  }
  void runPrompt(id, text, extras, { alreadyShown: madeHere });
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

/** The text of a message as a quote: every line behind `> `, ready to be followed by a question. */
export function quoteMessage(text: string): string {
  const lines = text.trim().split("\n");
  return `${lines.map((line) => (line ? `> ${line}` : ">")).join("\n")}\n\n`;
}

/**
 * "Ask in a side chat": a new thread beside `parentId` on the same harness, model, effort and
 * folder (never a new worktree), whose composer starts with `message` quoted. It opens in the
 * panel's Thread tab so the parent stays on screen; closing the panel keeps the thread, and the
 * sidebar and the parent's transcript list it. The engine records the parent (relationship
 * `side_chat`) in the new thread's own metadata.
 */
export async function openSideChat(parentId: string, message: string): Promise<string | undefined> {
  const parent = get().threads[parentId];
  if (!parent || !message.trim()) return undefined;
  // An engine that predates the relationship ignores it and would list the side chat as a delegate.
  if (!get().agent?.lineage) {
    toast("Side chats need a newer RustCode engine. Update RustCode and restart.", "error");
    return undefined;
  }
  const choice = (configId: string) => parent.configOptions.find((o) => o.id === configId)?.current;
  const title = sessionById(get(), parentId)?.title || parent.title;
  try {
    const id = await createSession({
      // The folder the parent runs in, its worktree included: a side chat reads the same files.
      cwd: parent.cwd,
      harness: parent.harness?.id,
      account: parent.harness?.account,
      model: choice("model") ?? null,
      effort: choice("effort") ?? null,
      parent: parentId,
      relationship: "side_chat",
      title: title ? `Side chat: ${title}` : "Side chat",
    });
    // Never broader than the thread it was asked from, as for a delegate (state/permissions.ts).
    setPermissionMode(id, effectiveMode(permissionModeOf(get(), parentId), get().defaultPermissionMode));
    setDraft(id, quoteMessage(message));
    peekThread(id);
    return id;
  } catch (error) {
    toast(`Could not open a side chat: ${error instanceof Error ? error.message : String(error)}`, "error");
    return undefined;
  }
}

/**
 * ⌘⏎: hand the running turn a message to read after its current step, else send. What it does is
 * `sendDecision`'s answer for this message, the same one the composer's hint and the queued row's
 * Send now read, so nothing here disagrees with what the app says it will do.
 */
export async function steerOrSubmit(text: string, extras: ComposeExtras = emptyExtras): Promise<boolean> {
  const id = get().selectedId;
  const thread = id ? get().threads[id] : undefined;
  const trimmed = text.trim();
  if (!id || !thread?.running || !get().agent?.steer) return submit(text, extras);
  // Nothing typed and nothing attached: take what waits first, by the row's own rule.
  if (!trimmed && !hasExtras(extras)) {
    const next = thread.queue[0];
    if (next) return sendQueuedNow(id, next.id);
    return submit(text, extras);
  }
  unsettleThreads([id], { quiet: true });
  // A steer or a queued row leaves no turn of its own behind to read the snooze: it is the user
  // writing in the thread, so the thread comes back here (`submit` does the same).
  wakeOnWrite(id);
  const verdict = sendDecisionFor(thread, extras);
  if (verdict.action === "steer" && (await steer(id, text, extras))) return true;
  if (verdict.action === "interruptRestart") return restartWith(id, text, extras);
  // A steer the engine refused (the turn ended or began waiting on the user meanwhile) waits too.
  return queueMessage(id, text, extras, verdict.action === "queue" ? verdict.reason : "refused");
}

/** A message that does not steer waits for the next turn, saying why it waits. */
async function queueMessage(
  id: string,
  text: string,
  extras: ComposeExtras,
  reason: QueueReason,
): Promise<boolean> {
  if (carriesExtras(extras)) {
    // Submitted rather than parked, so the chips and the ids a mention stands for go with it.
    const sent = await submit(text, extras);
    if (sent) toast(QUEUE_REASON[reason]);
    return sent;
  }
  updateThread(id, (t) => ({
    ...t,
    queue: [...t.queue, { id: localId("queued"), text }],
  }));
  toast(QUEUE_REASON[reason]);
  return true;
}

/**
 * Stop the running turn and send this message in its place: what `sendDecision` answers for a
 * harness that takes neither a steer nor the queue. The message goes to the head of the queue for
 * the stop to land, and a turn this stopped runs it on (`thread.ts` `finishTurn` keeps the queue
 * for Resume otherwise).
 */
async function restartWith(id: string, text: string, extras: ComposeExtras): Promise<boolean> {
  updateThread(id, (t) => ({
    ...t,
    queue: [
      {
        id: localId("queued"),
        text,
        extras: carriesExtras(extras) ? extras : undefined,
        restart: true,
      },
      ...t.queue,
    ],
  }));
  await cancel(id);
  return true;
}

/** The steering record of the harness a thread runs on: the snapshot's is the fresher of the two. */
export function steeringOf(thread?: ThreadState): SteeringCapabilities {
  const own = thread?.harness?.capabilities.steering;
  const fresh = threadHarness(thread)?.capabilities.steering;
  if (!own)
    return (
      fresh ?? {
        live: false,
        attachments: false,
        while_awaiting_user: true,
        interrupt_restart: true,
        queue: true,
      }
    );
  if (!fresh) return own;
  // A harness that learned to steer after this thread started is `live` in the snapshot only.
  return {
    ...own,
    live: own.live || fresh.live,
    attachments: own.attachments || fresh.attachments,
  };
}

/** What the thread's turn is doing, for the one send rule. */
export function turnState(thread?: ThreadState): TurnState {
  return {
    running: thread?.running ?? false,
    awaitingUser: (thread?.permissions.length ?? 0) > 0,
  };
}

/** What sending `extras` to this thread now would do: steer, queue (with the reason), or restart. */
export function sendDecisionFor(
  thread: ThreadState | undefined,
  extras: ComposeExtras | undefined,
): SendDecision {
  return sendDecision(extras, steeringOf(thread), turnState(thread));
}

/**
 * Hands the running turn a message to read after its current step (`_rustcode/steer`). False when the
 * engine will not take one now. Exported for the wake a busy parent gets for a child's question
 * (state/host.ts `sendWake`), which is sent by the same rule as a typed message.
 */
export async function steer(id: string, text: string, extras?: ComposeExtras): Promise<boolean> {
  const thread = get().threads[id];
  // What the turn reads: the typed text with its command, links and long pastes in place.
  const full = composedText(text, extras ?? emptyExtras);
  const conditions = {
    images: acceptsImages(thread),
    builtin: threadHarness(thread)?.builtin ?? true,
  };
  const prompt = extras?.attachments.length
    ? promptBlocks(text, extras, {
        images: conditions.images,
        builtin: conditions.builtin,
        cwd: thread?.cwd ?? "",
      })
    : undefined;
  try {
    const conn = connectionForThread(id);
    if (!conn) return false;
    const sessionId = splitThreadKey(id).id;
    const result = (await conn.request(
      Method.steer,
      prompt ? { sessionId, text: full, prompt } : { sessionId, text: full },
      10_000,
    )) as JsonObject | null;
    if (result?.accepted !== true) return false;
    updateThread(id, (t) => appendSteer(t, full, Date.now(), extras && sentAttachments(extras)));
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
  const verdict = sendDecisionFor(thread, item.extras);
  if (verdict.action === "queue") {
    toast(QUEUE_REASON[verdict.reason]);
    return false;
  }
  unsettleThreads([id], { quiet: true });
  if (verdict.action === "interruptRestart") {
    removeQueued(id, queuedId);
    return restartWith(id, item.text, item.extras ?? emptyExtras);
  }
  if (await steer(id, item.text, item.extras)) {
    removeQueued(id, queuedId);
    glideAway(id, item);
    return true;
  }
  toast(QUEUE_REASON.refused);
  return false;
}

/** How long a queued row takes to leave the stack, gliding up toward the transcript (index.css). */
const STEER_GLIDE_MS = 240;

/**
 * A queued row a steer took keeps the stack's place while it glides out, so the row itself leaves
 * instead of vanishing from under the reader; it is dropped once the glide is over.
 */
function glideAway(id: string, item: QueuedPrompt) {
  updateThread(id, (t) => departQueued(t, item));
  setTimeout(() => updateThread(id, (t) => dropDeparted(t, item.id)), STEER_GLIDE_MS);
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
    resumeLinks: next.resume,
    wake: next.wake,
    wakeLinks: next.wakeLinks,
    wakeAsking: next.wakeAsking,
    // A resumed row is the app's, not the user's: the thread stays where it was in the sidebar.
    automatic: next.resume !== undefined,
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
    // Its attachments stay; one with attachments survives an emptied text. The text is kept as it
    // was written, since a paste is spliced at its offset when the message is composed.
    if (!trimmed && !hasExtras(next)) return { ...t, queue: t.queue.filter((q) => q.id !== queuedId) };
    return {
      ...t,
      queue: t.queue.map((q) => (q.id === queuedId ? { ...q, text, extras: next } : q)),
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
    /** The turn that continues after a limit: its wait is settled when it ends. */
    limitResume?: boolean;
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
  // The turn runs on the thread's own computer, which may have gone away since it was opened.
  const conn = connectionForThread(id);
  if (!conn) {
    toast(threadIsRemote(id) ? "That computer is not connected." : "The server is not connected.", "error");
    return "skipped";
  }
  const thread = get().threads[id];
  // The prompt reads as a divider both ways: live, and after a reload from the history the engine
  // kept, which carries the text and not this flag (state/thread.ts `CONTINUE_MARK`).
  if (options.continued) text = withContinueMark(text);
  // A prompt no agent asked for is the user's: the thread is theirs now, even
  // when it was started by an agent (state/topLevel.ts). A resume after a
  // restart, a wake or a message from another thread is not a message the user
  // sent, and neither is the turn that continues after a usage limit.
  if (userTypedPrompt(options)) markMessaged(id);
  // Whoever starts a turn now moots the wait, and if this one hits the limit too it waits again.
  if (!options.limitResume && !options.autoCompact && !thread?.running) dropLimitWait(id);
  // Never a second prompt into a running turn: an external harness takes it as the next turn and can
  // interrupt the tools it is still running (a parent's wait_session). It waits in the queue, like a
  // message typed meanwhile, and starts when this turn ends.
  if (thread?.running) {
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
  if (userTypedPrompt(options) && !options.queued) wakeOnWrite(id);
  const startIndex = thread?.items.length ?? 0;
  if (options.link) linkStarted(options.link);
  // The turn a restart resumes this thread with answers the links the quit severed: they work again
  // from here, so the parent reads this turn's answer and its time, never the time before the quit.
  if (options.resumeLinks?.length) linksResumed(options.resumeLinks);
  // A queued wake reports its links now, as its turn starts: late enough that a restart before this
  // point brings the wake back, early enough that nothing in the turn reports it again.
  if (options.wake && options.wakeLinks?.length) wakeDelivered(options.wakeLinks, options.wakeAsking ?? []);
  // The message the transcript shows: the command, links and typed text, with each long paste back
  // where it was pasted. The prompt's blocks say the same thing (below).
  const shown = composedText(text, extras);
  // A message typed here flies out of the composer on its first sighting; one the new-thread screen
  // already drew does not (lib/sendFlight.ts).
  const sent = userTypedPrompt(options) && !options.fromView && !options.alreadyShown;
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
    const request: JsonObject = { sessionId: splitThreadKey(id).id, prompt };
    // The access level rides on every prompt: a harness that sandboxes its own shell (Codex) leaves
    // the sandbox for Full access, on this machine and on a remote one alike.
    request._meta = {
      rustcode: {
        access: permissionModeOf(get(), id),
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
  // The limit is settled first: a turn it stopped is reported as the limit, not twice.
  // An automatic compaction must leave a pending wait alone, not settle it.
  const waiting = options.autoCompact
    ? (get().limitWaits[id] ?? null)
    : // The limit is read from what the harness said, not the words the thread shows.
      settleLimitWait(id, outcome, rawFailure, startIndex);
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
  if (
    !options.autoCompact &&
    !options.continued &&
    (outcome === "completed" || outcome === "failed") &&
    id !== get().selectedId &&
    // A thread the user snoozed says nothing: its finish waits for the snooze to end, which then
    // says it once (state/store.ts `wakeSnoozed`).
    !(id in get().snoozed)
  ) {
    const name = get().sessions.find((s) => s.id === id)?.title || "A thread";
    // A delegate finishing is already said twice: the wake message into its parent's transcript and
    // the strip above that composer. Only a failure, or the limit that stopped it, is news here.
    const delegated = options.link !== undefined && outcome === "completed" && !waiting;
    if (!delegated) {
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
  const conn = id ? connectionForThread(id) : null;
  if (!id || !conn) return;
  const thread = get().threads[id];
  // Stopping a thread stops the work it gave other threads (M1); turns the user started there go on.
  void stopLinksFrom(id);
  if (!thread?.running) return;
  try {
    conn.notify(Method.sessionCancel, { sessionId: splitThreadKey(id).id });
    // ACP: after `session/cancel` the client answers outstanding permission requests as cancelled.
    for (const request of thread.permissions) conn.respond(request.rpcId, permissionCancelled);
    updateThread(id, (t) => settleEveryWait({ ...t, permissions: [] }, Date.now()));
    if (thread.permissions.length) linkWaitEnded(id);
  } catch (error) {
    toast(errorText(error), "error");
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

// ---------------------------------------------------------------- approvals

/** One answered approval, kept a moment so its card can settle into a line. */
export interface ResolvedApproval {
  rpcId: number | string;
  /** "Allowed edit src/app.ts · once". */
  text: string;
  allowed: boolean;
  at: number;
}

/**
 * Says the thread's computer is away, once, for a caller that would otherwise answer every
 * pending card with its own refusal. True when there is nothing to answer on.
 */
function refuseAway(key: string): boolean {
  if (connectionForThread(key)) return false;
  toast(threadIsRemote(key) ? "That computer is not connected." : "The server is not connected.", "error");
  return true;
}

/**
 * Answer exactly this request; it leaves the queue before the reply, so a double click cannot
 * answer the next. A question also leaves its answer in the transcript, where the reader can see
 * what was asked and what they said (state/thread.ts `questionItem`).
 */
function reply(request: PermissionRequest, result: JsonObject, answer?: string): boolean {
  // The answer goes back on the connection the request arrived on (`handleInbound`). With the
  // machine away there is nothing to answer on: the card stays, and an answer made now is refused
  // rather than dropped.
  const conn = connectionForThread(request.sessionId);
  if (!conn) {
    refuseAway(request.sessionId);
    return false;
  }
  updateThread(request.sessionId, (t) => {
    const settled = request.question ? t : settledWait(t, request.toolCallId, Date.now());
    return {
      ...settled,
      permissions: settled.permissions.filter((p) => p.rpcId !== request.rpcId),
      items: answer ? [...settled.items, questionItem(request, answer)] : settled.items,
    };
  });
  // Another request of the same thread keeps it waiting; the last one ends the wait.
  if (!get().threads[request.sessionId]?.permissions.length) linkWaitEnded(request.sessionId);
  conn.respond(request.rpcId, result);
  return true;
}

export function answerPermission(request: PermissionRequest, option: PermissionOption | null) {
  const answered = reply(
    request,
    option ? permissionSelected(option.id) : permissionCancelled,
    request.question ? (option?.name ?? "Declined") : undefined,
  );
  if (!answered || request.question) return;
  settleApproval(request, option);
  // An hour's grant is the engine's to keep: list it again so the panel shows it.
  if (option && optionScope(option) === "hour") void refreshGrants(connection);
}

/** How long an answered card stays as one settled line before it leaves. */
export const RESOLVED_APPROVAL_MS = 1800;

/**
 * The card an answer just closed folds into one line in place ("Allowed edit
 * src/app.ts · once") for a moment, so the answer has a visible result instead
 * of the card vanishing under the pointer.
 */
function settleApproval(request: PermissionRequest, option: PermissionOption | null) {
  const threadId = request.sessionId;
  const scope = option ? optionScope(option) : "decline";
  const text = option ? resolvedLine(request, option) : `Declined ${approvalSubject(request)}`;
  const previous = resolvedTimers.get(threadId);
  if (previous) clearTimeout(previous);
  set((s) => ({
    resolvedApprovals: {
      ...s.resolvedApprovals,
      [threadId]: { rpcId: request.rpcId, text, allowed: scope !== "decline", at: Date.now() },
    },
  }));
  const timer = setTimeout(() => {
    resolvedTimers.delete(threadId);
    set((s) => {
      // A later answer replaced it: that one owns the line and its own timer.
      if (s.resolvedApprovals[threadId]?.rpcId !== request.rpcId) return s;
      const { [threadId]: _gone, ...rest } = s.resolvedApprovals;
      return { resolvedApprovals: rest };
    });
  }, RESOLVED_APPROVAL_MS);
  resolvedTimers.set(threadId, timer);
}

const resolvedTimers = new Map<string, ReturnType<typeof setTimeout>>();

// ---------------------------------------------------------------- timed grants

/**
 * `_rustcode/permissions`: the "Allow for 1 hour" grants still in force, with
 * what each covers and when it ends. An engine without the method lists none.
 */
export async function refreshGrants(conn: AcpConnection | null = connection): Promise<void> {
  if (!conn) return;
  try {
    const grants = parseGrants(await conn.request(Method.permissions, {}, 10_000));
    if (connection === conn) set({ grants });
  } catch {
    // An older engine has none to list; what is known stays as it is.
  }
}

/** Revoke one timed grant; the engine answers with the list as it stands then. */
export async function revokeGrant(id: string): Promise<void> {
  const conn = connection;
  if (!conn) return;
  try {
    const grants = parseGrants(await conn.request(Method.permissionsRevoke, { id }, 10_000));
    if (connection === conn) set({ grants });
  } catch (error) {
    toast(`Could not revoke it: ${errorText(error)}`, "error");
  }
}

/** Approve every approval the thread waits on (not questions), each with its "allow once" option. */
export function approveAll(threadId: string) {
  // The answers go over the thread's own connection; with the machine away, one refusal is said
  // once, not once per card.
  if (refuseAway(threadId)) return;
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
  // With the machine away nothing can be answered: one refusal, not one per pending card.
  if (refuseAway(threadId)) return;
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

/**
 * Every answer carries the whole harness snapshot, and answers to requests in
 * flight together arrive in any order. A request takes a number when it is
 * sent, and an answer replaces the snapshot only when no later request's
 * answer has been applied already: a late, older one would bring back a stale
 * detection (a Ready harness read as Not installed).
 */
let harnessRequestSeq = 0;
let harnessSnapshotSeq = 0;

function applyHarnessSnapshot(seq: number, snapshot: HarnessSnapshot) {
  if (seq < harnessSnapshotSeq) return;
  harnessSnapshotSeq = seq;
  set({ harnesses: snapshot });
}

export async function refreshHarnesses(): Promise<HarnessSnapshot | null> {
  const conn = connection;
  if (!conn) return null;
  const seq = ++harnessRequestSeq;
  try {
    const snapshot = parseHarnessSnapshot(await conn.request(Method.harnesses, {}, 15_000));
    applyHarnessSnapshot(seq, snapshot);
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

/** `report: false` lets the caller word the failure itself, so no toast says it twice. */
async function harnessCall(
  method: string,
  params: JsonObject,
  report = true,
): Promise<HarnessSnapshot | null> {
  const seq = ++harnessRequestSeq;
  try {
    const snapshot = parseHarnessSnapshot(await engine().request(method, params, 15_000));
    applyHarnessSnapshot(seq, snapshot);
    return snapshot;
  } catch (error) {
    if (!report) throw error;
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

/** The named accounts of `harness`, as the snapshot lists them, on the computer `host` names. */
export function accountsOf(
  s: { harnesses: HarnessSnapshot | null; remoteHarnesses?: Record<string, HarnessSnapshot> },
  harness: string | undefined,
  host?: string,
): HarnessAccount[] {
  return harness ? (harnessesOn(s, host)?.harnesses.find((h) => h.id === harness)?.accounts ?? []) : [];
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
  if (harness === CLAUDE_CODE) claudePlansStarted = true;
  set({ newThreadAccount: account });
  if (harness) storeAccount(harness, account);
  // The login picked may never have been read: its own meter numbers come now.
  void refreshPlanCache();
}

/**
 * The user picked a chip in another harness's header: that harness's account is
 * remembered for new threads, and the new thread moves to that harness on the
 * model and effort it was last left on.
 */
export function pickNewThreadAccountOn(harness: string, account: string) {
  if (harness === CLAUDE_CODE) claudePlansStarted = true;
  storeAccount(harness, account);
  if (newThreadChoice(get())?.harness !== harness) switchNewThreadHarness(harness);
  set({ newThreadAccount: account });
  void refreshPlanCache();
}

export const addHarnessAccount = (harness: string, id: string, name: string, configDir?: string) =>
  harnessCall(Method.harnessAccountAdd, {
    harness,
    id,
    name,
    ...(configDir ? { config_dir: configDir } : {}),
  });
export const removeHarnessAccount = (harness: string, id: string) =>
  harnessCall(Method.harnessAccountRemove, { harness, id });

/**
 * Reads each account's own credentials again (nothing is started, no token is
 * read). `readPlans` also re-reads the Claude plan rows, which a 3-second
 * wait for a sign-in does not need: that would force a Keychain read each time.
 */
export async function checkHarnessAccounts(
  readPlans = true,
  /** Harnesses detected again instead of read from the engine's last-minute detection. */
  redetect: string[] = [],
  /** False throws a failure instead of a toast. */
  report = true,
): Promise<HarnessSnapshot | null> {
  const snapshot = await harnessCall(Method.harnessAccountCheck, { redetect }, report);
  // Claude Code signs in with its own credentials, which that snapshot does not
  // report (`auth_probe: None`), and its plan read pauses once it finds the
  // default login signed out or cannot read it (a denied Keychain prompt), so
  // neither asks again and again. An explicit check is the user asking to look
  // now: read the default login again, or it would stay as it was found after
  // signing in or out.
  if (readPlans) void refreshPlanCache(true);
  return snapshot;
}

export const removeHarness = (id: string) => harnessCall(Method.harnessRemove, { id });

/**
 * Registers a custom ACP harness from a local command (`_rustcode/harnesses/set`): the command is
 * launched directly, one argument per entry, never through a shell. Off until it is verified.
 * Throws the engine's refusal so the form can show it.
 */
export async function addCustomHarness(request: {
  id: string;
  name: string;
  command: string[];
  env: Record<string, string>;
}): Promise<void> {
  await harnessCall(Method.harnessSet, { ...request, kind: "acp", enabled: false }, false);
}

/** Turns a verified custom harness on, or takes an unverified one out again; a failure throws. */
export const enableCustomHarness = (id: string) =>
  harnessCall(Method.harnessSet, { id, enabled: true }, false);
export const discardCustomHarness = (id: string) => harnessCall(Method.harnessRemove, { id }, false);
/**
 * The Settings default: it is also where a new thread starts, so a draft the new-thread screen
 * kept for the session is dropped once the engine took the change, or the new default would never
 * reach that screen. A default the engine refused keeps the draft.
 */
export async function setDefaultHarness(choice: NewThreadChoice): Promise<HarnessSnapshot | null> {
  const snapshot = await harnessCall(Method.harnessDefault, {
    harness: choice.harness,
    model: choice.model,
    effort: choice.effort,
  });
  if (snapshot) set({ newThread: null });
  return snapshot;
}
export const completeOnboarding = async () => {
  const snapshot = await harnessCall(Method.onboardingComplete, {});
  if (snapshot) set({ onboardingOpen: false });
};

/** "Test": ask the harness for its models. Throws the harness's error so the card can show it. */
export async function fetchHarnessModels(id: string, refresh = false): Promise<number> {
  // `refresh` starts the harness again even when its models are known: a Check.
  const result = (await engine().request(
    Method.harnessModels,
    refresh ? { harness: id, refresh } : { harness: id },
    60_000,
  )) as JsonObject | null;
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
  if (refresh) await refreshHarnesses();
  else void refreshHarnesses();
  return models.length;
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
export function ensureHarnessModels(id: string, host = LOCAL_HOST): Promise<void> {
  // A machine lists its own harnesses' models, and only its own `_rustcode/harnesses/models` can
  // ask (`docs/desktop.md` › Several machines): what its snapshot carries is what the picker has.
  if (host) return Promise.resolve();
  const harness = get().harnesses?.harnesses.find((h) => h.id === id);
  if (!harness || harness.builtin || harness.models.length || get().status !== "ready")
    return Promise.resolve();
  let asked = modelsAsked.get(id);
  if (!asked) {
    asked = fetchHarnessModels(id)
      .then(() => undefined)
      .catch(() => undefined)
      .finally(() => modelsAsked.delete(id));
    modelsAsked.set(id, asked);
  }
  return asked;
}

export function setNewThreadChoice(choice: NewThreadChoice | null) {
  set({ newThread: choice });
}

/**
 * One harness's choice as the app keeps it: the engine's `last` shape, plus the effort each of
 * its models was last left on. Keyed by harness only, so its logins share one memory.
 */
export interface LocalChoice extends HarnessLast {
  efforts?: Record<string, string>;
}

/** The per-model efforts in one stored entry, dropping anything that is not a model and a level. */
function parseEfforts(value: Json | undefined): Record<string, string> {
  const raw = obj(value) ?? {};
  const out: Record<string, string> = {};
  for (const [model, level] of Object.entries(raw)) {
    const effort = str(level);
    if (model && effort) out[model] = effort;
  }
  return out;
}

/**
 * What the app keeps per harness: the choice an engine that does not remember starts from, and the
 * effort each model was last left on. An entry for an engine that remembers carries only `efforts`
 * (no model, no effort), which is why one is kept even when there is no model.
 */
export function localChoices(): Record<string, LocalChoice> {
  try {
    const raw = obj(JSON.parse(stored(STORAGE.harnessChoices) ?? "{}")) ?? {};
    const out: Record<string, LocalChoice> = {};
    for (const [id, value] of Object.entries(raw)) {
      const last = parseHarnessLast(value);
      const efforts = parseEfforts(obj(value)?.efforts);
      const known = Object.keys(efforts).length > 0;
      // The shape before per-model memory had no `efforts`: it reads as a model and an effort.
      if (last) out[id] = known ? { ...last, efforts } : last;
      else if (known) out[id] = { model: null, effort: null, efforts };
    }
    return out;
  } catch {
    return {};
  }
}

/** The effort each model of each harness was last left on (state/harness.ts `EffortMemory`). */
export function effortMemory(): EffortMemory {
  const out: EffortMemory = {};
  for (const [id, choice] of Object.entries(localChoices())) {
    if (choice.efforts) out[id] = choice.efforts;
  }
  return out;
}

/**
 * Keeps the effort a model was last left on, so picking that model again returns to its level.
 * Keyed by harness and model only, never a login: every account of a harness shares it.
 */
export function rememberModelEffort(
  harness: string | null | undefined,
  model: string | null | undefined,
  effort: string | null | undefined,
) {
  if (!harness || !model || !effort) return;
  const all = localChoices();
  const entry = all[harness];
  // The harness's own last model and effort stay as they are; an entry that has none (an engine
  // that remembers keeps them itself) keeps them empty, so only the memory is added.
  all[harness] = {
    model: entry?.model ?? null,
    effort: entry?.effort ?? null,
    efforts: { ...entry?.efforts, [model]: effort },
  };
  store(STORAGE.harnessChoices, JSON.stringify(all));
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
  // The per-model memory outlives the harness's own last choice: a write keeps it.
  const efforts = all[harness]?.efforts;
  if (value) all[harness] = efforts ? { ...value, efforts } : value;
  else if (efforts) all[harness] = { model: null, effort: null, efforts };
  else delete all[harness];
  store(STORAGE.harnessChoices, JSON.stringify(all));
}

/**
 * The user picked a model or effort in the new-thread picker: it is the choice, and the harness's
 * remembered one (`_rustcode/harnesses/remember`, else app storage), so switching harness and back,
 * or a restart, starts there again. The model keeps its own effort for the next time it is picked.
 * `host` is the computer the thread runs on: a machine's own last choice is its to keep, so only
 * this computer is asked to remember it.
 */
export function pickNewThreadChoice(choice: NewThreadChoice, host = LOCAL_HOST) {
  set({ newThread: choice });
  const last = { model: choice.model, effort: choice.effort };
  if (!host) noteChoice(choice.harness, last);
  rememberModelEffort(choice.harness, choice.model, choice.effort);
  if (host || !get().harnesses?.remembers || !connection) return;
  void engine()
    .request(Method.harnessRemember, { harness: choice.harness, ...last }, 15_000)
    .catch(() => undefined);
}

/** Switch the new-thread picker to `harness`, on the model and effort it was last left on. */
export function switchNewThreadHarness(harness: string) {
  // The machine the thread will run on has its own harnesses and its own defaults.
  const snapshot = harnessesOn(get(), get().newThreadHost);
  if (!snapshot) return;
  const next = snapshot.harnesses.find((h) => h.id === harness);
  const remembered = lastAccountFor(harness);
  set({
    newThread: restoreChoice(snapshot, harness, appChoices(snapshot), effortMemory()),
    // A harness with named accounts never starts on its own sign-in.
    newThreadAccount: newThreadAccountOn(next, remembered, get().signedOutLogins),
  });
  if (harness === CLAUDE_CODE) claudePlansStarted = true;
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
  const host = splitThreadKey(id).host;
  const named = accountsOf(get(), harness, host).find((a) => a.id === account);
  try {
    const conn = connectionForThread(id);
    if (!conn) throw new Error("That computer is not connected.");
    const result = (await conn.request(
      Method.sessionHarness,
      { sessionId: splitThreadKey(id).id, harness, account },
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
    const harnessName = harnessesOn(get(), host)?.harnesses.find((h) => h.id === harness)?.name ?? harness;
    toast(
      named
        ? `This thread now runs on ${named.name}. The conversation so far moves over as a summary with your next message.`
        : `This thread now runs on ${harnessName}'s own sign-in, with the conversation so far as a summary.`,
      "success",
    );
    void refreshPlanUsage(id);
    if (harness === CLAUDE_CODE) claudePlansStarted = true;
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
export async function switchThreadHarness(
  id: string,
  harness: string,
  account?: string | null,
): Promise<boolean> {
  const thread = get().threads[id];
  if (!thread || thread.running) return false;
  const host = splitThreadKey(id).host;
  const last = harnessesOn(get(), host)?.harnesses.find((h) => h.id === harness)?.last;
  try {
    const conn = connectionForThread(id);
    if (!conn) throw new Error("That computer is not connected.");
    const result = (await conn.request(
      Method.sessionHarness,
      {
        sessionId: splitThreadKey(id).id,
        harness,
        account: account ?? null,
        model: last?.model ?? null,
        effort: last?.effort ?? null,
      },
      30_000,
    )) as JsonObject | null;
    const listed = harnessesOn(get(), host)?.harnesses.find((h) => h.id === harness);
    const next =
      parseSessionHarness(result) ?? (listed && { id: harness, capabilities: listed.capabilities });
    updateThread(id, (t) => ({
      ...t,
      harness: next,
      configOptions: parseConfigOptions(result?.configOptions),
    }));
    set((s) => ({
      sessions: s.sessions.map((x) =>
        x.id === id ? { ...x, harness, account: next?.account ?? account ?? undefined } : x,
      ),
    }));
    if (harness === CLAUDE_CODE) claudePlansStarted = true;
    const opened = get().threads[id];
    if (opened && lacksModels(opened)) void ensureHarnessModels(harness, host);
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

export function setSystemNotifications(on: boolean) {
  set({ systemNotifications: on });
  store(STORAGE.systemNotifications, on ? "1" : "0");
}

export function setNotificationsBackgroundOnly(on: boolean) {
  set({ notificationsBackgroundOnly: on });
  store(STORAGE.notificationsBackgroundOnly, on ? "1" : "0");
}

export function setRecapWhileAway(on: boolean) {
  set({ recapWhileAway: on, ...(on ? {} : { recapSince: null }) });
  store(STORAGE.recapWhileAway, on ? "1" : "0");
}

export function setRecapHours(hours: number) {
  const kept = parseRecapHours(String(hours));
  set({ recapHours: kept });
  store(STORAGE.recapHours, String(kept));
}

/** A return after a long enough break: the recap starts at the moment the user left. */
export function beginRecap(since: number) {
  set({ recapSince: since, recapDismissed: false });
}

/** Puts the card away until the next break; the inbox keeps a row that brings it back. */
export function dismissRecap() {
  set({ recapDismissed: true });
}

export function showRecap() {
  set({ recapDismissed: false });
}

/** When the app was last used, from app storage; 0 on a first run. */
export function lastActiveAt(): number {
  const raw = stored(STORAGE.lastActive);
  const n = raw === null ? Number.NaN : Number.parseInt(raw, 10);
  return Number.isFinite(n) && n > 0 ? n : 0;
}

/** Records that the app was used now, so a later return measured from here. */
export function markActive(now = Date.now()) {
  store(STORAGE.lastActive, String(now));
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

export function setReduceTransparency(on: boolean) {
  set({ reduceTransparency: on });
  store(STORAGE.reduceTransparency, on ? "1" : "0");
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

/** A thread's own delegate limit, kept to 2..32. */
export function setThreadLimit(limit: number) {
  const n = Math.min(MAX_DELEGATES_PER_THREAD, Math.max(MIN_DELEGATES_PER_THREAD, Math.round(limit)));
  set({ threadLimit: n });
  store(STORAGE.threadLimit, String(n));
}

/** The machine-wide delegate total, kept to 2..64. */
export function setTotalLimit(limit: number) {
  const n = Math.min(MAX_DELEGATES_TOTAL, Math.max(MIN_DELEGATES_TOTAL, Math.round(limit)));
  set({ totalLimit: n });
  store(STORAGE.totalLimit, String(n));
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
  const mark = threadIsRemote(id) ? undefined : s.statusMarks[id];
  if (listed === "done" || listed === "review") return mark ?? listed;
  return mark ?? "idle";
}

function saveMarks(marks: Record<string, ThreadStatus>) {
  set({ statusMarks: marks });
  store(STORAGE.statusMarks, JSON.stringify(marks));
}

/**
 * Drop the marks the engine contradicts: it reports a status now, so a different one is the newer
 * truth — a settle or a review from the CLI, or another window — and the row has to say that. A
 * mark still covers an engine that reports no status at all (an older build, or a thread the list
 * has not caught up with).
 */
function reconcileStatusMarks(listed: SessionSummary[]) {
  const next = { ...get().statusMarks };
  let dropped = false;
  for (const x of listed) {
    const mark = next[x.id];
    if (!x.status || mark === undefined || mark === x.status) continue;
    delete next[x.id];
    dropped = true;
  }
  if (dropped) saveMarks(next);
}

export function clearStatusMark(id: string) {
  const { [id]: _gone, ...rest } = get().statusMarks;
  if (_gone) saveMarks(rest);
}

// ---------------------------------------------------------------- auto-compaction

/** How recently a delegate's parent must have worked for it to still count as talking to it. */
const DELEGATE_PARENT_ACTIVE_MS = 10 * 60_000;
/** One timer per thread: the next cache deadline, or an immediate size rule. */
const autoCompactTimers = new Map<string, ReturnType<typeof setTimeout>>();
/** Threads that already auto-compacted in this idle period, until the next turn starts. */
const autoCompactedIdle = new Set<string>();
/** The thread an auto-compaction runs in, so its card can say it ran on its own. */
const autoCompactInFlight = new Set<string>();
const autoCompactCardSeen = new Set<string>();
/** A compaction with no card waits this long for the usage that proves it moved. */
const AUTO_COMPACT_USAGE_WAIT_MS = 10_000;
/** Compactions whose notice waits for the post-compaction usage update. */
const autoCompactPendingNotice = new Set<string>();
const autoCompactNoticeTimers = new Set<ReturnType<typeof setTimeout>>();
/** The context before each automatic compaction, for the inspector's before → after. */
const autoCompactBefore = new Map<string, SessionUsage>();

function clearAutoCompactTimer(id: string): boolean {
  const timer = autoCompactTimers.get(id);
  if (timer === undefined) return false;
  clearTimeout(timer);
  autoCompactTimers.delete(id);
  return true;
}

export function setAutoCompactMode(mode: AutoCompactMode) {
  set((s) => ({ autoCompact: { ...s.autoCompact, mode } }));
  store(STORAGE.autoCompact, JSON.stringify(get().autoCompact));
  rescheduleAutoCompact();
}

export function setAutoCompactThreshold(tokens: number) {
  const threshold = clampAutoCompactThreshold(tokens);
  set((s) => ({ autoCompact: { ...s.autoCompact, threshold } }));
  store(STORAGE.autoCompact, JSON.stringify(get().autoCompact));
  rescheduleAutoCompact();
}

/** The thread's menu switch: true turns auto-compaction on for that thread again. */
export function setAutoCompactForThread(id: string, on: boolean) {
  set((s) => {
    const next = { ...s.autoCompactOff };
    if (on) delete next[id];
    else next[id] = true;
    return { autoCompactOff: next };
  });
  store(STORAGE.autoCompactOff, JSON.stringify(get().autoCompactOff));
  if (on) {
    autoCompactedIdle.delete(id);
    scheduleAutoCompact(id);
  } else {
    clearAutoCompactTimer(id);
  }
}

export function autoCompactOffFor(s: Pick<AppState, "autoCompactOff">, id: string): boolean {
  return s.autoCompactOff[id] === true;
}

function autoCompactSettingsFor(s: AppState, id: string): AutoCompactSettings {
  return autoCompactOffFor(s, id) ? { ...s.autoCompact, mode: "off" } : s.autoCompact;
}

function autoCompactPolicyFor(s: AppState, id: string) {
  const harness = harnessOf(s, id);
  const known = s.harnesses?.harnesses.find((h) => h.id === harness);
  return autoCompactPolicy(harness ?? "", known?.kind);
}

/** The threshold the context bar marks for this thread, if rule 1 can run on it. */
export function autoCompactThresholdFor(s: AppState, id: string): number | undefined {
  return autoCompactMark(autoCompactSettingsFor(s, id), autoCompactPolicyFor(s, id));
}

/** Whether the harness runs its own compaction. An ACP agent says so by advertising `/compact`. */
function canCompact(s: AppState, id: string): boolean {
  const harness = harnessOf(s, id);
  const known = s.harnesses?.harnesses.find((h) => h.id === harness);
  if (!known) return false;
  if (known.builtin || known.kind === "codex") return true;
  // Claude Code takes `/compact` whether or not its agent lists it as a command.
  if (known.id === "claude-code") return true;
  return commandsFor(s, id).some((command) => command.name === "/compact");
}

/** When the thread's last turn ended: where its prompt cache clock starts. */
function lastTurnEndedAt(thread: ThreadState | undefined): number | undefined {
  for (let i = (thread?.items.length ?? 0) - 1; i >= 0; i -= 1) {
    const item = thread!.items[i]!;
    if (item.kind === "turnEnd" && item.at !== undefined) return item.at;
  }
  return undefined;
}

/**
 * Whether the user stopped the thread's last turn, or it failed. The app's own quiet turns leave no
 * footer, so the last `turnEnd` is the last turn the thread ran for its work.
 */
function lastTurnInterrupted(thread: ThreadState | undefined): boolean {
  for (let i = (thread?.items.length ?? 0) - 1; i >= 0; i -= 1) {
    const item = thread!.items[i]!;
    if (item.kind === "turnEnd") return item.outcome !== "completed";
  }
  return false;
}

/**
 * Whether the thread still owes work it started: a delegate that is still working, or a command it
 * left running in the background. A thread that finished what it was doing has none, and a
 * compaction is not a reason to hand it another turn.
 */
function workInFlightOn(s: AppState, id: string): boolean {
  if (linksOf(s.links, id).driving.some((link) => isActive(link.state))) return true;
  return (s.threads[id]?.waiting ?? 0) > 0;
}

function autoCompactInput(s: AppState, id: string, now: number) {
  const thread = s.threads[id];
  const links = linksOf(s.links, id);
  const parentId = delegateParent(sessionById(s, id)?.parent)?.id;
  const parent = parentId ? s.sessions.find((x) => x.id === parentId) : undefined;
  const delegated = !!parentId || links.drivenBy !== undefined;
  // A delegate is kept only while its orchestrator is still talking to it: a
  // live link, or a parent turn/wake in the last ten minutes. A settled or
  // merely existing parent is not enough.
  const parentActive = parent?.updatedAt !== undefined && now - parent.updatedAt <= DELEGATE_PARENT_ACTIVE_MS;
  const orchestratorTalking = links.drivenBy !== undefined || parentActive;
  return {
    settings: autoCompactSettingsFor(s, id),
    policy: autoCompactPolicyFor(s, id),
    canCompact: canCompact(s, id),
    usedTokens: thread?.usage?.used,
    now,
    idleSince: lastTurnEndedAt(thread),
    status: statusOf(s, id),
    running: thread?.running ?? false,
    queued: thread?.queue.length ?? 0,
    approvals: thread?.permissions.length ?? 0,
    waiting: (thread?.waiting ?? 0) > 0,
    limitWaiting: s.limitWaits[id] !== undefined,
    snoozed: id in s.snoozed,
    // Done or Failed until the row is opened (`markFinished`, `clearFinished`). A delegate is
    // read by its orchestrator, not by the user, so its own flag says nothing about what they read.
    unread: !delegated && (id in s.finished || id in s.failed),
    typing: !!(s.drafts[id] ?? "").trim() || hasExtras(s.extras[id]),
    compactedThisIdle: autoCompactedIdle.has(id),
    delegated,
    orchestratorTalking,
  };
}

/** Whether a usage gauge shows the compaction the app asked for. */
function contextMoved(before: SessionUsage | undefined, after: SessionUsage | undefined): boolean {
  return (
    !!after &&
    (!before ||
      after.used < before.used ||
      (after.context?.compactions ?? 0) > (before.context?.compactions ?? 0))
  );
}

/**
 * The quiet row for a compaction whose harness sends no card (Claude Code).
 * It waits for the usage update that proves the context moved, so it does not
 * race the turn's reply.
 */
function settleAutoCompactNotice(id: string): void {
  if (!autoCompactPendingNotice.has(id)) return;
  autoCompactPendingNotice.delete(id);
  const before = autoCompactBefore.get(id);
  if (autoCompactCardSeen.has(id)) return;
  // The usage never proved it moved: "Compacting context…" must not stay up for ever.
  if (!contextMoved(before, get().threads[id]?.usage)) {
    updateThread(id, (thread) => settleCompaction(thread, null));
    return;
  }
  updateThread(id, (thread) => settleCompaction(thread, compactedNotice()));
}

/** The quiet row an automatic compaction leaves when its harness sent no card. */
function compactedNotice() {
  return {
    kind: "notice",
    id: localId("compaction"),
    text: "Compacted automatically.",
    tone: "info",
    setting: "auto-compact",
  } as const;
}

function runAutoCompact(id: string): void {
  const s = get();
  const decision = autoCompactDecision(autoCompactInput(s, id, Date.now()));
  if (decision.action !== "compact") return;
  autoCompactedIdle.add(id);
  const before = s.threads[id]?.usage;
  if (before) autoCompactBefore.set(id, before);
  autoCompactInFlight.add(id);
  autoCompactCardSeen.delete(id);
  autoCompactPendingNotice.delete(id);
  // What the compaction is about to hold up, read before the app's own turn starts: that turn clears
  // the thread's waiting state (thread.ts `beginQuietTurn`), and a compaction must not lose work.
  const wasWorking = workInFlightOn(s, id);
  const userStopped = lastTurnInterrupted(s.threads[id]);
  // Where the transcript is now: whatever a turn writes after it ran while the compaction did.
  const itemsBefore = s.threads[id]?.items.length ?? 0;
  void runPrompt(id, "/compact", emptyExtras, {
    automatic: true,
    autoCompact: true,
    autoCompactText: autoCompactingText(decision.rule, s.autoCompact.threshold),
  })
    // The compaction's own row is settled whether or not the prompt became a turn.
    .then(
      (outcome) => {
        settleAutoCompactTurn(id, before);
        return outcome;
      },
      () => {
        settleAutoCompactTurn(id, before);
        return "skipped" as TurnOutcome;
      },
    )
    .then((outcome) => {
      const after = get();
      const thread = after.threads[id];
      // A turn that ran since the compaction started: the queue's, which `runPrompt` starts when the
      // compaction's own turn ends. It is the thread's next step already.
      const carriedOn = (thread?.items.slice(itemsBefore) ?? []).some((item) => item.kind === "turnEnd");
      if (
        !shouldContinueAfterCompaction({
          outcome,
          // Read before the compaction, and again now: the work it held up must still be owed. A
          // delegate that settled while it ran is carried by its wake, not by a second turn.
          workInFlight: wasWorking && workInFlightOn(after, id),
          running: thread?.running ?? false,
          queued: thread?.queue.length ?? 0,
          // What the user is composing counts as having the thread back, text or attachments.
          typing: !!(after.drafts[id] ?? "").trim() || hasExtras(after.extras[id]),
          interrupted: userStopped,
          carriedOn,
          putAway: id in after.snoozed || statusOf(after, id) === "done",
        })
      ) {
        return;
      }
      // The compaction itself is the only turn that ran: the thread carries on with the work it
      // still owed, as its own divider row rather than a message the user typed.
      void runPrompt(id, CONTINUE_AFTER_COMPACTION_TEXT, emptyExtras, {
        automatic: true,
        continued: true,
      }).catch(() => undefined);
    })
    .catch(() => undefined);
}

/**
 * The app's compaction turn ended: its "Compacting context…" row becomes the card (a harness that
 * sends one), the "Compacted automatically" row, or the reason nothing came of it.
 */
function settleAutoCompactTurn(id: string, before: SessionUsage | undefined): void {
  autoCompactInFlight.delete(id);
  if (autoCompactCardSeen.has(id)) {
    autoCompactPendingNotice.delete(id);
    autoCompactCardSeen.delete(id);
    return;
  }
  if (contextMoved(before, get().threads[id]?.usage)) {
    updateThread(id, (thread) => settleCompaction(thread, compactedNotice()));
    return;
  }
  // The harness may send its usage just after the prompt returns: wait for
  // that event, bounded, instead of reading a stale gauge.
  autoCompactPendingNotice.add(id);
  const timer = setTimeout(() => {
    autoCompactNoticeTimers.delete(timer);
    settleAutoCompactNotice(id);
  }, AUTO_COMPACT_USAGE_WAIT_MS);
  autoCompactNoticeTimers.add(timer);
}

/**
 * Look at one thread and set its next auto-compaction timer. Called when a
 * turn ends, a thread opens, a draft changes or the setting does; the decision
 * itself is pure (`state/autoCompact.ts`).
 */
export function scheduleAutoCompact(id: string): void {
  clearAutoCompactTimer(id);
  const s = get();
  if (!s.threads[id]?.loaded) return;
  const decision = autoCompactDecision(autoCompactInput(s, id, Date.now()));
  if (decision.action === "compact") {
    autoCompactTimers.set(
      id,
      setTimeout(() => {
        autoCompactTimers.delete(id);
        runAutoCompact(id);
      }, 0),
    );
  } else if (decision.action === "wait") {
    // Timers cap at a day; a longer wait is rescheduled by the next check.
    const delay = Math.min(Math.max(0, decision.at - Date.now()), 24 * 60 * 60_000);
    autoCompactTimers.set(
      id,
      setTimeout(() => {
        autoCompactTimers.delete(id);
        scheduleAutoCompact(id);
      }, delay),
    );
  }
}

/** The user typing in a thread ends its idle period: give it back to them. */
function noteTyping(id: string): void {
  if (!get().threads[id]) return;
  clearAutoCompactTimer(id);
  autoCompactedIdle.add(id);
  // Typing in a thread whose automatic compaction already runs stops it, so
  // the user gets the thread back rather than waiting for a summary.
  if (autoCompactInFlight.has(id)) void cancel(id);
}

/** Start a fresh idle period: the next turn may auto-compact once more. */
function beginAutoCompactIdle(id: string): void {
  autoCompactedIdle.delete(id);
  autoCompactBefore.delete(id);
}

function rescheduleAutoCompact(): void {
  for (const id of Object.keys(get().threads)) scheduleAutoCompact(id);
}

/** The context an automatic compaction started from, for the inspector's comparison. */
export function autoCompactBeforeUsage(id: string): SessionUsage | undefined {
  return autoCompactBefore.get(id);
}

/** Drop every timer and per-thread record, for tests and when a thread is trashed. */
function forgetAutoCompact(id?: string): void {
  if (id === undefined) {
    for (const timer of autoCompactTimers.values()) clearTimeout(timer);
    for (const timer of autoCompactNoticeTimers) clearTimeout(timer);
    autoCompactTimers.clear();
    autoCompactNoticeTimers.clear();
    autoCompactedIdle.clear();
    autoCompactInFlight.clear();
    autoCompactCardSeen.clear();
    autoCompactPendingNotice.clear();
    autoCompactBefore.clear();
    return;
  }
  clearAutoCompactTimer(id);
  autoCompactedIdle.delete(id);
  autoCompactInFlight.delete(id);
  autoCompactCardSeen.delete(id);
  autoCompactPendingNotice.delete(id);
  autoCompactBefore.delete(id);
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
  const remote = ids.filter(threadIsRemote);
  let remoteErrors = 0;
  for (const id of remote) {
    try {
      const conn = connectionForThread(id);
      if (!conn) throw new Error("That computer is not connected.");
      await conn.request(V2Method.sessionStatus, { session_id: splitThreadKey(id).id, status }, 10_000);
      clearStatusMark(id);
      set((s) => ({
        sessions: s.sessions.map((x) => (x.id === id ? { ...x, status } : x)),
        ...(status === "done"
          ? { failed: Object.fromEntries(Object.entries(s.failed).filter(([key]) => key !== id)) }
          : {}),
      }));
    } catch (error) {
      remoteErrors++;
      toast(`Could not save the status: ${errorText(error)}`, "error");
    }
  }
  ids = ids.filter((id) => !threadIsRemote(id));
  if (!ids.length) return remoteErrors;
  const wanted = new Set(ids);
  set((s) => ({
    sessions: s.sessions.map((x) => (wanted.has(x.id) ? { ...x, status } : x)),
    // A settled thread asks nothing of the user: a turn of it that failed is no longer theirs to
    // retry (`cleanup_worktree`, a fan-out's losers, a delegate its parent replaced), so its
    // Inbox card goes with the settle.
    ...(status === "done"
      ? { failed: Object.fromEntries(Object.entries(s.failed).filter(([id]) => !wanted.has(id))) }
      : {}),
  }));
  if (status === "idle") for (const id of ids) clearStatusMark(id);
  else saveMarks({ ...get().statusMarks, ...Object.fromEntries(ids.map((id) => [id, status])) });
  for (const id of ids) scheduleAutoCompact(id);
  if (!get().features.status) return remoteErrors;
  // The status is kept by the computer the thread runs on, one request each on its own connection.
  const groups = new Map<AcpConnection, string[]>();
  const errors: unknown[] = [];
  for (const id of ids) {
    const conn = connectionForThread(id);
    if (conn) groups.set(conn, [...(groups.get(conn) ?? []), id]);
    // A local thread with no engine keeps the mark in the app only, as before; a machine's thread
    // that cannot be reached is said, since the machine will not have it.
    else if (threadIsRemote(id)) errors.push(new Error("That computer is not connected."));
  }
  for (const [conn, group] of groups) {
    for (const id of group) {
      try {
        await conn.request(V2Method.sessionStatus, { session_id: splitThreadKey(id).id, status }, 10_000);
        // The engine keeps it now; the app's mark only covers the gap until the list says so.
      } catch (error) {
        if (!(error instanceof RpcError && error.isMethodNotFound)) errors.push(error);
      }
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
  return errors.length + remoteErrors;
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
export async function settleThreads(ids: string[]): Promise<void> {
  const state = get();
  const before = ids.map((id) => ({ id, status: statusOf(state, id) })).filter((x) => x.status !== "done");
  if (!before.length) {
    toast("Already settled", "info");
    return;
  }
  const errors = await setThreadsStatus(
    before.map((x) => x.id),
    "done",
  );
  if (errors) return;
  if (
    get().view.kind === "chat" &&
    get().selectedId === state.selectedId &&
    state.selectedId &&
    before.some((x) => x.id === state.selectedId)
  ) {
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
export function settleThread(id: string): Promise<void> {
  return settleThreads([id]);
}

/**
 * Unsettles the settled threads among `ids` (the sidebar's selection bar and
 * the selection's menu): a Review mark on the others stays Review. Quiet on
 * purpose — the toast confirms the change and offers no Undo.
 *
 * `quiet` is for a message sent to a settled thread: it moves back to the
 * active list because the user wrote to it, not because they asked to unsettle
 * anything, so the send says nothing about it.
 */
export function unsettleThreads(ids: string[], options: { quiet?: boolean } = {}): void {
  const state = get();
  const settled = ids.filter((id) => (state.statusMarks[id] ?? sessionById(state, id)?.status) === "done");
  if (!settled.length) return;
  void setThreadsStatus(settled, "idle");
  if (options.quiet) return;
  toast(settled.length === 1 ? "Unsettled" : `Unsettled ${settled.length} threads`, "info");
}

export function setView(view: AppState["view"]) {
  dismissOverlays();
  set({ view, switcherOpen: false, settings: null });
  // The inbox lists what those toasts were about, so they have said their piece.
  if (view.kind === "inbox") dismissAttentionToasts();
}

/** The toasts that point at a thread needing the user; the inbox lists them all. */
const ATTENTION_TOASTS = ["A thread needs approval", "A thread has a question"];

/** Dismiss every attention toast already on screen, leaving the rest alone. */
export function dismissAttentionToasts() {
  for (const toast of get().toasts) {
    if (!toast.persist && ATTENTION_TOASTS.includes(toast.text)) dismissToast(toast.id);
  }
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
  // Checkpoints are files on the machine that ran the turn: this computer cannot read another's.
  if (threadIsRemote(id) || !connection || !get().features.checkpoints) return;
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
 * Rewind to before turn N: put the files back to that turn's checkpoint, and the conversation with them
 * when the harness can rewind. With `keep`, the state it abandons is kept as a checkpoint first,
 * so the attempt it leaves can be returned to. The restored files and the reloaded conversation
 * are the feedback; the caller says what happened to the transcript.
 */
async function restoreCheckpoint(
  id: string,
  turn: number,
  { rewind, keep }: { rewind: boolean; keep: boolean },
): Promise<RestoreAnswer | undefined> {
  try {
    const result = await engine().request(
      V2Method.checkpointRestore,
      { session_id: id, turn, rewind, keep },
      60_000,
    );
    const answer = parseRestoreAnswer(result);
    if (answer.conversation === "rewound") {
      // The conversation changed under the app: load it again.
      updateThread(id, (t) => ({ ...t, loaded: false }));
      set((s) => {
        const { [id]: _gone, ...threads } = s.threads;
        return { threads };
      });
      await openThread(id);
    }
    void refreshCheckpoints(id);
    return answer;
  } catch (error) {
    toast(`Could not rewind: ${errorText(error)}`, "error");
    return undefined;
  }
}

/** Whether this engine keeps the state a rewind abandons, so there is a way back to it. */
function keepsAbandonedState(): boolean {
  return !!get().agent?.checkpointAttempts;
}

/**
 * Rewind to a turn's checkpoint. The turns it leaves stay in the app as an earlier attempt, faded
 * under a divider below the transcript (`state/attempts.ts`), and the state they ended in is kept,
 * so "go back to this attempt" can put the files there again. A harness that cannot rewind its
 * conversation gets the files only, and says so: the thread stays as it is.
 */
export async function rewindCheckpoint(id: string, checkpoint: Checkpoint): Promise<void> {
  const thread = get().threads[id];
  if (!thread || thread.running) return;
  const rewind = canRewind(thread);
  const keep = rewind && keepsAbandonedState();
  const go = () => void runRewind(id, checkpoint.turn, { rewind, keep });
  if (!rewind) {
    askConfirm({
      title: `Put the files back to before turn ${checkpoint.turn}?`,
      message:
        "This harness cannot rewind the conversation, so the thread stays as it is and only the files go back. Later messages are kept.",
      action: "Files Only",
      destructive: true,
      run: go,
    });
    return;
  }
  if (!keep) {
    askConfirm({
      title: `Rewind to before turn ${checkpoint.turn}?`,
      message:
        "The files and the conversation go back to before this turn. This server cannot keep the state it leaves, so there is no way back to it.",
      action: "Rewind",
      destructive: true,
      run: go,
    });
    return;
  }
  go();
}

/** The rewind itself: what it abandons becomes an attempt, then the thread reads from the cut. */
async function runRewind(
  id: string,
  turn: number,
  { rewind, keep }: { rewind: boolean; keep: boolean },
): Promise<void> {
  const thread = get().threads[id];
  if (!thread) return;
  const tail = abandonsAt(thread, turn);
  const answer = await restoreCheckpoint(id, turn, { rewind, keep });
  if (!answer) return;
  if (answer.conversation !== "rewound") {
    toast(
      answer.rewindError
        ? `The files went back to before turn ${turn}. The conversation stayed: ${answer.rewindError}`
        : `The files went back to before turn ${turn}. The conversation stayed.`,
      answer.rewindError ? "error" : "success",
    );
    return;
  }
  if (!tail) {
    toast(`The files went back to before turn ${turn}.`, "success");
    return;
  }
  const attempt: Attempt = {
    id: localId("attempt"),
    at: Date.now(),
    turn,
    head: answer.kept,
    items: tail,
  };
  set((s) => ({ attempts: { ...s.attempts, [id]: [attempt, ...(s.attempts[id] ?? [])] } }));
}

/** The turns a rewind to `turn` abandons: from the message that started it to the transcript's end. */
function abandonsAt(thread: ThreadState, turn: number): TranscriptItem[] | undefined {
  const byEnd = checkpointsByTurn(transcriptTurns(thread.items), get().checkpoints[thread.id] ?? []);
  const end = Object.entries(byEnd).find(([, checkpoint]) => checkpoint.turn === turn)?.[0];
  if (!end) return undefined;
  return splitAtTurn(thread.items, end)?.tail;
}

/**
 * Go back to an earlier attempt: the files return to where it ended. The conversation cannot
 * follow (its messages are out of the thread's context for good), so the transcript stays as it
 * reads, and the confirm says so rather than pretending the two go together.
 */
export function returnToAttempt(threadId: string, attemptId: string): void {
  const attempt = get().attempts[threadId]?.find((a) => a.id === attemptId);
  const thread = get().threads[threadId];
  if (!attempt || attempt.head === undefined || !thread || thread.running) return;
  const head = attempt.head;
  askConfirm({
    title: "Go back to this attempt?",
    message:
      "The files go back to where this attempt ended. Its messages cannot: they are no longer in the thread's context, so the thread keeps reading as it does now.",
    action: "Restore the Files",
    run: () => {
      void restoreCheckpoint(threadId, head, { rewind: false, keep: false }).then((answer) => {
        if (answer) toast("Files restored to the earlier attempt", "success");
      });
    },
  });
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

/** A panel view: one of the side views, never closed. */
export type PanelKind = NonNullable<AppState["panel"]>;

/**
 * The view a thread with no panel open might show: its own overview. Changes,
 * Pull requests and the Thread tab are a tab away. It is only what ⌘⌥B opens on
 * a thread whose own panel was never opened; nothing opens it on its own.
 */
export const DEFAULT_PANEL: PanelKind = "details";

/** The memory without the oldest entries past `MAX_PANELS` (the map keeps insertion order). */
export function prunePanels(panels: Record<string, PanelKind | null>): Record<string, PanelKind | null> {
  const ids = Object.keys(panels);
  if (ids.length <= MAX_PANELS) return panels;
  return Object.fromEntries(ids.slice(ids.length - MAX_PANELS).map((id) => [id, panels[id]!]));
}

/** The memory from storage: this build's views and closes only, kept to the last `MAX_PANELS`. */
export function parsePanels(raw: unknown): Record<string, PanelKind | null> {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return {};
  const kept: Record<string, PanelKind | null> = {};
  for (const [id, panel] of Object.entries(raw as Record<string, unknown>)) {
    if (panel === null || PANELS.includes(panel as PanelKind)) kept[id] = panel as PanelKind | null;
  }
  return prunePanels(kept);
}

/**
 * The panel a thread opens with: the view it was last showing, or none when it
 * was closed by hand — and none for a thread never seen. A view this project or
 * thread cannot show right now (Pull requests in a project without them, the
 * Thread tab with nothing peeked) opens nothing rather than something else.
 */
function panelForThread(s: AppState, id: string | null): AppState["panel"] {
  if (!id) return null;
  const kept = s.panelByThread[id];
  if (!kept) return null;
  if (kept === "pulls" && !s.agent?.pullRequests) return null;
  if (kept === "peek" && !s.peekId) return null;
  return kept;
}

/**
 * The memory with `panel` kept for `id` as its newest entry, persisted; nothing
 * is kept on the new-thread screen, which has no thread.
 */
function keptPanels(id: string | null, panel: AppState["panel"]): Record<string, PanelKind | null> {
  const current = get().panelByThread;
  if (!id) return current;
  const { [id]: _before, ...rest } = current;
  const next = prunePanels({ ...rest, [id]: panel });
  store(STORAGE.panelByThread, JSON.stringify(next));
  return next;
}

/**
 * The state a thread opens with: selected, on the panel that thread was last
 * showing (none for one never seen, or one whose panel was closed by hand).
 */
function viewingThread(s: AppState, id: string | null): Pick<AppState, "selectedId" | "panel" | "peekId"> {
  // The Thread tab shows another thread beside this one: opening the thread it
  // shows ends the peek, so the panel goes back to this thread's own view.
  const shown = id && s.peekId === id ? { ...s, peekId: null } : s;
  return { selectedId: id, panel: panelForThread(shown, id), peekId: shown.peekId };
}

/** Opens `panel`, or closes it when it is the one open (the header's buttons). */
export function setPanel(panel: AppState["panel"]) {
  const s = get();
  const next = s.panel === panel ? null : panel;
  // Kept on the thread it was opened or closed on, so only that thread gets it back.
  set({ panel: next, panelByThread: keptPanels(s.selectedId, next) });
}

/** Shows a thread in the side panel, next to the one you are in. */
export function peekThread(id: string) {
  set({ peekId: id });
  showPanel("peek");
  void openThread(id);
}

/** Shows `panel` (the panel's tabs): never closes. */
export function showPanel(panel: PanelKind) {
  const s = get();
  set({ panel, panelByThread: keptPanels(s.selectedId, panel) });
}

/** ⌘⌥B: the right panel open or closed, with this thread's own view, else the overview. */
export function toggleRightPanel() {
  const s = get();
  const next = s.panel ? null : (panelForThread(s, s.selectedId) ?? DEFAULT_PANEL);
  set({ panel: next, panelByThread: keptPanels(s.selectedId, next) });
}

/** Whether the thread's harness lists subagents (`_rustcode/agents`). */
export function hasAgents(thread?: ThreadState): boolean {
  return !!get().agent?.agents && (thread?.harness?.capabilities.subagents ?? true);
}

export async function refreshAgents(id: string): Promise<SubagentInfo[]> {
  const thread = get().threads[id];
  const conn = connectionForThread(id);
  if (!conn || !hasAgents(thread)) return [];
  try {
    const agents = parseAgentList(
      await conn.request(Method.agents, { sessionId: splitThreadKey(id).id, action: "list" }, 10_000),
    );
    set((s) => ({ agents: { ...s.agents, [id]: agents } }));
    return agents;
  } catch {
    return get().agents[id] ?? [];
  }
}

/** A child's conversation, folded like a replayed thread. */
export async function agentHistory(id: string, agentId: number): Promise<ThreadState> {
  const conn = connectionForThread(id);
  if (!conn) throw new Error("That computer is not connected.");
  const result = (await conn.request(
    Method.agents,
    { sessionId: splitThreadKey(id).id, action: "history", id: agentId },
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
  s: Pick<AppState, "threads" | "sessions" | "newThread" | "harnesses"> & {
    newThreadHost?: string;
    remoteHarnesses?: Record<string, HarnessSnapshot>;
  },
  threadId?: string,
): string | undefined {
  if (!threadId)
    return s.newThread?.harness ?? harnessesOn(s, s.newThreadHost)?.default.harness ?? "rustcode";
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
  > & { newThreadHost?: string; remoteHarnesses?: Record<string, HarnessSnapshot> },
  threadId?: string,
): string | null {
  if (!threadId) {
    const harness = harnessOf(s);
    const known = harnessesOn(s, s.newThreadHost)?.harnesses.find((h) => h.id === harness);
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
> & { newThreadHost?: string; remoteHarnesses?: Record<string, HarnessSnapshot> };

/**
 * The slash commands a composer offers: an open thread reads its session's own list, a new one the
 * menu of the harness picked there. Until a thread's session has advertised (or for a new thread),
 * the menu is the engine's catalog with that harness's last list over it, so switching between
 * them never leaves another harness's commands behind.
 */
export function commandsFor(s: CommandsSource, threadId?: string | null): SlashCommand[] {
  const own = threadId ? s.sessionCommands[threadId] : undefined;
  // A session's own list is the harness's, which leaves out the skills RustCode discovered itself.
  if (own) return withSkills(own, s.baseCommands);
  const harness = harnessOf(s, threadId ?? undefined);
  const known = harnessesOn(s, threadId ? undefined : s.newThreadHost)?.harnesses.find(
    (h) => h.id === harness,
  );
  return menuCommands({
    base: s.baseCommands,
    seen: harness ? s.harnessCommands[harness] : undefined,
    // Before the harnesses are listed the engine answers the thread, so its catalog applies.
    builtin: known?.builtin ?? (!s.harnesses || harness === undefined || harness === "rustcode"),
  });
}

/** How often the app asks for every plan while its window is visible. */
export const PLAN_CACHE_POLL_MS = 180_000;

/**
 * How much before the poll interval an entry counts as stale. A read stamps
 * `fetchedAt` when it finishes, a moment after the timer that asked for it
 * fired, so comparing against the whole interval would always miss the next
 * tick and read every other one — every six minutes instead of three.
 */
export const PLAN_STALE_MARGIN_MS = 15_000;

/** True when a harness's numbers are missing or old enough for the next tick to read them again. */
export function planIsStale(plan: CachedPlan | undefined, now = Date.now()): boolean {
  return !plan?.fetchedAt || now - plan.fetchedAt >= PLAN_CACHE_POLL_MS - PLAN_STALE_MARGIN_MS;
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
  return w.resetsAt && w.resetsAt <= now ? { ...w, usedPercent: 0, resetsAt: undefined } : w;
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
        label: w.label,
      })),
      fetchedAt,
      name: p.name,
      plan: p.plan,
      harness,
      account: p.account ?? null,
      ...(p.asOf !== undefined ? { asOf: p.asOf } : {}),
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
  const conn = connectionForThread(id);
  if (!conn || planUsageInFlight.has(id)) return;
  planUsageInFlight.add(id);
  try {
    const usage = parsePlanUsage(
      await conn.request(Method.usageLimits, { sessionId: splitThreadKey(id).id }, 10_000),
    );
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

// Start Claude reads only after an explicit look or use of a Claude account.
let claudePlansStarted = false;

/**
 * Folds a plans answer into the cache: a signed-in harness's windows go in, one
 * that is signed out or gone loses its entry, and a Claude login that could not
 * be read stops the background poll asking for it again (a Deny in the Keychain
 * must not prompt every three minutes). An answer may hold only some rows, so
 * nothing it did not read is touched. `asked` is Settings' own call, which is
 * what unpauses them.
 */
export function commitPlansAnswer(answers: UsagePlan[], asked = false): void {
  if (asked) {
    claudePlansStarted = true;
    planPauses = NO_PAUSES;
  }
  const gone = answers.filter((p) => p.status === "signedOut" || p.status === "notInstalled").map(planKey);
  const kept = forgetPlans(get().planCache, gone);
  const learned = plansToCache(answers);
  setPlanCache(mergePlans(kept, learned));
  // Which logins are signed out, for the places that must not offer one: the
  // picker's account chips and a new thread's account (state/logins.ts).
  set({ signedOutLogins: signedOutFrom(answers, get().signedOutLogins) });
  // A login whose read was denied, or that is signed out, pauses its own row
  // only: the default login's failure must not stop a named account's meter,
  // and an account without a sign-in must not stop the default login's.
  planPauses = pausesAfter(answers, planPauses);
}

/**
 * What a refresh asks the engine for: the default login and the account of the
 * thread on screen, after first use and while it was not paused.
 */
function currentPlansAsk(explicit: boolean): PlansAsk {
  const s = get();
  const threadId = s.selectedId ?? undefined;
  if (explicit) claudePlansStarted = true;
  return plansAskFor({
    allowed: claudePlansStarted,
    harness: harnessOf(s, threadId),
    account: accountOn(s, threadId),
    pauses: planPauses,
    explicit,
  });
}

/**
 * Asks for every subscription's plan (`_rustcode/usage/plans`, no thread
 * needed) and folds the answer into the cache. Claude is asked for after first use: the default login, and the account of the thread on screen so its
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

/**
 * What a background read asks for, for a caller that sends its own request
 * (Settings › Usage's own interval): the default login and the account of the
 * thread on screen, each only with consent and while the last read did not
 * pause it (`state/plans.ts`). Settings' explicit look asks for every login
 * instead, which is what `_rustcode/usage/plans`'s `claudeAccounts` does.
 */
export function backgroundPlansAsk(): PlansAsk {
  return currentPlansAsk(false);
}

/** The harness whose plans come from named logins (`acp/rustcode.ts` `harnessPlanKey`). */
const CLAUDE_CODE = "claude-code";

/** When the named logins were last asked for, so that it happens once per poll. */
let planLoginsAskedAt = 0;

/**
 * Asks in the background for the numbers a caller that can only report the
 * cache needs newer ones of (`list_harnesses`): the app's own ask — the default
 * login and the account of the thread on screen (`currentPlansAsk`) — and every
 * named login that was read before and has gone stale, which the poll alone
 * never reads.
 *
 * A login nothing was read for is never asked for: reading an unseen Keychain
 * item can prompt macOS, and the user never asked for that. Neither is one a
 * failed read paused (`failurePauses` in `state/plans.ts`). All of it happens
 * at most once per poll interval however often it is called, and the requests
 * queue behind the one in flight, so one Keychain item is read at a time. The
 * caller's answer stays cache-only: this never awaits.
 */
export function refreshReadPlanLogins(now = Date.now()): void {
  if (!connection || get().status !== "ready") return;
  if (now - planLoginsAskedAt < PLAN_CACHE_POLL_MS) return;
  planLoginsAskedAt = now;
  const cache = get().planCache;
  const prefix = `${CLAUDE_CODE}#`;
  const stale = Object.keys(cache)
    .filter((key) => key.startsWith(prefix))
    .map((key) => key.slice(prefix.length))
    .filter((id) => planIsStale(cache[harnessPlanKey(CLAUDE_CODE, id)], now))
    .filter((id) => !planPauses.accounts.has(id));
  for (const account of stale) plansQueued = queueAsks(plansQueued, { claude: false, account });
  // One request at a time: with one in flight its loop drains these asks, and
  // otherwise the app's own ask goes first and they follow it.
  if (!plansInFlight) void refreshPlanCache();
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
 * longer than that. A hidden window asks for nothing, and a cache another read
 * just filled (Settings › Usage reads the same subscriptions) is left alone.
 */
export function startPlanPolling() {
  if (planTimer) return;
  planTimer = setInterval(() => {
    if (typeof document !== "undefined" && document.hidden) return;
    if (!planCacheStale(get().planCache)) return;
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
  claudePlansStarted = false;
  if (plansOnVisible) document.removeEventListener("visibilitychange", plansOnVisible);
  plansOnVisible = null;
  plansInFlight = null;
  plansQueued = [];
  planLoginsAskedAt = 0;
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

// ---------------------------------------------------------------- remote access

// Every read of the machine's remote state runs on one clock. A change made here (pair, answer,
// revoke, the switch) or a dismissed QR code bumps the revision, so a read that started before it
// cannot put back what it changed; among the reads that started after, only the newest lands. All
// of them, and every mutation's answer, land through `applyRemoteStatus`, which keeps or clears the
// QR challenge and notices a device that just paired.
function invalidateRemoteStatusReads() {
  remoteStatusRevision += 1;
  latestRemoteStatusReadId = remoteStatusReadId;
}

function commitRemoteStatus(status: RemoteStatus) {
  invalidateRemoteStatusReads();
  applyRemoteStatus(status);
}

export function dismissRemoteChallenge() {
  invalidateRemoteStatusReads();
  set({ remoteChallenge: null });
}

export function applyRemoteStatus(status: RemoteStatus, now = Date.now()) {
  set((s) => {
    const challenge = s.remoteChallenge;
    const fresh = challenge && newPairedDevice(challenge.baselineIds, status.devices);
    return {
      remote: status,
      remoteError: null,
      remotePaired: fresh ? { name: fresh.name } : status.enabled ? s.remotePaired : null,
      remoteChallenge: challenge && !fresh && challengeIsValid(challenge, status, now) ? challenge : null,
    };
  });
}

/**
 * Re-read the machine's remote access state. `sessionId` reads one thread's
 * share instead, for its header indicator. Never throws: Settings shows the
 * engine's refusal next to the switch.
 */
export async function refreshRemote(sessionId?: string): Promise<RemoteStatus | null> {
  const params: JsonObject = sessionId ? { sessionId } : {};
  const revision = remoteStatusRevision;
  const readId = sessionId ? 0 : ++remoteStatusReadId;
  try {
    const status = parseRemoteStatus(await request(RemoteMethod.status, params, 10_000));
    if (sessionId)
      set((s) => ({
        remoteByThread: { ...s.remoteByThread, [sessionId]: status },
        remoteError: null,
      }));
    else if (revision === remoteStatusRevision && readId > latestRemoteStatusReadId) {
      latestRemoteStatusReadId = readId;
      applyRemoteStatus(status);
    }
    return sessionId || (revision === remoteStatusRevision && readId === latestRemoteStatusReadId)
      ? status
      : get().remote;
  } catch (error) {
    // The engine answered nothing (a restart, a dropped request): say so and
    // keep what was last true, rather than showing remote access as off.
    if (sessionId || (revision === remoteStatusRevision && readId > latestRemoteStatusReadId)) {
      if (!sessionId) latestRemoteStatusReadId = readId;
      set({ remoteError: errorText(error) });
    }
    return null;
  }
}

/**
 * Switch remote access on or off for every thread on this Mac. On starts the
 * gateway and shares every open thread; off stops the gateway, so no device
 * sees anything until it is switched on again. Pairings are kept.
 */
export async function setRemoteAccess(on: boolean): Promise<void> {
  invalidateRemoteStatusReads();
  if (!on) set({ remoteChallenge: null, remotePaired: null });
  if (on) {
    set({
      remote: {
        ...(get().remote ?? {}),
        available: true,
        enabled: true,
        shared: false,
        pairedDevices: 0,
        devices: [],
      } as RemoteStatus,
    });
  }
  try {
    const status = parseRemoteStatus(await request(on ? RemoteMethod.start : RemoteMethod.stop, {}, 20_000));
    commitRemoteStatus(status);
    if (status.error) toast(status.error, "error");
    else {
      toast(
        on
          ? "Remote access is on. Every open thread is shared with paired devices."
          : "Remote access is off. Paired devices stay paired.",
        "success",
      );
    }
    // Every open thread's share changed with it.
    for (const threadId of Object.keys(get().remoteByThread)) void refreshRemote(threadId);
  } catch (error) {
    toast(errorText(error), "error");
    void refreshRemote();
  }
}

/**
 * Ask the gateway for a pairing challenge. Remote access is switched on first
 * if it was off, as `/remote` does in a terminal.
 */
export async function pairRemoteDevice(): Promise<RemoteStatus | null> {
  const baselineIds = (get().remote?.devices ?? []).map((device) => device.id);
  invalidateRemoteStatusReads();
  const revision = remoteStatusRevision;
  set({ remoteChallenge: null, remotePaired: null });
  try {
    const status = parseRemoteStatus(await request(RemoteMethod.pair, {}, 20_000));
    if (revision === remoteStatusRevision) {
      const fresh = newPairedDevice(baselineIds, status.devices);
      const instanceChanged =
        status.offer?.instanceId &&
        status.gateway?.instanceId &&
        status.offer.instanceId !== status.gateway.instanceId;
      set({
        remotePaired: fresh ? { name: fresh.name } : null,
        remoteChallenge:
          status.offer && !fresh && !instanceChanged
            ? makeChallenge(status.offer, Date.now(), baselineIds)
            : null,
      });
      commitRemoteStatus(status);
    } else {
      // Dismissed, switched off or answered meanwhile: the code is not shown, but the machine's
      // state did change, so read it again.
      void refreshRemote();
    }
    if (status.pairingError) toast(status.pairingError, "error");
    return status;
  } catch (error) {
    toast(errorText(error), "error");
    return null;
  }
}

/**
 * Accept or deny a phone that asked to connect. Accepting shows the code the phone needs
 * (`connectCode`); the request's own prompt goes with the next status either way.
 */
export async function answerConnectRequest(id: string, accept: boolean): Promise<boolean> {
  invalidateRemoteStatusReads();
  try {
    const status = parseRemoteStatus(await request(RemoteMethod.answer, { id, accept }, 15_000));
    commitRemoteStatus(status);
    if (status.connectError) {
      toast(status.connectError, "error");
      return false;
    }
    const answer = status.connectAnswer;
    if (accept && answer?.code) {
      set({
        connectCode: {
          requestId: answer.id,
          deviceName: answer.deviceName,
          code: answer.code,
          instanceId: answer.instanceId ?? status.gateway?.instanceId,
          expiresAt: Date.now() + (answer.expiresInSecs ?? 120) * 1000,
          lockedForSecs: answer.lockedForSecs,
        },
      });
    }
    return true;
  } catch (error) {
    toast(errorText(error), "error");
    return false;
  }
}

/** Take the connection code off the screen. */
export function closeConnectCode() {
  set({ connectCode: null });
}

/** Forget a paired device: its token stops working and its connections close. */
export async function revokeRemoteDevice(id: string): Promise<void> {
  invalidateRemoteStatusReads();
  try {
    const status = parseRemoteStatus(await request(RemoteMethod.revoke, { deviceId: id }, 15_000));
    commitRemoteStatus(status);
    if (status.revokeError) toast(status.revokeError, "error");
    else if (status.revoked) toast(`${status.revoked.name} can no longer connect.`, "success");
  } catch (error) {
    toast(errorText(error), "error");
  }
}

/** Test hook: reset to a clean slate. */
export function resetForTests() {
  // A machine's channel and its threads are process-global too: one test file's MockMachine must
  // not answer the next one's `openThread` (machinesThreads.test.ts › findings 1).
  resetMachinesForTests();
  lastFocusListAt = 0;
  remoteStatusRevision += 1;
  remoteStatusReadId = 0;
  latestRemoteStatusReadId = 0;
  clearTimeout(slowTimer);
  if (limitTimer) clearInterval(limitTimer);
  limitTimer = null;
  clearPageCache();
  stopPlanPolling();
  resetWaits();
  forgetAutoCompact();
  engineWatch?.();
  engineWatch = undefined;
  crashed = false;
  modelsAsked.clear();
  connection = null;
  transport = null;
  setShellTransport(null);
  loading.clear();
  opening.clear();
  set({
    status: "connecting",
    remote: null,
    remoteChallenge: null,
    remotePaired: null,
    remoteByThread: {},
    remoteError: null,
    engineLog: [],
    sessions: [],
    sessionsLoaded: false,
    sessionsCapped: false,
    threads: {},
    selectedId: null,
    projectCwd: "",
    defaultCwd: "",
    newThread: null,
    newThreadHost: LOCAL_HOST,
    sidebarDevice: null,
    remoteHarnesses: {},
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
    connectCode: null,
    limitWaits: {},
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
    permissionModes: {},
    newThreadPermissionMode: null,
    view: { kind: "chat" },
    checkpoints: {},
    attempts: {},
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
  notifyThread({ threadId: id, kind: "limit", key: String(wait.until), title: threadName(id) }, get());
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

// What follows the app rather than a moment in it: the Dock badge, and a click on a notification
// opening its thread (state/notify.ts).
startNotifications({ get: () => useApp.getState(), subscribe: useApp.subscribe, open: selectThread });

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
 * without the user pressing anything. Settings › General turns it off. Up to {@link RESUME_MAX}
 * are resumed, and only the ones that really get a turn count: a thread the app cannot open, or
 * one whose resume the app could not send, does not use up someone else's slot.
 */
async function resumeInterrupted(ids: string[]): Promise<void> {
  store(STORAGE.runningThreads, "[]");
  lastRunning = "";
  if (!get().resumeAfterRestart || !ids.length) return;
  const known = new Set(get().sessions.map((x) => x.id));
  let started = 0;
  for (const id of ids) {
    if (started >= RESUME_MAX) break;
    if (!known.has(id)) continue;
    await openThread(id);
    // The selected thread may already be loading from the handshake: wait for it.
    for (let i = 0; i < 100 && !get().threads[id]?.loaded; i++) await new Promise((r) => setTimeout(r, 100));
    const thread = get().threads[id];
    if (!thread?.loaded) continue;
    started++;
    // The links the restart severed are this turn's to answer, not any later turn's
    // (state/host.ts `linksResumed`, `linksFinished`): the resume carries them, so a parent that
    // was waiting reads the answer in a wake instead of waiting for good. A thread whose turn still
    // runs (the engine outlived the window) takes the resume behind it as a queued row, which then
    // starts one that carries the links — dropping it would settle nothing again.
    void runPrompt(id, RESUME_TEXT, undefined, {
      automatic: true,
      resumeLinks: interruptedLinks(id),
    });
  }
  if (started) {
    toast(
      started === 1
        ? "A thread the restart interrupted is continuing."
        : `${started} threads the restart interrupted are continuing.`,
      "success",
    );
  }
}
