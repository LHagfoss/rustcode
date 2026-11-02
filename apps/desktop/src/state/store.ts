// App state (Zustand) and the engine controller that drives it over ACP.
// Components read slices with `useApp(selector)` and call the actions below.

import { create } from "zustand";
import { AcpConnection, type Inbound, type Json, type JsonObject, RpcError, str } from "../acp/jsonrpc";
import type { Transport } from "../acp/transport";
import {
  type AgentInfo,
  byActivity,
  fallbackHarnessSnapshot,
  type HarnessSnapshot,
  initializeParams,
  isReportUpdate,
  Method,
  type PermissionOption,
  type PermissionRequest,
  parseAgentInfo,
  parseConfigOptions,
  parseHarnessModels,
  parseHarnessSnapshot,
  parsePermissionRequest,
  parseSessionHarness,
  parseSessionList,
  parseSessionUpdate,
  permissionCancelled,
  permissionSelected,
  questionOptionsAnswer,
  questionTextAnswer,
  type SessionSummary,
  type SlashCommand,
} from "../acp/types";
import { setShellTransport, shellCall } from "../lib/shell";
import {
  type ComposeExtras,
  emptyExtras,
  hasExtras,
  promptBlocks,
  sentAttachments,
  splitImageMarkers,
} from "./compose";
import {
  appendSteer,
  applyUpdate,
  beginTurn,
  emptyThread,
  finishLoad,
  finishTurn,
  localId,
  messagesAfter,
  requeueSteers,
  type ThreadState,
  truncateBefore,
  userIndexOf,
} from "./thread";

export const APP_VERSION = "0.1.0";

export type SettingsSection =
  | "general"
  | "harnesses"
  | "connections"
  | "skills"
  | "mcp"
  | "appearance"
  | "engine";

export interface Toast {
  id: string;
  text: string;
  tone: "info" | "success" | "warning";
}

export interface NewThreadChoice {
  harness: string;
  model: string | null;
  effort: string | null;
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
  projectCwd: string;
  newThread: NewThreadChoice | null;
  harnesses: HarnessSnapshot | null;
  commands: SlashCommand[];
  drafts: Record<string, string>;
  /** Command token, link chips and attachments of each composer, by thread id (or "new"). */
  extras: Record<string, ComposeExtras>;
  settings: SettingsSection | null;
  switcherOpen: boolean;
  onboardingOpen: boolean;
  sidebarHidden: boolean;
  toasts: Toast[];
  theme: "system" | "light" | "dark";
  /** Pinned threads, kept in app storage like the SwiftUI app's `pinnedSessions`. */
  pinned: string[];
  /** Archived threads and when, kept in app storage; they leave the list until shown. */
  archived: Record<string, number>;
  /** A sent message being edited in the composer: sending rewinds the thread to it first. */
  editing: Record<string, { itemId: string; userIndex: number; original: string }>;
  /** An app-drawn confirmation (the webview has no native dialogs). */
  confirm: { title: string; message: string; action: string; destructive?: boolean; run: () => void } | null;
}

const STORAGE = {
  pinned: "rustcode.pinned",
  archived: "rustcode.archived",
  project: "rustcode.projectCwd",
  theme: "rustcode.theme",
  sidebar: "rustcode.sidebarHidden",
};

function stored(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function store(key: string, value: string) {
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

export const useApp = create<AppState>(() => ({
  pinned: storedJson<string[]>(STORAGE.pinned, []),
  archived: storedJson<Record<string, number>>(STORAGE.archived, {}),
  editing: {},
  confirm: null,
  status: "connecting",
  engineLog: [],
  sessions: [],
  sessionsLoaded: false,
  threads: {},
  selectedId: null,
  projectCwd: stored(STORAGE.project) ?? "",
  newThread: null,
  harnesses: null,
  commands: [],
  drafts: {},
  extras: {},
  settings: null,
  switcherOpen: false,
  onboardingOpen: false,
  sidebarHidden: stored(STORAGE.sidebar) === "1",
  toasts: [],
  theme: (stored(STORAGE.theme) as AppState["theme"]) ?? "system",
}));

const set = useApp.setState;
const get = useApp.getState;

function updateThread(id: string, fn: (thread: ThreadState) => ThreadState) {
  set((state) => {
    const thread = state.threads[id];
    if (!thread) return state;
    const next = fn(thread);
    return next === thread ? state : { threads: { ...state.threads, [id]: next } };
  });
}

export function toast(text: string, tone: Toast["tone"] = "info") {
  const id = localId("toast");
  set((s) => ({ toasts: [...s.toasts.slice(-3), { id, text, tone }] }));
  setTimeout(() => set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) })), 3200);
}

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

// ---------------------------------------------------------------- connection

let connection: AcpConnection | null = null;
let transport: Transport | null = null;
/** Sessions whose `session/load` replay is arriving. */
const loading = new Set<string>();

export function engine(): AcpConnection {
  if (!connection) throw new Error("The engine is not connected.");
  return connection;
}

/** Start the transport once; every engine (re)start then gets a fresh ACP connection. */
export async function connect(next: Transport): Promise<void> {
  transport = next;
  setShellTransport(next);
  set({ status: "connecting", transport: next.name, statusDetail: undefined });
  try {
    await next.start((event) => {
      if (event.type === "line") connection?.receive(event.line);
      else if (event.type === "log") set((s) => ({ engineLog: [...s.engineLog.slice(-400), event.line] }));
      else if (event.type === "closed") connection?.close(event.reason);
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
  try {
    const result = await conn.request(Method.initialize, initializeParams(APP_VERSION), 20_000);
    const agent = parseAgentInfo(result);
    set({ agent, status: "ready", statusDetail: undefined });
    // Threads that were open lost their engine-side state with the old process.
    set((s) => ({
      threads: Object.fromEntries(
        Object.entries(s.threads).map(([id, t]) => [
          id,
          { ...t, loaded: false, running: false, permissions: [] },
        ]),
      ),
    }));
    await Promise.all([refreshSessions(), refreshHarnesses()]);
    const selected = get().selectedId;
    if (selected) void openThread(selected);
  } catch (error) {
    set({ status: "closed", statusDetail: errorText(error) });
  }
}

export async function restartEngine(): Promise<void> {
  if (!transport) return;
  set({ status: "connecting", statusDetail: "Restarting the engine…" });
  await transport.restart().catch((error) => set({ status: "closed", statusDetail: errorText(error) }));
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
      return;
    case "request":
      if (message.method === Method.requestPermission) {
        const request = parsePermissionRequest(message.id, message.params);
        if (!request || !get().threads[request.sessionId]) {
          connection?.respond(message.id, permissionCancelled);
          return;
        }
        updateThread(request.sessionId, (t) => ({ ...t, permissions: [...t.permissions, request] }));
        if (request.sessionId !== get().selectedId) {
          toast(request.question ? "A thread has a question" : "A thread needs approval", "warning");
        }
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
    toast(update.text.trim().split("\n")[0] ?? "Done");
    return;
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
    toast(`Could not list threads: ${errorText(error)}`, "warning");
  }
}

export function selectThread(id: string | null) {
  set({ selectedId: id, switcherOpen: false, settings: null });
  if (id) void openThread(id);
}

export function startNewThread(cwd?: string) {
  const s = get();
  const projectCwd = cwd ?? (s.selectedId ? s.threads[s.selectedId]?.cwd : undefined) ?? s.projectCwd;
  set({ selectedId: null, projectCwd, settings: null, switcherOpen: false, newThread: null });
  if (projectCwd) store(STORAGE.project, projectCwd);
}

export function setProjectCwd(cwd: string) {
  set({ projectCwd: cwd });
  store(STORAGE.project, cwd);
}

async function openThread(id: string) {
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
  } catch (error) {
    updateThread(id, (t) => ({ ...finishLoad(t), error: `Could not open this thread: ${errorText(error)}` }));
  } finally {
    loading.delete(id);
  }
}

export async function closeThread(id: string) {
  try {
    await connection?.request(Method.sessionClose, { sessionId: id }, 10_000);
  } catch {
    // Closing is best effort.
  }
}

/** Creates a thread with the new-thread choice (or the default harness), and selects it. */
async function createThread(): Promise<string> {
  const conn = engine();
  const s = get();
  const cwd = s.projectCwd;
  if (!cwd) throw new Error("Choose a project folder for this thread first.");
  const choice = s.newThread;
  const params: JsonObject = { cwd, mcpServers: [] };
  if (choice && !s.harnesses?.unsupported) {
    const meta: JsonObject = { harness: choice.harness };
    if (choice.model) meta.model = choice.model;
    if (choice.effort) meta.effort = choice.effort;
    params._meta = { rustcode: meta };
  }
  const result = (await conn.request(Method.sessionNew, params, 60_000)) as JsonObject;
  const id = str(result.sessionId);
  if (!id) throw new Error("The engine did not return a session id.");
  const thread: ThreadState = {
    ...emptyThread(id, cwd),
    loaded: true,
    configOptions: parseConfigOptions(result.configOptions),
    harness: parseSessionHarness(result),
  };
  set((st) => ({
    threads: { ...st.threads, [id]: thread },
    selectedId: id,
    newThread: null,
    sessions: [{ id, cwd, updatedAt: Date.now(), harness: thread.harness?.id }, ...st.sessions],
  }));
  // A model or effort the user picked before the thread existed applies now (built-in threads take it this way).
  if (choice && s.harnesses?.unsupported) {
    if (choice.model) await setConfigOption(id, "model", choice.model);
    if (choice.effort) await setConfigOption(id, "effort", choice.effort);
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
    toast(`Could not rewind: ${errorText(error)}`, "warning");
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
  const pinned = get().pinned.includes(id) ? get().pinned.filter((x) => x !== id) : [id, ...get().pinned];
  set({ pinned });
  store(STORAGE.pinned, JSON.stringify(pinned));
  toast(pinned.includes(id) ? "Pinned" : "Unpinned", "success");
}

export function setArchived(id: string, on: boolean) {
  const archived = { ...get().archived };
  if (on) {
    if (get().threads[id]?.running) {
      toast("Stop the running turn before archiving this thread.", "warning");
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
    toast(`Could not rename the thread: ${errorText(error)}`, "warning");
    return false;
  }
}

/** Move a thread's folder to the Trash, closing it in the engine first, as the SwiftUI app does. */
export async function trashThread(id: string): Promise<boolean> {
  if (get().threads[id]?.running) {
    toast("Stop the running turn before moving this thread to the Trash.", "warning");
    return false;
  }
  if (get().threads[id] && get().agent?.closeSession) await closeThread(id);
  try {
    await shellCall("thread_trash", { id });
  } catch (error) {
    toast(errorText(error), "warning");
    return false;
  }
  const { [id]: _gone, ...threads } = get().threads;
  const archived = { ...get().archived };
  delete archived[id];
  const pinned = get().pinned.filter((x) => x !== id);
  set((s) => ({ threads, archived, pinned, sessions: s.sessions.filter((x) => x.id !== id) }));
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
export async function submit(text: string, extras: ComposeExtras = emptyExtras): Promise<boolean> {
  const trimmed = text.trim();
  if (!trimmed && !hasExtras(extras)) return false;
  let id = get().selectedId;
  try {
    if (!id) id = await createThread();
  } catch (error) {
    toast(`Could not start the thread: ${errorText(error)}`, "warning");
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
  void runPrompt(id, trimmed, extras);
  return true;
}

/** ⌘⏎: hand the running turn a message to read after its current step, else send. */
export async function steerOrSubmit(text: string): Promise<boolean> {
  const id = get().selectedId;
  const thread = id ? get().threads[id] : undefined;
  const trimmed = text.trim();
  if (!id || !thread?.running || !canSteer(thread)) return submit(text);
  if (!trimmed) {
    const next = thread.queue[0];
    if (next) await sendQueuedNow(id, next.id);
    return true;
  }
  if (await steer(id, trimmed)) return true;
  updateThread(id, (t) => ({ ...t, queue: [...t.queue, { id: localId("queued"), text: trimmed }] }));
  toast("Queued: the turn is waiting for you");
  return true;
}

export function canSteer(thread: ThreadState): boolean {
  if (!get().agent?.steer) return false;
  return !thread.harness || thread.harness.capabilities.steer === "live";
}

async function steer(id: string, text: string): Promise<boolean> {
  try {
    const result = (await engine().request(
      Method.steer,
      { sessionId: id, text },
      10_000,
    )) as JsonObject | null;
    if (result?.accepted !== true) return false;
    updateThread(id, (t) => appendSteer(t, text));
    toast("Steering: read after the current step");
    return true;
  } catch {
    return false;
  }
}

export async function sendQueuedNow(id: string, queuedId: string) {
  const item = get().threads[id]?.queue.find((q) => q.id === queuedId);
  if (!item) return;
  if (item.extras?.attachments.length) {
    toast("A message with attachments is sent when the turn ends");
    return;
  }
  if (await steer(id, item.text)) {
    removeQueued(id, queuedId);
  } else {
    toast("The turn can't take it now. It stays queued");
  }
}

export function removeQueued(id: string, queuedId: string) {
  updateThread(id, (t) => ({ ...t, queue: t.queue.filter((q) => q.id !== queuedId) }));
}

async function runPrompt(
  id: string,
  text: string,
  extras: ComposeExtras = emptyExtras,
  options: { fromView?: boolean } = {},
): Promise<void> {
  const conn = engine();
  const thread = get().threads[id];
  const harness = threadHarness(thread);
  const prompt = promptBlocks(text, extras, {
    images: acceptsImages(thread),
    builtin: harness?.builtin ?? true,
    cwd: thread?.cwd ?? "",
  });
  const shown = [extras.command?.name, ...extras.links, text].filter(Boolean).join(" ");
  updateThread(id, (t) => beginTurn(t, shown, Date.now(), sentAttachments(extras), options.fromView));
  set((s) => ({
    sessions: byActivity(
      s.sessions.map((x) =>
        x.id === id
          ? { ...x, updatedAt: Date.now(), title: x.title ?? (shown.split("\n")[0]?.slice(0, 80) || "Image") }
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
  void refreshSessions();
  if (outcome === "completed" && id !== get().selectedId) toast("A thread finished", "success");
  // A queued message starts at once, so "working" stays up across the hand-off.
  const next = get().threads[id]?.queue[0];
  if (outcome === "completed" && next) {
    removeQueued(id, next.id);
    await runPrompt(id, next.text, next.extras);
  }
}

export async function cancel(id = get().selectedId): Promise<void> {
  if (!id || !connection) return;
  const thread = get().threads[id];
  if (!thread?.running) return;
  try {
    connection.notify(Method.sessionCancel, { sessionId: id });
    // ACP: after `session/cancel` the client answers outstanding permission requests as cancelled.
    for (const request of thread.permissions) connection.respond(request.rpcId, permissionCancelled);
    updateThread(id, (t) => ({ ...t, permissions: [] }));
  } catch (error) {
    toast(errorText(error), "warning");
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
  } catch (error) {
    toast(`Could not change ${configId}: ${errorText(error)}`, "warning");
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
      toast(`Could not read harnesses: ${errorText(error)}`, "warning");
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
    toast(errorText(error), "warning");
    return null;
  }
}

export const setHarnessEnabled = (id: string, enabled: boolean) =>
  harnessCall(Method.harnessSet, { id, enabled });
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

export function setNewThreadChoice(choice: NewThreadChoice | null) {
  set({ newThread: choice });
}

// ---------------------------------------------------------------- UI

export function openSettings(section: SettingsSection | null) {
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
  });
}
