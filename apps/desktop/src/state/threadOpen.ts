import { isLoaded, isLoading } from "./thread";
// Selecting, opening and creating threads: the thread on screen, a new thread's folder, machine and
// harness, opening a thread's history and repairing one that will not open, creating its session,
// and the setup a new worktree runs. Split out of state/store.ts; the state these write still lives
// in the app store.

import { type AcpConnection, type Json, type JsonObject, RpcError, str } from "../acp/jsonrpc";
import {
  fallbackHarnessSnapshot,
  type HarnessSnapshot,
  Method,
  parseConfigOptions,
  parseHarnessSnapshot,
  parseSessionHarness,
  projectName,
  type SessionSummary,
  type ThreadRelationship,
} from "../acp/types";
import { parseWorktree, type Worktree } from "../acp/v2";
import { afterPaint } from "../lib/frame";
import { dismissOverlays } from "../lib/overlays";
import { titleFrom } from "../lib/title";
import { setPermissionMode } from "./approvals";
import { scheduleAutoCompact } from "./autoCompactRun";
import { refreshCheckpoints } from "./checkpointActions";
import { type ComposeExtras, carriesExtras, emptyExtras, hasExtras } from "./compose";
import {
  concreteModels,
  listedModel,
  normalizeDefault,
  preferredDefault,
  restoreChoice,
  selectOption,
  unavailableReason,
} from "./harness";
import { appChoices, effortMemory, ensureHarnessModels } from "./harnessActions";
import { newThreadAccountOn } from "./logins";
import { loadTranscript, threadFromTranscript } from "./machineCache";
import { loadMirroredTranscript } from "./machineMirror";
import { CLAUDE_CODE, markClaudePlansStarted, refreshPlanCache, refreshPlanUsage } from "./planUsage";
import { projectKey as actionsProjectKey, projectActions, worktreeHooks } from "./projectActions";
import { projectChoice } from "./projectBook";
import { matchProject, projectAt, projectsOn } from "./projects";
import { beginSetup, dismissSetup, type PendingSetup, setupFailed, setupSucceeded } from "./setup";
import {
  type AppState,
  beginCreatingSession,
  connection,
  connectionFor,
  connectionForThread,
  endCreatingSession,
  engine,
  errorText,
  harnessesOn,
  harnessOf,
  keepUnlisted,
  keptUnlistedThreads,
  type NewThreadChoice,
  onSessionActivity,
  refreshSessions,
  releaseAnnouncements,
  runPrompt,
  STORAGE,
  setConfigOption,
  setDraft,
  setExtras,
  store,
  threadIsRemote,
  toast,
  updateThread,
  useApp,
  viewingThread,
} from "./store";
import { emptyThread, finishLoad, isRunning, localId, type ThreadState } from "./thread";
import { renameThread } from "./threadActions";
import { LOCAL_HOST, splitThreadKey, threadKey } from "./threadKeys";
import { clearFinished } from "./threadNotices";
import { sessionById } from "./threadStatus";

// Read through `useApp` at call time: store.ts imports this module, so `useApp` is not defined yet
// while this one loads.
const get = () => useApp.getState();
const set = (patch: Partial<AppState> | ((s: AppState) => Partial<AppState>)) => useApp.setState(patch);

/** Moving away keeps the start and its message in the sidebar without owning the new composer. */
function setupSelection(s: AppState, id: string | null = null) {
  const backgroundSetups = { ...s.backgroundSetups };
  if (s.pendingSetup) backgroundSetups[s.pendingSetup.id] = s.pendingSetup;
  const pendingSetup = id ? (backgroundSetups[id] ?? null) : null;
  if (pendingSetup) delete backgroundSetups[pendingSetup.id];
  return { pendingSetup, backgroundSetups };
}

/** Finish only this start; another pending thread keeps its message and selection. */
function finishedSetup(s: AppState, id: string | undefined) {
  const backgroundSetups = { ...s.backgroundSetups };
  if (id) delete backgroundSetups[id];
  return { pendingSetup: setupSucceeded(s.pendingSetup, id), backgroundSetups };
}

/** Sessions whose `session/load` replay is arriving. */
export const loading = new Set<string>();
/** The shared `session/load` for each thread; callers must not prompt before its replay finishes. */
export const opening = new Map<string, Promise<string | null>>();

/** The shortest gap between two window-focus list refreshes, see below. */
const FOCUS_LIST_GAP_MS = 5_000;
let lastFocusListAt = 0;

/** Forgets the last list read on focus, for the store's reset between tests. */
export function resetFocusList(): void {
  lastFocusListAt = 0;
}

/**
 * A window focus re-lists the threads, at most once per 5 s: a change made outside this window
 * (`rustcode threads settle` in a terminal, another app) reaches the list with no turn to carry it,
 * and flapping between windows should not read 500 threads each time. Off until connected, and
 * `now` is a parameter so it can be felt with a clock in tests.
 */
export async function refreshSessionsOnFocus(now = Date.now()): Promise<void> {
  if (!connection || get().engine.phase !== "ready") return;
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
  const selection = setupSelection(get(), id);
  if (selection.pendingSetup) id = null;
  clearFinished(id);
  dismissOverlays();
  set((s) => ({
    ...viewingThread(s, id),
    ...selection,
    switcherOpen: false,
    settings: null,
    view: { kind: "chat" },
  }));
  if (id && harnessOf(get(), id) === CLAUDE_CODE) markClaudePlansStarted();
  // The account this thread runs on may never have been read: its meter has no
  // numbers until it is, and it appears without a visit to Settings › Usage.
  // The list's summary names the account before the thread itself is opened.
  void refreshPlanCache();
  if (id) void openThread(id);
}

/**
 * The folder a new thread starts in when none was chosen: the home folder of the machine it runs
 * on. `session/new` is sent no folder for it, which every engine resolves to its own home
 * (`session_new_cwd`), so it works on a machine whose home this app cannot know.
 */
export const HOME_FOLDER = "~";

/** Start beside the last thread or project visited, keeping its machine. */
export function startNewThreadFromContext() {
  const s = get();
  const selected = s.selectedId
    ? (s.threads[s.selectedId] ?? s.sessions.find((thread) => thread.id === s.selectedId))
    : undefined;
  startNewThread(
    selected?.cwd || s.projectCwd || undefined,
    selected ? splitThreadKey(selected.id).host : s.newThreadHost,
  );
}

export function startNewThread(cwd?: string, host?: string) {
  const s = get();
  // An explicit machine wins, and a folder picked on this disk still starts here.
  // A new thread started from a machine's group in the sidebar names it (docs/desktop.md › Several
  // machines).
  const target = host ?? LOCAL_HOST;
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
  // Nothing chosen and nothing to go on: the machine's home folder, so a new thread can always start.
  const projectCwd = cwd ?? (preferred || remembered || HOME_FOLDER);
  dismissOverlays();
  set({
    selectedId: null,
    ...setupSelection(s),
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

/**
 * The draft "Create with agent" starts a thread with (Settings › Capabilities, the command
 * palette). It names the built-in `create-skill` skill and both ways to save one, so a harness
 * that does not load RustCode's built-in skills still knows where a skill goes; it ends open for
 * the reader to say what the skill is for.
 */
export const CREATE_SKILL_PROMPT =
  'Create a new RustCode skill. Follow the create-skill skill if you have it. Save it with the RustCode MCP tool manage_capabilities (kind "skill", action "create"), or write ~/.config/rustcode/skills/<name>/SKILL.md. Ask me anything you need first.\n\nThe skill should: ';

/** A new thread (on `host`'s machine when named) whose draft asks an agent to write a skill. */
export function createSkillWithAgent(host?: string) {
  startNewThread(undefined, host || undefined);
  setDraft("new", CREATE_SKILL_PROMPT);
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
  if (isRunning(thread)) {
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
    if (!existing || (!isLoaded(existing) && !existing.items.length)) {
      const summary = get().sessions.find((x) => x.id === id);
      // The last copy this computer read from the machine, if it kept one; it is replaced by the
      // machine's own replay when the machine is back (`watchMachineConnection`).
      const cached = loadTranscript(id);
      const base = cached
        ? threadFromTranscript(cached)
        : { ...emptyThread(id, summary?.cwd ?? "", summary?.title), load: { phase: "idle" as const } };
      set((s) => ({
        threads: {
          ...s.threads,
          [id]: { ...base, cwd: summary?.cwd || base.cwd, title: summary?.title ?? base.title },
        },
      }));
      // Past what the web view's storage keeps (a transcript older than 30 days, a wiped web
      // view), this computer's disk still has the last read (`state/machineMirror.ts`).
      if (!cached) {
        void loadMirroredTranscript(id).then((mirrored) => {
          if (!mirrored || connectionForThread(id)) return;
          const now = get().threads[id];
          if (!now || isLoaded(now) || now.items.length) return;
          const copy = threadFromTranscript(mirrored);
          set((s) => ({
            threads: {
              ...s.threads,
              [id]: { ...copy, cwd: summary?.cwd || copy.cwd, title: summary?.title ?? copy.title },
            },
          }));
        });
      }
    }
    return Promise.resolve(null);
  }
  // A copy whose open failed is no copy: opening it again tries again, so a thread that could not
  // be opened once (the engine was restarting, its harness was down) is not refused for good.
  if (!conn || isLoaded(existing) || isLoading(existing) || loading.has(id)) return Promise.resolve(null);
  const summary = get().sessions.find((x) => x.id === id);
  const cwd = existing?.cwd ?? summary?.cwd ?? get().projectCwd;
  let pending: Promise<string | null>;
  pending = Promise.resolve().then(async () => {
    try {
      let result: Json;
      try {
        result = await conn.request(Method.sessionLoad, { sessionId, cwd, mcpServers: [] }, 60_000);
      } catch (first) {
        // A record naming another folder is put right once, quietly; only a repair that finds
        // problems, or a second failure, reaches the banner.
        const fixedCwd = await repairBeforeRetry(id, conn, first);
        if (fixedCwd === null) throw first;
        result = await conn.request(Method.sessionLoad, { sessionId, cwd: fixedCwd, mcpServers: [] }, 60_000);
      }
      updateThread(id, (t) => ({
        ...finishLoad(t),
        configOptions: parseConfigOptions((result as JsonObject | null)?.configOptions),
        harness: parseSessionHarness(result) ?? t.harness,
      }));
      const opened = get().threads[id];
      if (id === get().selectedId && opened?.harness?.id === CLAUDE_CODE) {
        markClaudePlansStarted();
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
      updateThread(id, (t) => ({ ...finishLoad(t), load: { phase: "failed", error: message } }));
      return message;
    } finally {
      loading.delete(id);
      if (opening.get(id) === pending) opening.delete(id);
      // What the thread does now, sent with the load: applied once its replay is in, or dropped.
      const activity = activityDuringLoad.get(id);
      activityDuringLoad.delete(id);
      if (activity !== undefined && isLoaded(get().threads[id])) onSessionActivity(activity);
    }
  });
  opening.set(id, pending);
  loading.add(id);
  set((s) => ({
    threads: { ...s.threads, [id]: { ...emptyThread(id, cwd, summary?.title), load: { phase: "loading" } } },
  }));
  return pending;
}

/** The last `_rustcode/session/activity` of each thread whose load replay is arriving. */
const activityDuringLoad = new Map<string, unknown>();

/**
 * `_rustcode/session/activity`, held while its thread loads: the engine says what the thread does
 * now ahead of the load's answer, and applied before it, the load's own end (`finishLoad`) would
 * close the turn the agent woke for and show the thread as finished.
 */
export function receiveActivity(params: unknown): void {
  const id = str((params as JsonObject | null)?.sessionId);
  if (id && isLoading(get().threads[id])) activityDuringLoad.set(id, params);
  else onSessionActivity(params);
}

/** Threads whose open was repaired automatically in this app session: once each, never a loop. */
export const autoRepaired = new Set<string>();

/** An open the engine refuses because the thread's record names another folder (Repair Thread fixes it). */
export function isRepairableOpenError(error: unknown): boolean {
  return /session\/load cwd does not match the recorded workspace/i.test(errorText(error));
}

/**
 * A failed open that Repair Thread can fix is repaired here, once per thread: answers the folder to
 * open again in, or null when it is not repairable, was tried already, or the repair found problems.
 */
async function repairBeforeRetry(id: string, conn: AcpConnection, error: unknown): Promise<string | null> {
  if (!isRepairableOpenError(error) || autoRepaired.has(id) || !get().agent?.sessionRepair) return null;
  autoRepaired.add(id);
  try {
    const answer = ((await conn.request(
      Method.sessionRepair,
      { sessionId: splitThreadKey(id).id },
      10_000,
    )) ?? {}) as JsonObject;
    const problems = Array.isArray(answer.problems) ? answer.problems : [];
    const cwd = typeof answer.cwd === "string" ? answer.cwd : null;
    if (problems.length || !cwd) return null;
    set((s) => ({ sessions: s.sessions.map((x) => (x.id === id ? { ...x, cwd } : x)) }));
    return cwd;
  } catch {
    return null;
  }
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
  if (isRunning(get().threads[id])) {
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
  // The engine announces the thread to this very client before it answers: those announcements
  // wait until the answer says which thread is this one's (`deferAnnouncement`).
  beginCreatingSession();
  let own: string | undefined;
  try {
    return await makeSession(options, (id) => {
      own = id;
    });
  } finally {
    endCreatingSession();
    releaseAnnouncements(own);
  }
}

async function makeSession(options: NewSession, made: (id: string) => void): Promise<string> {
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
    // The project's setup and settle actions (state/projectActions.ts); the engine runs them in the
    // worktree, the repository's own `.rustcode/actions.json` winning a slot both fill.
    // They are this computer's settings, keyed by a local folder: a worktree on another machine
    // runs only that repository's own actions.
    const hooks = host ? undefined : worktreeHooks(projectActions(actionsProjectKey(options.worktree.repo)));
    if (hooks) w.hooks = hooks as unknown as JsonObject;
    meta.worktree = w;
  }
  // The engine keeps this in the thread's own metadata, so pruning the link
  // book does not make the child a top-level thread (`state/topLevel.ts`).
  if (options.parent) {
    // The machine's own session id, not a key that names it here (`state/threadKeys.ts`).
    meta.parent = splitThreadKey(options.parent).id;
    if (options.relationship && options.relationship !== "delegate") meta.relationship = options.relationship;
  }
  // The title this thread is made with, so a thread on another machine is named
  // there too: `thread_rename` is this computer's shell, and it never reaches a
  // machine's own history (issue 3524).
  if (options.title) meta.title = options.title;
  if (Object.keys(meta).length) params._meta = { rustcode: meta };
  const result = (await conn.request(Method.sessionNew, params, 120_000)) as JsonObject;
  const raw = str(result.sessionId);
  if (!raw) throw new Error("The server did not return a thread id.");
  const id = threadKey(host, raw);
  made(id);
  const worktree: Worktree | undefined = parseWorktree(
    ((result._meta as JsonObject | undefined)?.rustcode as JsonObject | undefined)?.worktree,
  );
  const repo = str(((result._meta as JsonObject | undefined)?.rustcode as JsonObject | undefined)?.repo);
  const cwd = worktree?.path ?? options.cwd;
  const thread: ThreadState = {
    ...emptyThread(id, cwd, options.title),
    load: { phase: "loaded" as const },
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
    model: selectOption(thread.configOptions, "model")?.current ?? options.model ?? undefined,
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
  const pending = !!options.pendingId;
  // No list has this thread yet: it is kept here, so a snapshot from before it cannot take it
  // away mid-turn (`mergeMachineLists`, `refreshSessions`, issue 3523).
  keepUnlisted(summary);
  set((st) => ({
    threads: { ...st.threads, [id]: thread },
    ...(pending
      ? {}
      : {
          // Selecting the new thread keeps the picker's choice as the session's draft, so the
          // new-thread screen comes back on it (state/store.ts `startNewThread`).
          ...(options.select ? viewingThread(st, id) : {}),
          // The engine announces the thread to every client (`threads/created`) while this
          // request is still out: the row it added is replaced by this one, never doubled.
          sessions: [summary, ...st.sessions.filter((x) => x.id !== id)],
          ...finishedSetup(st, options.pendingId),
        }),
  }));
  // Saved, not only shown: otherwise the thread's first message replaces it at the next list.
  // A pending setup lets its rename run beside it: the message starts the moment the thread is listed.
  // A thread on another computer is titled by the `session/new` it was made with: only this
  // computer's folder is renamed from here (`thread_rename` is this computer's shell).
  if (options.title && !host) {
    const saved = renameThread(id, options.title);
    if (!pending) await saved;
  }
  // A model or effort picked before the thread existed applies now, when the thread did not take it
  // from `session/new` (an older engine, or a harness that ignores it there).
  const current = (configId: string) => thread.configOptions.find((o) => o.id === configId)?.current;
  const has = (configId: string) => thread.configOptions.some((o) => o.id === configId);
  try {
    if (options.model && current("model") !== options.model && has("model")) {
      await setConfigOption(id, "model", options.model, { remember: false });
    }
    if (options.effort && current("effort") !== options.effort && has("effort")) {
      await setConfigOption(id, "effort", options.effort, { remember: false });
    }
  } catch (error) {
    // The creation failed: the row it kept and listed must not linger for the keep's five minutes.
    keptUnlistedThreads.delete(id);
    set((st) => {
      const { [id]: _gone, ...threads } = st.threads;
      return { threads, sessions: st.sessions.filter((x) => x.id !== id) };
    });
    throw error;
  }
  // The composer needs this login's windows during the first turn as well as after it ends.
  if (options.select) void refreshPlanUsage(id);
  if (pending) {
    // Listed now, with the setup gone in the same update: the row the message was sent to becomes
    // the thread, in place, and the chat goes on from the message it already shows.
    set((st) => ({
      sessions: [
        {
          ...summary,
          model: selectOption(st.threads[id]?.configOptions ?? [], "model")?.current ?? summary.model,
        },
        ...st.sessions.filter((x) => x.id !== id),
      ],
      ...(options.select && st.pendingSetup?.id === options.pendingId && !st.selectedId
        ? viewingThread(st, id)
        : {}),
      ...finishedSetup(st, options.pendingId),
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
      normalizeDefault(
        snapshot,
        restoreChoice(snapshot, snapshot.default.harness, appChoices(snapshot), effortMemory()),
      )
    );
  }
  // A kept draft can name a harness that is gone, or one that cannot run now (turned off, signed
  // out in Settings): then the screen falls back to the harness a new thread would start on, never
  // to this draft. One that can run is normalized as the picker normalizes it.
  const harness = snapshot.harnesses.find((h) => h.id === s.newThread?.harness);
  if (!harness || unavailableReason(harness)) return normalizeDefault(snapshot, preferredDefault(snapshot));
  const choice = normalizeDefault(snapshot, s.newThread);
  // A machine's `_rustcode/harnesses` lists only the models a harness has already learned there
  // (the picker asks for the rest, `ensureHarnessModels`): with none listed yet, keep no model, so
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
  const offered =
    !!model &&
    (!found.models.length || !!listedModel(found.id, concreteModels(found.id, found.models), model));
  return normalizeDefault(s.harnesses, { ...restored, model: offered ? model : restored.model });
}

/**
 * Creates a thread with the new-thread choice (or the default harness) and worktree toggle, and
 * selects it. `session/new` starts a harness there (and a worktree, when the toggle is on), which
 * takes a moment: the setup, with the message and its attachments, is shown before it is asked for,
 * so ⏎ on the new-thread screen is answered at once. A second send while it runs starts nothing; a
 * setup that failed is sent again with `retryPendingSetup`.
 */
export async function createThread(
  firstMessage = "",
  extras: ComposeExtras = emptyExtras,
): Promise<string | null> {
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
  set({
    pendingSetup: setup,
    newThreadPermissionMode: null,
    newWorktree: { on: s.newWorktree.on, branch: "", edited: false },
  });
  // That update gets its own painted frame before `session/new` is asked for. An in-process harness
  // answers in a microtask, so both updates would otherwise land in one task and one frame: the
  // pending screen's message would be drawn for the first time by the thread's own transcript, and
  // its entry (`rc-send-in`) would never get a frame to play in.
  await afterPaint();
  return await startSetup(setup, s);
}

/** The worktree the new-thread composer asked for, or none. */
function newWorktreeFor(
  s: Pick<AppState, "newWorktree" | "features">,
  cwd: string,
  firstMessage: string,
): PendingSetup["worktree"] {
  const wt = s.newWorktree;
  // A worktree is cut on the machine the thread runs on, by that machine's own `session/new`
  // (`makeSession`), so the toggle applies to a thread on another computer too.
  if (!wt.on || !s.features.worktrees || cwd === HOME_FOLDER) return undefined;
  return { repo: cwd, branch: wt.branch.trim() || undefined, title: titleFrom(firstMessage, 80) };
}

/**
 * `session/new` for a setup, with the choices the new-thread screen holds. Its title is the one the
 * row already shows, so nothing blinks back to "New thread" when the row becomes real. On failure
 * the setup keeps the message, its attachments and the reason, for a retry.
 */
async function startSetup(setup: PendingSetup, s = get()): Promise<string | null> {
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
      cwd: setup.cwd === HOME_FOLDER ? "" : setup.cwd,
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
    set((st) => {
      if (st.pendingSetup?.id === setup.id) return { pendingSetup: setupFailed(st.pendingSetup, text) };
      const background = st.backgroundSetups[setup.id];
      return background
        ? { backgroundSetups: { ...st.backgroundSetups, [setup.id]: setupFailed(background, text) } }
        : {};
    });
    return null;
  }
  // The access level chosen on the new-thread composer belongs to the thread it started.
  const pendingMode = s.newThreadPermissionMode;
  if (pendingMode) {
    setPermissionMode(id, pendingMode);
  }
  return id;
}

/**
 * Retry: a failed setup sends its message again, with the attachments and the place it held, and the
 * prompt goes out the moment the session exists. Its own row stays where it is while it runs.
 */
export async function retryPendingSetup(): Promise<boolean> {
  const s = get();
  const failed = s.pendingSetup;
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
  set({ pendingSetup: setup, newThreadPermissionMode: null });
  const id = await startSetup(setup, s);
  if (!id) return false;
  // Its bubble is already on screen, from the setup it keeps through the retry: it does not fly in.
  void runPrompt(id, setup.text, setup.extras ?? emptyExtras, { alreadyShown: true });
  return true;
}

/**
 * Gives up a failed setup: its row and its error leave, and its message goes back to the field it
 * was typed in, with the pictures and files that went with it, so nothing that was written is lost.
 */
export function dismissPendingSetup(id?: string) {
  const s = get();
  if (id && s.pendingSetup?.id !== id) {
    set(finishedSetup(s, id));
    return;
  }
  const pending = s.pendingSetup;
  set({ pendingSetup: dismissSetup() });
  if (!pending?.failure) return;
  if (pending.text.trim()) setDraft("new", pending.text);
  const extras = pending.extras;
  if (extras) setExtras("new", () => ({ ...extras, attachments: [...extras.attachments] }));
}
