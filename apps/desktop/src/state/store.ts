// App state (Zustand) and the engine controller that drives it over ACP.
// Components read slices with `useApp(selector)` and call the actions below.

import { create } from "zustand";
import { AcpConnection, type Inbound, type Json, type JsonObject, obj, RpcError, str } from "../acp/jsonrpc";
import {
  agentActive,
  nextPlanRefresh,
  type PlanUsage,
  parseAgentList,
  parsePlans,
  parsePlanUsage,
  planHarness,
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
import { dismissOverlays } from "../lib/overlays";
import { setShellTransport, shellCall } from "../lib/shell";
import { BUILTIN_THEMES, parseThemes, ThemeMethod, type ThemesSnapshot } from "../lib/themes";
import { titleFrom } from "../lib/title";
import { createBatcher, frameQueue } from "./batch";
import {
  type ComposeExtras,
  emptyExtras,
  hasExtras,
  promptBlocks,
  sentAttachments,
  splitImageMarkers,
} from "./compose";
import { restoreChoice, selectOption } from "./harness";
import {
  answerOrchestration,
  linkFinished,
  linkStarted,
  loadBook,
  queuedLinkDropped,
  stopLinksFrom,
} from "./host";
import { CAPABILITY_VERSION, forget, type GrantAsk, type LinkBook } from "./orchestration";
import { autoAnswer, type PermissionMode, parsePermissionMode } from "./permissions";
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
  type QueuedPrompt,
  requeueSteers,
  type ThreadState,
  truncateBefore,
  userIndexOf,
  wakeTurn,
} from "./thread";

export const APP_VERSION = "0.1.0";

export type SettingsSection =
  | "general"
  | "harnesses"
  | "connections"
  | "capabilities"
  | "appearance"
  | "usage"
  | "engine";

export type ToastTone = "info" | "success" | "error";

export interface Toast {
  id: string;
  text: string;
  tone: ToastTone;
  /** One button at the right (Restart, Undo, Open…). */
  action?: { label: string; run: () => void };
  /** Stays until dismissed; errors do by default. */
  persist: boolean;
  /** 0–1 draws a progress bar along the foot. */
  progress?: number;
  /** Fading out; removed once the exit animation has run. */
  leaving?: boolean;
  /** When it was shown or last repeated: a change restarts its timer. */
  at: number;
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
  threads: Record<string, ThreadState>;
  /** Null shows the new-thread screen. */
  selectedId: string | null;
  /** The folder a new thread starts in. */
  /** The folder the next new thread starts in. */
  projectCwd: string;
  /** Where new threads start by default (Settings › General, ⌘⇧O). */
  defaultCwd: string;
  newThread: NewThreadChoice | null;
  harnesses: HarnessSnapshot | null;
  commands: SlashCommand[];
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
  toasts: Toast[];
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
  /** A sent message being edited in the composer: sending rewinds the thread to it first. */
  editing: Record<string, { itemId: string; userIndex: number; original: string }>;
  /** Smart UI blocks run in the transcript (Settings › General › Chat). */
  smartUi: boolean;
  /** The engine learned a different Smart UI setting at initialize: a restart applies the new one. */
  smartUiPending: boolean;
  /** The workspace terminal under the chat (⌘J). */
  terminalOpen: boolean;
  /** The panel on the right of the chat. */
  panel: "details" | "changes" | "pulls" | "peek" | null;
  /** The thread shown beside this one in the panel's Thread tab. */
  peekId: string | null;
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
  /** Threads can start and message other threads (Settings › General); the engine learns it at initialize. */
  orchestration: boolean;
  /** Threads that were running when the app quit or the engine restarted continue on their own. */
  resumeAfterRestart: boolean;
  /** The interface in SF Pro Rounded (the default) or the standard system font. */
  roundedFont: boolean;
  /** A streaming reply's new words fade in as the model writes them. */
  animateStreaming: boolean;
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
}

/** A cached plan older than this is dropped on launch: its windows have moved on. */
export const PLAN_CACHE_MAX_AGE_MS = 24 * 60 * 60_000;

const STORAGE = {
  smartUi: "rustcode.smartUi",
  pinned: "rustcode.pinned",
  archived: "rustcode.archived",
  project: "rustcode.projectCwd",
  theme: "rustcode.theme",
  themeActive: "rustcode.themeActive",
  pinsMigrated: "rustcode.pinsMigrated",
  archiveMigrated: "rustcode.archiveMigrated",
  sidebar: "rustcode.sidebarHidden",
  orchestration: "rustcode.orchestration",
  resumeAfterRestart: "rustcode.resumeAfterRestart",
  /** The ids of the threads running a turn, kept while the engine is up. */
  runningThreads: "rustcode.runningThreads",
  roundedFont: "rustcode.roundedFont",
  animateStreaming: "rustcode.animateStreaming",
  statusMarks: "rustcode.statusMarks",
  snoozed: "rustcode.snoozed",
  permissionMode: "rustcode.permissionMode",
  permissionModes: "rustcode.permissionModes",
  /** Each harness's last new-thread choice, for an engine that does not remember it. */
  harnessChoices: "rustcode.harnessChoices",
  /** Each harness's plan windows, so the composer's meter is there on launch. */
  planCache: "rustcode.planCache",
};

function stored(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
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
  editing: {},
  confirm: null,
  // On unless turned off: an agent asks before it starts or messages a thread (Ask), so
  // delegating is there to use without first finding the setting.
  orchestration: stored(STORAGE.orchestration) !== "0",
  resumeAfterRestart: stored(STORAGE.resumeAfterRestart) !== "0",
  roundedFont: stored(STORAGE.roundedFont) === "1",
  animateStreaming: stored(STORAGE.animateStreaming) !== "0",
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
  smartUi: stored(STORAGE.smartUi) !== "0",
  smartUiPending: false,
  terminalOpen: false,
  panel: null,
  peekId: null,
  agents: {},
  planUsage: {},
  planCache: prunePlans(storedJson<Record<string, CachedPlan>>(STORAGE.planCache, {})),
  status: "connecting",
  engineLog: [],
  sessions: [],
  sessionsLoaded: false,
  threads: {},
  selectedId: null,
  projectCwd: stored(STORAGE.project) ?? "",
  defaultCwd: stored(STORAGE.project) ?? "",
  newThread: null,
  harnesses: null,
  commands: [],
  drafts: {},
  extras: {},
  settings: null,
  switcherOpen: false,
  modelPickerAsk: 0,
  report: null,
  onboardingOpen: false,
  sidebarHidden: stored(STORAGE.sidebar) === "1",
  toasts: [],
  theme: (stored(STORAGE.theme) as AppState["theme"]) ?? "system",
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

/** Exit animation of a toast (docs/design.md, Motion). */
const TOAST_EXIT_MS = 120;

/**
 * Shows a toast (ui/Toaster.tsx). Info and success leave after 4 s, errors stay
 * until dismissed. Passing an `id` replaces the toast with that id in place
 * (the engine's toast moves from "Starting…" to an error this way); the same
 * text and tone again restarts the shown one instead of stacking a copy.
 * Returns the toast's id.
 */
export function toast(
  text: string,
  tone: ToastTone = "info",
  options: { id?: string; action?: Toast["action"]; persist?: boolean; progress?: number } = {},
): string {
  const at = Date.now();
  let id = options.id ?? localId("toast");
  set((s) => {
    const same = s.toasts.find(
      (t) => !t.leaving && (t.id === options.id || (!options.id && t.text === text && t.tone === tone)),
    );
    if (same) id = same.id;
    const next: Toast = {
      id,
      text,
      tone,
      action: options.action,
      // Errors leave on their own too (after longer): a burst of them must not pile up for good.
      persist: options.persist ?? false,
      progress: options.progress,
      at,
    };
    return {
      toasts: same
        ? s.toasts.map((t) => (t === same ? next : t))
        : // The Toaster shows the newest; up to four older wait behind it, the rest are dropped.
          // A fading toast with the same id goes now, so the new one keeps its key.
          [...s.toasts.filter((t) => !(t.leaving && t.id === id)).slice(-4), next],
    };
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

/** Fades a toast out, then drops it. */
/** Every toast at once: the "+N" on the toast in front. */
export function clearToasts() {
  set((s) => ({ toasts: s.toasts.filter((t) => t.persist && t.progress !== undefined) }));
}

export function dismissToast(id: string) {
  if (!get().toasts.some((t) => t.id === id && !t.leaving)) return;
  set((s) => ({ toasts: s.toasts.map((t) => (t.id === id ? { ...t, leaving: true } : t)) }));
  setTimeout(
    () => set((s) => ({ toasts: s.toasts.filter((t) => !(t.id === id && t.leaving)) })),
    TOAST_EXIT_MS,
  );
}

export function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
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
/** Engine stderr, kept for Settings › Engine; appended once a frame, not per line. */
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
    await Promise.all([refreshSessions(), refreshHarnesses(), probeFeatures(conn), refreshThemes(conn)]);
    // The composer's meter is ready before the first thread opens.
    void refreshPlanCache();
    if (connection === conn) await syncPinsAndSettled(conn);
    const selected = get().selectedId;
    if (selected) void openThread(selected);
    if (connection === conn) void resumeInterrupted(interrupted);
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
  const changed = name !== get().themeActive;
  adoptTheme(name);
  const picked = (get().themes?.themes ?? BUILTIN_THEMES).find((t) => t.name === name);
  if (changed && picked) toast(`Theme: ${picked.name}`, "success");
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
      else if (message.method === ThemeMethod.changed) onThemesChanged(message.params);
      return;
    case "request":
      if (message.method === Method.requestPermission) {
        const request = parsePermissionRequest(message.id, message.params);
        if (!request || !get().threads[request.sessionId]) {
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
    set({ commands: update.commands });
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
    set({ sessionsLoaded: true });
    return;
  }
  try {
    const all: SessionSummary[] = [];
    let cursor: string | undefined;
    for (let page = 0; page < 10; page++) {
      const params: JsonObject = cursor ? { cursor } : {};
      const result = parseSessionList(await conn.request(Method.sessionList, params, 15_000));
      all.push(...result.sessions);
      cursor = result.nextCursor;
      if (!cursor) break;
    }
    set((s) => {
      // Threads started here that the engine does not list yet (it lists a session once it saved history).
      const listed = new Set(all.map((x) => x.id));
      const started = s.sessions.filter((x) => !listed.has(x.id) && s.threads[x.id]);
      const sessions = byActivity([...started, ...all]);
      return {
        sessions,
        sessionsLoaded: true,
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

/** Mark a thread unread again: its row says Done until you open it. */
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
  clearFinished(id);
  dismissOverlays();
  set({ selectedId: id, switcherOpen: false, settings: null, view: { kind: "chat" } });
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
  model?: string | null;
  effort?: string | null;
  /** Create a worktree first and run the thread in it (docs/orchestrator.md). */
  worktree?: { repo: string; branch?: string; title?: string };
  title?: string;
  select?: boolean;
}

/** `session/new` with the harness, model, effort and worktree in `_meta.rustcode`; the thread is added to the list. */
export async function createSession(options: NewSession): Promise<string> {
  const conn = engine();
  const s = get();
  const params: JsonObject = { cwd: options.cwd, mcpServers: [] };
  const meta: JsonObject = {};
  if (options.harness && !s.harnesses?.unsupported) {
    meta.harness = options.harness;
    if (options.model) meta.model = options.model;
    if (options.effort) meta.effort = options.effort;
  }
  if (options.worktree) {
    const w: JsonObject = { repo: options.worktree.repo };
    if (options.worktree.branch) w.branch = options.worktree.branch;
    if (options.worktree.title) w.title = options.worktree.title;
    meta.worktree = w;
  }
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
  set((st) => ({
    threads: { ...st.threads, [id]: thread },
    ...(options.select ? { selectedId: id, newThread: null } : {}),
    sessions: [
      {
        id,
        cwd,
        updatedAt: Date.now(),
        harness: thread.harness?.id ?? options.harness,
        worktree,
        title: options.title,
      },
      ...st.sessions,
    ],
  }));
  // Saved, not only shown: otherwise the thread's first message replaces it at the next list.
  if (options.title) await renameThread(id, options.title);
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

/** Creates a thread with the new-thread choice (or the default harness) and worktree toggle, and selects it. */
async function createThread(firstMessage = ""): Promise<string> {
  const s = get();
  const cwd = s.projectCwd;
  if (!cwd) throw new Error("Choose a project folder for this thread first.");
  const choice = newThreadChoice(s);
  const wt = s.newWorktree;
  const worktree =
    wt.on && s.features.worktrees
      ? { repo: cwd, branch: wt.branch.trim() || undefined, title: titleFrom(firstMessage, 80) }
      : undefined;
  const id = await createSession({
    cwd,
    harness: choice?.harness,
    model: choice?.model,
    effort: choice?.effort,
    worktree,
    select: true,
  });
  // The access level chosen on the new-thread composer belongs to the thread it started.
  const pendingMode = get().newThreadPermissionMode;
  if (pendingMode) {
    setPermissionMode(id, pendingMode);
    set({ newThreadPermissionMode: null });
  }
  set({ newWorktree: { on: wt.on, branch: "", edited: false } });
  if (worktree) {
    const branch = get().threads[id]?.worktree?.branch ?? worktree.branch;
    toast(branch ? `Working in a new worktree on ${branch}` : "Working in a new worktree", "success");
  }
  return id;
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

export function togglePin(id: string) {
  const before = get().pinned;
  const on = !before.includes(id);
  const pinned = on ? [id, ...before] : before.filter((x) => x !== id);
  set({ pinned });
  store(STORAGE.pinned, JSON.stringify(pinned));
  toast(on ? "Pinned" : "Unpinned", "success");
  if (!get().features.pins || !connection) return;
  connection.request(V2Method.sessionPin, { session_id: id, pinned: on }, 10_000).catch((error) => {
    set({ pinned: before });
    store(STORAGE.pinned, JSON.stringify(before));
    toast(`Could not ${on ? "pin" : "unpin"} the thread: ${errorText(error)}`, "error");
  });
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
  toast(on ? "Archived. It's under Archived in the sidebar." : "Unarchived", "success");
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
  set((s) => ({
    threads,
    archived,
    pinned,
    sessions: s.sessions.filter((x) => x.id !== id),
    links: forget(s.links, id),
  }));
  store(STORAGE.archived, JSON.stringify(archived));
  store(STORAGE.pinned, JSON.stringify(pinned));
  toast("Moved to the Trash", "success");
  if (get().selectedId === id) startNewThread();
  return true;
}

// ---------------------------------------------------------------- turns

export function setDraft(key: string, text: string) {
  set((s) => ({ drafts: { ...s.drafts, [key]: text } }));
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
  const command = get().commands.find((c) => c.name === text);
  if (!command || command.action === "execute") return false;
  const thread = from ? get().threads[from] : undefined;
  switch (command.action) {
    case "new_session": {
      // A fresh thread on the same project, harness, model and effort, selected and empty.
      const model = thread ? selectOption(thread.configOptions, "model")?.current : undefined;
      const effort = thread ? selectOption(thread.configOptions, "effort")?.current : undefined;
      const harness = thread?.harness?.id ?? get().sessions.find((x) => x.id === from)?.harness;
      startNewThread(thread?.cwd);
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
    if (!id) id = await createThread(trimmed);
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
  void runPrompt(id, trimmed, extras);
  return true;
}

/**
 * `/fork` copies the chat into a new thread and leaves this one open (the
 * engine's rule). The reply does not name the new thread, so the list is
 * compared before and after, and a toast offers to open it.
 */
async function forkThread(id: string): Promise<void> {
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
 * send. A steer carries text only, so a message with attachments (or a
 * command token, or links) queues whole for the next turn, and says so.
 */
export async function steerOrSubmit(text: string, extras: ComposeExtras = emptyExtras): Promise<boolean> {
  const id = get().selectedId;
  const thread = id ? get().threads[id] : undefined;
  const trimmed = text.trim();
  // ⌘⏎ always tries: a harness learns it can steer when its first thread starts, so a thread
  // started before that still reads "none" though the engine can steer it now.
  if (!id || !thread?.running || !get().agent?.steer) return submit(text, extras);
  // Images ride along when the harness takes them mid-turn; other extras wait for the next turn.
  const imagesOnly =
    extras.attachments.length > 0 &&
    extras.attachments.every((a) => a.kind === "image") &&
    !extras.command &&
    !extras.links.length &&
    acceptsImages(thread) &&
    !(threadHarness(thread)?.builtin ?? true);
  if (imagesOnly && (await steer(id, trimmed, extras))) return true;
  if (hasExtras(extras)) {
    const sent = await submit(text, extras);
    if (sent)
      toast(
        extras.attachments.length
          ? "Queued: attachments go with the next turn"
          : "Queued: the turn takes plain text only",
      );
    return sent;
  }
  if (!trimmed) {
    const next = thread.queue[0];
    if (next) await sendQueuedNow(id, next.id);
    return true;
  }
  if (await steer(id, trimmed)) return true;
  updateThread(id, (t) => ({ ...t, queue: [...t.queue, { id: localId("queued"), text: trimmed }] }));
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

async function steer(id: string, text: string, extras?: ComposeExtras): Promise<boolean> {
  const thread = get().threads[id];
  const prompt = extras?.attachments.length
    ? promptBlocks(text, extras, { images: true, builtin: false, cwd: thread?.cwd ?? "" })
    : undefined;
  try {
    const result = (await engine().request(
      Method.steer,
      prompt ? { sessionId: id, text, prompt } : { sessionId: id, text },
      10_000,
    )) as JsonObject | null;
    if (result?.accepted !== true) return false;
    updateThread(id, (t) => appendSteer(t, text, Date.now(), extras && sentAttachments(extras)));
    toast("Steering: read after the current step");
    return true;
  } catch {
    return false;
  }
}

export async function sendQueuedNow(id: string, queuedId: string) {
  const item = get().threads[id]?.queue.find((q) => q.id === queuedId);
  if (!item) return;
  const thread = get().threads[id];
  if (item.extras?.attachments.length && (threadHarness(thread)?.builtin ?? true)) {
    toast("Attachments go with the next turn: RustCode takes text only mid-turn");
    return;
  }
  if (await steer(id, item.text, item.extras)) {
    removeQueued(id, queuedId);
  } else {
    toast("The turn can't take it now. It stays queued");
  }
}

export function removeQueued(id: string, queuedId: string) {
  const link = get().threads[id]?.queue.find((q) => q.id === queuedId)?.link;
  updateThread(id, (t) => ({ ...t, queue: t.queue.filter((q) => q.id !== queuedId) }));
  if (link) queuedLinkDropped(link);
}

/**
 * Take a queued message back out of the queue, whole, for the composer (↑ or
 * Edit in composer). A message delivered for another thread keeps its link and
 * stays queued, since sending it again would lose the way back.
 */
export function takeQueued(id: string, queuedId: string): QueuedPrompt | undefined {
  const item = get().threads[id]?.queue.find((q) => q.id === queuedId);
  if (!item || item.link) return undefined;
  removeQueued(id, queuedId);
  return item;
}

/** A queued message edited in place; an empty text removes it. */
export function updateQueued(id: string, queuedId: string, text: string) {
  const trimmed = text.trim();
  updateThread(id, (t) => ({
    ...t,
    // Its attachments stay; one with attachments survives an emptied text.
    queue: trimmed
      ? t.queue.map((q) => (q.id === queuedId ? { ...q, text: trimmed } : q))
      : t.queue
          .filter((q) => q.id !== queuedId || q.extras?.attachments.length)
          .map((q) => (q.id === queuedId ? { ...q, text: "" } : q)),
  }));
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
  options: { fromView?: boolean; link?: string } = {},
): Promise<void> {
  const conn = engine();
  const thread = get().threads[id];
  const startIndex = thread?.items.length ?? 0;
  if (options.link) linkStarted(options.link);
  if (id in get().snoozed) snoozeThread(id, null);
  const harness = threadHarness(thread);
  const prompt = promptBlocks(text, extras, {
    images: acceptsImages(thread),
    builtin: harness?.builtin ?? true,
    cwd: thread?.cwd ?? "",
  });
  const shown = [extras.command?.name, ...extras.links, text].filter(Boolean).join(" ");
  updateThread(id, (t) => beginTurn(t, shown, Date.now(), sentAttachments(extras), options.fromView));
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
  // A report command that answered in the panel leaves no turn behind: its message and footer go.
  const reported = get().report?.threadId === id;
  if (reported && outcome === "completed" && isReportCommand(text, extras)) {
    updateThread(id, (t) => ({ ...t, items: t.items.slice(0, startIndex) }));
  }
  if (options.link)
    linkFinished(options.link, id, outcome, failure, get().threads[id]?.items.slice(startIndex) ?? []);
  void refreshSessions();
  void refreshPlanUsage(id);
  void refreshAgents(id);
  void refreshCheckpoints(id);
  if ((outcome === "completed" || outcome === "failed") && id !== get().selectedId) {
    const name = get().sessions.find((s) => s.id === id)?.title || "A thread";
    toast(
      outcome === "failed" ? `“${name}” failed` : `“${name}” finished`,
      outcome === "failed" ? "error" : "success",
      { action: { label: "Open", run: () => selectThread(id) } },
    );
  }
  // A queued message starts at once, so "working" stays up across the hand-off.
  const next = get().threads[id]?.queue[0];
  if (outcome === "completed" && next) {
    removeQueued(id, next.id);
    await runPrompt(id, next.text, next.extras, { fromView: next.fromView, link: next.link });
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

/** Answer exactly this request; it leaves the queue before the reply, so a double click cannot answer the next. */
function reply(request: PermissionRequest, result: JsonObject) {
  updateThread(request.sessionId, (t) => ({
    ...t,
    permissions: t.permissions.filter((p) => p.rpcId !== request.rpcId),
  }));
  connection?.respond(request.rpcId, result);
}

export function answerPermission(request: PermissionRequest, option: PermissionOption | null) {
  reply(request, option ? permissionSelected(option.id) : permissionCancelled);
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
  reply(request, options.length === 1 ? permissionSelected(options[0]!.id) : questionOptionsAnswer(options));
}

export function answerQuestionText(request: PermissionRequest, text: string) {
  const carrier = request.options.find(
    (o) => o.kind === "allow_once" || o.kind === "allow_always" || !o.kind,
  );
  if (!carrier || !text.trim()) return;
  reply(request, questionTextAnswer(text.trim(), carrier));
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
  const snapshot = await harnessCall(Method.harnessSet, { id, enabled });
  const name = snapshot?.harnesses.find((h) => h.id === id)?.name ?? id;
  if (snapshot) toast(`${name} ${enabled ? "is on" : "is off"}`, "success");
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
  set({ newThread: restoreChoice(snapshot, harness, appChoices(snapshot)) });
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
    set((s) => ({ sessions: s.sessions.map((x) => (x.id === id ? { ...x, harness } : x)) }));
    const opened = get().threads[id];
    if (opened && lacksModels(opened)) void ensureHarnessModels(harness);
    void refreshPlanUsage(id);
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

export function setAnimateStreaming(on: boolean) {
  set({ animateStreaming: on });
  store(STORAGE.animateStreaming, on ? "1" : "0");
}

export function setOrchestration(on: boolean) {
  set((s) => ({
    orchestration: on,
    orchestrationPending: s.status === "ready" ? !s.orchestrationPending : s.orchestrationPending,
  }));
  store(STORAGE.orchestration, on ? "1" : "0");
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
export async function setThreadStatus(id: string, status: "review" | "done" | "idle"): Promise<void> {
  set((s) => ({ sessions: s.sessions.map((x) => (x.id === id ? { ...x, status } : x)) }));
  if (status === "idle") clearStatusMark(id);
  else saveMarks({ ...get().statusMarks, [id]: status });
  if (!get().features.status || !connection) return;
  try {
    await connection.request(V2Method.sessionStatus, { session_id: id, status }, 10_000);
    // The engine keeps it now; the app's mark only covers the gap until the list says so.
  } catch (error) {
    if (!(error instanceof RpcError && error.isMethodNotFound)) {
      toast(`Could not save the status: ${errorText(error)}`, "error");
    }
  }
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
    toast(`Removed the worktree of ${wt.branch}`, "success");
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
    toast(
      rewind && !kept
        ? "Restored the files and the conversation"
        : "Restored the files; the conversation stays",
      "success",
    );
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

/** Opens `panel`, or closes it when it is the one open (the header's buttons). */
export function setPanel(panel: AppState["panel"]) {
  if (panel) lastPanel = panel;
  set((s) => ({ panel: s.panel === panel ? null : panel }));
}

/** Shows `panel` (the panel's tabs): never closes. */
/** Shows a thread in the side panel, next to the one you are in. */
export function peekThread(id: string) {
  set({ peekId: id });
  showPanel("peek");
  void openThread(id);
}

export function showPanel(panel: NonNullable<AppState["panel"]>) {
  lastPanel = panel;
  set({ panel });
}

/** ⌘⌥B: the right panel open or closed, with what it showed last. */
export function toggleRightPanel() {
  set((s) => ({ panel: s.panel ? null : lastPanel }));
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
    const harness = planHarness(p.id);
    out[harness] = {
      source: harness,
      windows: p.windows.map((w) => ({
        usedPercent: w.usedPercent,
        minutes: w.minutes,
        resetsAt: w.resetsAt,
      })),
      fetchedAt,
      name: p.name,
      plan: p.plan,
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
      commitPlans({ [harness]: { ...usage, fetchedAt, source: harness } });
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

let plansInFlight = false;
/** Claude's answer could not be read: the background poll stops asking for it. */
let claudePaused = false;

/** The consent Settings › Usage keeps before Claude's sign-in is read from the Keychain. */
export const CLAUDE_USAGE_KEY = "rustcode.claudeUsage";

function claudeUsageAllowed(): boolean {
  return stored(CLAUDE_USAGE_KEY) === "1";
}

/**
 * Turns that consent on or off. Off forgets the numbers it read: the composer
 * must not keep showing a plan the user asked the app to stop reading.
 */
export function setClaudeUsageAllowed(on: boolean): void {
  store(CLAUDE_USAGE_KEY, on ? "1" : "0");
  claudePaused = false;
  if (!on) setPlanCache(forgetPlans(get().planCache, ["claude-code"]));
}

/**
 * Folds a plans answer into the cache: a signed-in harness's windows go in, one
 * that is signed out or gone loses its entry, and a Claude answer that could
 * not be read stops the background poll asking again (a Deny in the Keychain
 * must not prompt every three minutes). `asked` is Settings' own call, which
 * is what unpauses it.
 */
export function commitPlansAnswer(answers: UsagePlan[], asked = false): void {
  if (asked) claudePaused = false;
  const gone = answers
    .filter((p) => p.status === "signedOut" || p.status === "notInstalled")
    .map((p) => planHarness(p.id));
  const kept = forgetPlans(get().planCache, gone);
  const learned = plansToCache(answers);
  setPlanCache(mergePlans(kept, learned));
  if (
    answers.some(
      (p) => planHarness(p.id) === "claude-code" && (p.status === "signedOut" || p.status === "unavailable"),
    )
  ) {
    claudePaused = true;
  }
}

/**
 * Asks for every subscription's plan (`_rustcode/usage/plans`, no thread
 * needed) and folds the answer into the cache. One request at a time; a failed
 * one keeps the last numbers for the next poll. Claude is asked for only with
 * consent and while the last answer could be read.
 */
export async function refreshPlanCache(): Promise<void> {
  if (!connection || plansInFlight || get().status !== "ready") return;
  plansInFlight = true;
  try {
    const result = await connection.request(
      Method.usagePlans,
      { claude: claudeUsageAllowed() && !claudePaused },
      20_000,
    );
    commitPlansAnswer(parsePlans(result));
  } catch {
    // Cached numbers stay; the next poll tries again.
  } finally {
    plansInFlight = false;
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
  claudePaused = false;
  if (plansOnVisible) document.removeEventListener("visibilitychange", plansOnVisible);
  plansOnVisible = null;
  plansInFlight = false;
}

// ---------------------------------------------------------------- UI

export function openSettings(section: SettingsSection | null) {
  dismissOverlays();
  set({ settings: section, switcherOpen: false });
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
  clearTimeout(slowTimer);
  stopPlanPolling();
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
    threads: {},
    selectedId: null,
    projectCwd: "",
    newThread: null,
    harnesses: null,
    commands: [],
    drafts: {},
    extras: {},
    settings: null,
    switcherOpen: false,
    onboardingOpen: false,
    toasts: [],
    agent: undefined,
    pinned: [],
    archived: {},
    editing: {},
    confirm: null,
    panel: null,
    agents: {},
    planUsage: {},
    planCache: {},
    orchestration: false,
    orchestrationPending: false,
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
  });
}

/** A command the engine marks as a report (`/status`, `/context`…), sent alone. */
function isReportCommand(text: string, extras: ComposeExtras): boolean {
  const name = extras.command?.name ?? text.trim().split(/\s/)[0];
  return (
    !extras.links.length &&
    !extras.attachments.length &&
    get().commands.some((c) => c.name === name && c.report)
  );
}

export function closeReport() {
  set({ report: null });
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
    void runPrompt(id, RESUME_TEXT);
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
