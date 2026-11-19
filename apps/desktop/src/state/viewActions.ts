// View, panel, worktree and window actions on the app store; read it only when called to keep module loading safe.

import { type Json, type JsonObject, RpcError } from "../acp/jsonrpc";
import { harnessPlanKey } from "../acp/rustcode";
import { displayPath, type HarnessSnapshot, type SlashCommand } from "../acp/types";
import { parseWorktreeList, removeRefusal, V2Method, type Worktree } from "../acp/v2";
import { dismissOverlays } from "../lib/overlays";
import { readStorage, writeStorage } from "../lib/storage";
import { MAX_PANELS, PANELS, type SettingsSection, STORAGE, settingsSection, store } from "./appState";
import { menuCommands, withSkills } from "./commands";
import { emptyExtras } from "./compose";
import { newThreadAccountOn } from "./logins";
import {
  type AppState,
  connection,
  dismissToast,
  engine,
  errorText,
  harnessesOn,
  runPrompt,
  toast,
  updateThread,
  useApp,
} from "./store";
import { isRunning, localId } from "./thread";
import { askConfirm } from "./threadActions";
import { splitThreadKey } from "./threadKeys";
import { openThread } from "./threadOpen";
import { sessionById } from "./threadStatus";

const get = () => useApp.getState();
const set = (patch: Partial<AppState> | ((state: AppState) => Partial<AppState>)) => useApp.setState(patch);

// ---------------------------------------------------------------- views and worktrees

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
  if (isRunning(get().threads[id])) {
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

// ---------------------------------------------------------------- views

export function setNewWorktree(change: Partial<AppState["newWorktree"]>) {
  set((s) => ({ newWorktree: { ...s.newWorktree, ...change } }));
}

export function setSmartUi(on: boolean) {
  set((s) => ({
    smartUi: on,
    smartUiPending: s.engine.phase === "ready" ? !s.smartUiPending : s.smartUiPending,
  }));
  store(STORAGE.smartUi, on ? "1" : "0");
}

/** A message a Smart UI view sends (`rustcode.send`), as the user's next message, tagged "from view". */
export function sendFromView(id: string, text: string) {
  const thread = get().threads[id];
  if (!thread || !connection) return;
  if (isRunning(thread)) {
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
 * Pull requests and the Thread tab are a tab away. It is only what ⌘⇧B opens on
 * a thread whose own panel was never opened; nothing opens it on its own.
 */
export const PANEL_SHEET_AT = 980;

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
 * was closed by hand. A new thread selection defaults to details when it fits. A view this project or
 * thread cannot show right now (Pull requests in a project without them, the
 * Thread tab with nothing peeked) opens nothing rather than something else.
 */
function panelForThread(s: AppState, id: string | null): AppState["panel"] {
  if (!id) return null;
  const kept = s.panelByThread[id];
  if (kept === undefined && typeof window !== "undefined" && window.innerWidth > PANEL_SHEET_AT)
    return DEFAULT_PANEL;
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
 * showing, with a wide-window default for a thread never seen.
 */
export function viewingThread(
  s: AppState,
  id: string | null,
): Pick<AppState, "selectedId" | "panel" | "peekId"> {
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

/** ⌘⇧B: the right panel open or closed, with this thread's own view, else the overview. */
export function toggleRightPanel() {
  const s = get();
  const next = s.panel ? null : (panelForThread(s, s.selectedId) ?? DEFAULT_PANEL);
  set({ panel: next, panelByThread: keptPanels(s.selectedId, next) });
}

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

const CODEX_COMPACT: SlashCommand = {
  name: "/compact",
  description: "Compact thread history to free context space",
  takesArguments: false,
  report: false,
  action: "execute",
};

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
  const harness = harnessOf(s, threadId ?? undefined);
  const host = threadId ? splitThreadKey(threadId).host : s.newThreadHost;
  const known = harnessesOn(s, host)?.harnesses.find((h) => h.id === harness);
  const commands = own
    ? withSkills(own, s.baseCommands)
    : menuCommands({
        base: s.baseCommands,
        seen: harness ? s.harnessCommands[harness] : undefined,
        // Before the harnesses are listed the engine answers the thread, so its catalog applies.
        builtin: known?.builtin ?? (!s.harnesses || harness === undefined || harness === "rustcode"),
      });
  // The Codex adapter handles /compact directly without advertising it through ACP.
  return (known?.kind === "codex" || harness === "codex") &&
    !commands.some((command) => command.name === CODEX_COMPACT.name)
    ? [...commands, CODEX_COMPACT]
    : commands;
}

/**
 * The commands a thread's harness advertised itself: its session's own list, or before the session
 * has advertised, the harness's last list. `promptBlocks` sends a catalog skill one of them names as
 * the harness's own `/name` command.
 */
export function advertisedCommands(
  s: Pick<
    CommandsSource,
    "threads" | "sessions" | "newThread" | "harnesses" | "harnessCommands" | "sessionCommands"
  >,
  threadId: string,
): SlashCommand[] | undefined {
  const own = s.sessionCommands[threadId];
  if (own) return own;
  const harness = harnessOf(s, threadId);
  return harness ? s.harnessCommands[harness] : undefined;
}

// ---------------------------------------------------------------- UI

/**
 * Opens Settings on a page (null closes it). An id from an older build still
 * lands somewhere sensible, so a caller that names Harnesses or Engine works.
 */
export function openSettings(
  section: SettingsSection | string | null = readStorage("rustcode.settingsSection") ?? "general",
) {
  dismissOverlays();
  const next = settingsSection(section);
  if (next) writeStorage("rustcode.settingsSection", next);
  set({ settings: next, settingsMachine: null, switcherOpen: false });
}

/** Opens Settings on a page and scrolls to one of its rows, named by its search-index title. */
export function openSettingAt(section: SettingsSection, title: string) {
  openSettings(section);
  set({ settingsLanding: { section, title } });
}

/** The landing row was lit (or was not there): Settings forgets it. */
export function settingLanded() {
  set({ settingsLanding: null });
}

/** Open one machine's own Settings page, reached from its card and the Machines list. */
export function openMachineSettings(id: string) {
  dismissOverlays();
  set({ settings: "machines", settingsMachine: id, switcherOpen: false });
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
