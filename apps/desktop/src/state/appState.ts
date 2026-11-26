// The app state's shape, its settings sections and defaults, and the storage keys it persists under.

import type { RemoteStatus } from "../acp/remote";
import type { PlanUsage, SubagentInfo } from "../acp/rustcode";
import type { Transport } from "../acp/transport";
import type { AgentInfo, HarnessSnapshot, SessionSummary, SlashCommand } from "../acp/types";
import type { Checkpoint, ThreadStatus } from "../acp/v2";
import { CHAT_WIDTH_DEFAULT, type ChatWidth } from "../lib/chatWidth";
import { SHORTCUTS_KEY } from "../lib/keys";
import {
  DEFAULT_NOTIFY_SOUND_ENABLED,
  DEFAULT_NOTIFY_SOUND_TONE,
  DEFAULT_NOTIFY_SOUND_VOLUME,
  type NotifySoundTone,
} from "../lib/notifySound";
import { readStorage, writeStorage } from "../lib/storage";
import type { ThemesSnapshot } from "../lib/themes";
import type { ResolvedApproval } from "./approvals";
import type { Attempt } from "./attempts";
import { type AutoCompactSettings, DEFAULT_AUTO_COMPACT } from "./autoCompact";
import type { HarnessCommands } from "./commands";
import type { ComposeExtras } from "./compose";
import type { ConnectCode } from "./connectRequests";
import type { EngineState } from "./engine";
import type { BrowserGrants, TimedGrant } from "./grants";
import { DEFAULT_HEADER_BUTTONS, type HeaderButtonId } from "./headerButtons";
import type { InboxDismissals } from "./inboxDismissed";
import type { InternetWait } from "./internetWait";
import type { FallbackRule } from "./limitFallback";
import type { LimitWait } from "./limitResume";
import type { SignedOutLogins } from "./logins";
import {
  DEFAULT_DELEGATES_PER_THREAD,
  DEFAULT_DELEGATES_TOTAL,
  type GrantAsk,
  type LinkBook,
  MAX_DELEGATES_PER_THREAD,
  MAX_DELEGATES_TOTAL,
  MIN_DELEGATES_PER_THREAD,
  MIN_DELEGATES_TOTAL,
} from "./orchestration";
import { type PermissionMode, parsePermissionMode } from "./permissions";
import { type OpenRecap, RECAP_MINUTES_DEFAULT } from "./recap";
import type { PhonePairing } from "./remotePairing";
import type { ScheduleBook } from "./schedule";
import type { PendingSetup } from "./setup";
import type { SidebarOrder } from "./sidebarOrder";
import type { AppFont, PanelKind, TextSize } from "./store";
import type { ThreadState } from "./thread";
import type { Toast } from "./toasts";

export type SettingsSection =
  | "general"
  | "chat"
  | "permissions"
  | "agents"
  | "orchestration"
  | "voice"
  | "connections"
  | "capabilities"
  | "remote"
  | "outside"
  | "machines"
  | "appearance"
  | "keyboard"
  | "usage"
  | "statistics"
  | "diagnostics"
  | "about";

/** Every Settings page, in the order the panel's nav lists them. */
export const SETTINGS_SECTIONS: readonly SettingsSection[] = [
  "general",
  "chat",
  "permissions",
  "appearance",
  "keyboard",
  "agents",
  "orchestration",
  "capabilities",
  "connections",
  "voice",
  "machines",
  "remote",
  "outside",
  "usage",
  "statistics",
  "diagnostics",
  "about",
];

/**
 * What an older build's section id opens: the Harnesses page keeps its old
 * `agents` id, so a stored `harnesses` opens it, and Engine and later Advanced became part of
 * Diagnostics.
 * An id no build knows opens General.
 */
const LEGACY_SECTIONS: Record<string, SettingsSection> = {
  harnesses: "agents",
  engine: "diagnostics",
  advanced: "diagnostics",
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
  /** This computer's engine connection (`state/engine.ts`); `engine.phase === "ready"` is when it answers. */
  engine: EngineState;
  transport?: Transport["name"];
  engineLog: string[];
  agent?: AgentInfo;
  sessions: SessionSummary[];
  /** Each paired machine's name, by host id: the sidebar, the header and a notice say where. */
  machineNames: Record<string, string>;
  /** The paired machines whose connection is open now: their threads can be driven, not only read. */
  connectedHosts: string[];
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
  /** Unstarted new-thread model/effort drafts by host (`LOCAL_HOST` or machine id). */
  newThreadDrafts: Record<string, NewThreadChoice>;
  /** Unstarted new-thread account drafts by host (`LOCAL_HOST` or machine id). */
  newThreadAccounts: Record<string, string | null>;
  harnesses: HarnessSnapshot | null;
  /**
   * The computer the next new thread runs on: `LOCAL_HOST` for this one, or a paired
   * machine's id (`state/machines.ts`). The composer's **Run on** sets it, and a new
   * thread started from a machine's group in the sidebar starts there
   * (docs/desktop.md › Several machines).
   */
  newThreadHost: string;
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
  /** Messages written now and sent at a time the user picked, by thread id (state/schedule.ts). */
  scheduled: ScheduleBook;
  settings: SettingsSection | null;
  /** Which machine's own page Settings › Machines shows, or null for the list. */
  settingsMachine: string | null;
  /** The Settings row to scroll to and light once its page is drawn (a search result picked). */
  settingsLanding: { section: SettingsSection; title: string } | null;
  switcherOpen: boolean;
  /** A report command's answer (`/status`, `/context`…), shown over the composer of its thread. */
  report: { threadId: string; text: string } | null;
  /** A fresh request from `/model` or the model picker shortcut. */
  modelPickerAsk: { toggle: boolean } | null;
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
  /** Threads can start and message other threads (Settings › Delegation); the engine learns it at initialize. */
  orchestration: boolean;
  /** How many delegates one thread may run at once (Settings › Delegation). */
  threadLimit: number;
  /** How many delegates may run at once across this app (Settings › Delegation). */
  totalLimit: number;
  /** Trust this computer's named logins for delegation (default off). */
  trustDelegateAccounts: boolean;
  /** Agents may read threads of every project, not only the calling thread's (Settings › Delegation). */
  readAcrossProjects: boolean;
  /**
   * A delegate that stops on a usage limit goes on at once on the table's model of the other
   * provider instead of waiting for the reset (issue 3392). Server settings; read at start.
   */
  limitFallback: boolean;
  limitFallbackMap: FallbackRule[];
  /**
   * The window, in seconds, in which a parent's delegate finishes and stalls are merged into one
   * wake turn (`wake-batch-seconds`, issue 3649); 0 sends them as they come. Server setting.
   */
  wakeBatchSeconds: number;
  /** Threads that were running when the app quit or the engine restarted continue on their own. */
  resumeAfterRestart: boolean;
  /** A system notification when a thread needs you or finishes (Settings › General). */
  systemNotifications: boolean;
  /** Those notifications only while the app is in the background (Settings › General). */
  notificationsBackgroundOnly: boolean;
  /** An optional audio cue when a background thread needs you or finishes (Settings › General). */
  notificationSound: boolean;
  /** The synthesized tone played for notification audio cues. */
  notificationSoundTone: NotifySoundTone;
  /** Volume percentage (0–100) for notification audio cues. */
  notificationSoundVolume: number;
  /** A recap at the top of a chat the user comes back to after a while (Settings › General). */
  chatRecaps: boolean;
  /** How long a chat goes unseen before that recap is due, in minutes. */
  recapMinutes: number;
  /** The chats whose recap is open, each with when it went quiet (`state/recap.ts`). */
  recaps: Record<string, OpenRecap>;
  /** A thread stopped by its plan's limit continues when the limit resets. */
  continueAfterLimit: boolean;
  /** The threads waiting on that, by thread id (state/limitResume.ts). */
  limitWaits: Record<string, LimitWait>;
  /** The threads waiting for the internet, by thread id (state/internetWait.ts). */
  internetWaits: Record<string, InternetWait>;
  /** The interface's typeface: Inter (the default), the system's own, or SF Pro Rounded. */
  font: AppFont;
  /** Text scale: small is Default in Appearance; medium and large are Large and Larger. */
  textSize: TextSize;
  /** A streaming reply's new words fade in as the model writes them. */
  animateStreaming: boolean;
  /** Glass is solid on every OS (Settings › Appearance); the system's own request does the same. */
  reduceTransparency: boolean;
  /** How much of the window shows through the sidebar, in percent (Settings › Appearance). */
  transparency: number;
  /** How wide the chat lane grows, transcript and composer alike (Settings › Chat). */
  chatWidth: ChatWidth;
  /** A turn shows its tool calls (Settings › Chat); off, only its "Worked for" line and the reply. */
  showToolCalls: boolean;
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
  /**
   * Threads the engine's list has said are running two reads in a row while this window holds no
   * running turn for them (another client's turn, a wake, a turn started before a reload): they
   * read as running until the engine says otherwise (state/runState.ts).
   */
  engineRunning: Record<string, true>;
  /** How RustCode compacts a thread on its own (Settings › General › Threads). */
  autoCompact: AutoCompactSettings;
  /** Threads whose menu turned auto-compaction off, by thread id. */
  autoCompactOff: Record<string, boolean>;
  /** Threads out of the list until a time (state/snooze.ts). */
  snoozed: Record<string, number>;
  /** The thread the custom snooze dialog is open for. */
  snoozeAsk: string | null;
  /** What threads may do without asking (Settings › Permissions › Default access)... */
  defaultPermissionMode: PermissionMode;
  /** ...a project's own default, by project path, over the one above (Settings › Permissions)... */
  projectPermissionModes: Record<string, PermissionMode>;
  /**
   * ...each thread's own, only once someone chose it for that thread (the composer's pill, or a
   * delegate's start). A thread without one follows its project's default, then the default.
   */
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
  /** The sites each thread may use the browser on without a card, by thread id. */
  browserGrants: BrowserGrants;
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
  pairing: PhonePairing;
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
    | { kind: "loops" }
    | { kind: "dashboard" }
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
  /** Starts left running when the reader opens another thread or a fresh composer. */
  backgroundSetups: Record<string, PendingSetup>;
}

/** A cached plan older than this is dropped on launch: its windows have moved on. */
export const PLAN_CACHE_MAX_AGE_MS = 24 * 60 * 60_000;

/** How many threads keep their panel; past this the oldest entry is dropped. */
export const MAX_PANELS = 500;

/**
 * The views the panel can show. Declared before the store: `parsePanels` reads it while the store
 * is created, and a `const` further down is not initialized yet then (a blank window on start).
 */
export const PANELS: readonly PanelKind[] = ["details", "changes", "pulls", "peek"];

export const STORAGE = {
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
  trustDelegateAccounts: "rustcode.trustDelegateAccounts",
  readAcrossProjects: "rustcode.readAcrossProjects",
  resumeAfterRestart: "rustcode.resumeAfterRestart",
  systemNotifications: "rustcode.systemNotifications",
  notificationsBackgroundOnly: "rustcode.notificationsBackgroundOnly",
  notificationSound: "rustcode.notificationSound",
  notificationSoundTone: "rustcode.notificationSoundTone",
  notificationSoundVolume: "rustcode.notificationSoundVolume",
  /** Kept from the while-you-were-away recap, so turning that off also turns chat recaps off. */
  chatRecaps: "rustcode.recapWhileAway",
  recapMinutes: "rustcode.recapMinutes",
  /** When each chat was last on screen, so a return knows how long it went unseen. */
  threadSeen: "rustcode.threadSeenAt",
  continueAfterLimit: "rustcode.continueAfterLimit",
  limitWaits: "rustcode.limitWaits",
  /** The threads waiting for the internet to come back, by thread id. */
  internetWaits: "rustcode.internetWaits",
  /** The ids of the threads running a turn, kept while the engine is up. */
  runningThreads: "rustcode.runningThreads",
  /** The threads the user stopped and no turn has run on since: a restart does not resume them. */
  stoppedThreads: "rustcode.stoppedThreads",
  /** When a restart last resumed each thread, so a resume cut off again is not repeated in a loop. */
  resumedThreads: "rustcode.resumedThreads",
  roundedFont: "rustcode.roundedFont",
  font: "rustcode.font",
  textSize: "rustcode.textSize",
  animateStreaming: "rustcode.animateStreaming",
  reduceTransparency: "rustcode.reduceTransparency",
  transparency: "rustcode.transparency",
  chatWidth: "rustcode.chatWidth",
  showToolCalls: "rustcode.showToolCalls",
  headerButtons: "rustcode.headerButtons",
  statusMarks: "rustcode.statusMarks",
  snoozed: "rustcode.snoozed",
  permissionMode: "rustcode.permissionMode",
  permissionModes: "rustcode.permissionModes",
  projectPermissionModes: "rustcode.projectPermissionModes",
  /** Set once the pins older builds wrote for every thread were thinned (`loadPermissionPins`). */
  permissionPinsThinned: "rustcode.permissionModes.thinned",
  /** Each harness's last new-thread choice, for an engine that does not remember it. */
  harnessChoices: "rustcode.harnessChoices",
  /** Each harness's last account picked for new threads, by harness id. */
  harnessAccountChoices: "rustcode.harnessAccountChoices",
  /** Unstarted new-thread model/effort drafts by host, kept across navigation and restarts. */
  newThreadDrafts: "rustcode.newThreadDrafts",
  /** Unstarted new-thread account drafts by host, kept across navigation and restarts. */
  newThreadAccounts: "rustcode.newThreadAccounts",
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
  font: "inter" as AppFont,
  textSize: "small" as TextSize,
  animateStreaming: true,
  reduceTransparency: false,
  transparency: 28,
  chatWidth: CHAT_WIDTH_DEFAULT,
  showToolCalls: true,
  headerButtons: DEFAULT_HEADER_BUTTONS,
  resumeAfterRestart: true,
  systemNotifications: true,
  notificationsBackgroundOnly: true,
  notificationSound: DEFAULT_NOTIFY_SOUND_ENABLED,
  notificationSoundTone: DEFAULT_NOTIFY_SOUND_TONE,
  notificationSoundVolume: DEFAULT_NOTIFY_SOUND_VOLUME,
  chatRecaps: true,
  recapMinutes: RECAP_MINUTES_DEFAULT,
  continueAfterLimit: true,
  orchestration: true,
  threadLimit: DEFAULT_DELEGATES_PER_THREAD,
  totalLimit: DEFAULT_DELEGATES_TOTAL,
  trustDelegateAccounts: true,
  readAcrossProjects: true,
  smartUi: true,
  autoCompact: DEFAULT_AUTO_COMPACT,
  defaultPermissionMode: parsePermissionMode(null),
};

export function stored(key: string): string | null {
  return readStorage(key);
}

/** A boolean kept as "1"/"0", or the default when it was never set. */
export function storedFlag(key: string, fallback: boolean): boolean {
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

/** Settings › Appearance › Transparency, kept to the slider's 0..100. */
export function storedTransparency(): number {
  return storedNumber(STORAGE.transparency, SETTINGS_DEFAULTS.transparency, 0, 100);
}

/** A thread's own delegate limit, kept to 2..32. */
export function storedThreadLimit(key: string, fallback: number): number {
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

/** Storage can be unavailable (private window); the setting just does not persist. */
export function store(key: string, value: string) {
  writeStorage(key, value);
}

export function storedJson<T>(key: string, fallback: T): T {
  try {
    const raw = stored(key);
    return raw ? (JSON.parse(raw) as T) : fallback;
  } catch {
    return fallback;
  }
}

/** Reads persisted per-host new-thread model drafts, dropping corrupt entries. */
export function parseNewThreadDrafts(raw: string | null): Record<string, NewThreadChoice> {
  try {
    const parsed = raw ? (JSON.parse(raw) as unknown) : null;
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return {};
    const out: Record<string, NewThreadChoice> = {};
    for (const [host, value] of Object.entries(parsed as Record<string, unknown>)) {
      if (!value || typeof value !== "object" || Array.isArray(value)) continue;
      const entry = value as Record<string, unknown>;
      if (typeof entry.harness !== "string" || !entry.harness) continue;
      const model = typeof entry.model === "string" && entry.model ? entry.model : null;
      const effort = typeof entry.effort === "string" && entry.effort ? entry.effort : null;
      out[host] = { harness: entry.harness, model, effort };
    }
    return out;
  } catch {
    return {};
  }
}

/** Reads persisted per-host new-thread account drafts, dropping corrupt entries. */
export function parseNewThreadAccounts(raw: string | null): Record<string, string | null> {
  try {
    const parsed = raw ? (JSON.parse(raw) as unknown) : null;
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return {};
    const out: Record<string, string | null> = {};
    for (const [host, value] of Object.entries(parsed as Record<string, unknown>)) {
      if (value === null || (typeof value === "string" && value)) out[host] = value;
    }
    return out;
  } catch {
    return {};
  }
}
