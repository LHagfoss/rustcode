// Thread notices: marking a thread finished, unread or failed, the toast and system notification
// that announce it, and snoozing a thread until a time or its next write. Split out of
// state/store.ts; the state these write still lives in the app store.

import { RpcError } from "../acp/jsonrpc";
import { linksOf, stateOf, subtreeQuiet, subtreeQuietBelow } from "./host";
import { dismissRows } from "./inboxDismissed";
import { finishIsNews, type NotifyEvent, notifyThread, wakeEvent } from "./notify";
import { nextQueued } from "./queue";
import { dueSnoozes } from "./snooze";
import {
  type AppState,
  connectionForThread,
  errorText,
  limitTime,
  machineOfThread,
  STORAGE,
  selectThread,
  startNewThread,
  store,
  threadIsRemote,
  toast,
  useApp,
} from "./store";
import { splitThreadKey } from "./threadKeys";
import { sessionById } from "./threadStatus";
import { waitingOn as backgroundWork } from "./waiting";

// Read through `useApp` at call time: store.ts imports this module, so `useApp` is not defined yet
// while this one loads.
const get = () => useApp.getState();
const set = (patch: Partial<AppState> | ((s: AppState) => Partial<AppState>)) => useApp.setState(patch);

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
  // A limit or a lost network already said what stopped it, in the composer's own row.
  if (!failed || (!get().limitWaits[id] && !get().internetWaits[id])) {
    // A failure always is. A finish is only when the thread is done for good (`endedForGood`): not
    // a delegate (the wake message reaches its parent and the strip above that parent's composer
    // already says it), not a turn that left background work or a queued message to go on.
    if (failed || endedForGood(id)) {
      notifyThread(
        { threadId: id, kind: failed ? "failed" : "finished", key: String(now), title: notifyTitle(id) },
        get(),
      );
    }
  }
}

/** A thread another one runs, where the user has not written themselves. */
export function isDelegated(id: string): boolean {
  return linksOf(get().links, id).drivenBy !== undefined && get().messaged[id] === undefined;
}

/**
 * Whether a thread still waits on something it started: commands its last turn parked on, or
 * delegates it handed work to (state/waiting.ts `waitingOn`). It picks up by itself, so its turn
 * ending is not its end.
 */
function stillWaiting(id: string): boolean {
  const s = get();
  const thread = s.threads[id];
  return !!backgroundWork({
    id,
    running: thread?.running ?? false,
    commands: thread?.waiting,
    links: s.links.links,
    state: (link) => stateOf(s, link),
    quiet: subtreeQuiet,
    quietBelow: subtreeQuietBelow,
  });
}

/** Whether a turn that just ended leaves the thread truly finished: the only finish worth announcing. */
function endedForGood(id: string): boolean {
  const thread = get().threads[id];
  return finishIsNews({
    waiting: stillWaiting(id),
    delegated: isDelegated(id),
    queued: !!thread && !!nextQueued(thread.queue, thread),
  });
}

/**
 * The in-app toast for a thread whose turn ended off screen. A failure, or a wait that stopped it,
 * is always news; a finish only when the thread is done for good (`endedForGood`), and never for a
 * thread that answers a link (`link`). A thread the user snoozed says nothing: its finish waits for
 * the snooze to end, which then says it once (`wakeSnoozed`).
 */
export function finishToast(
  id: string,
  outcome: "completed" | "failed",
  stop: { waiting?: { reason?: string; until: number } | null; offline?: unknown; link?: unknown } = {},
): void {
  if (id === get().selectedId || id in get().snoozed) return;
  const { waiting, offline, link } = stop;
  const stopped = !!waiting || !!offline;
  if (!stopped && outcome === "completed" && (link !== undefined || !endedForGood(id))) return;
  const name = get().sessions.find((s) => s.id === id)?.title || "A thread";
  toast(
    waiting
      ? waiting.reason === "overload"
        ? `“${name}” was overloaded and tries again at ${limitTime(waiting.until)}`
        : `“${name}” hit its limit and continues at ${limitTime(waiting.until)}`
      : offline
        ? `“${name}” is waiting for the internet`
        : outcome === "failed"
          ? `“${name}” failed`
          : `“${name}” finished`,
    stopped ? "info" : outcome === "failed" ? "error" : "success",
    { action: { label: "Open", run: () => selectThread(id) } },
  );
}

/** A turn the thread woke itself for ended: the toast and the notification, once, when it is done for good. */
export function announceFinished(id: string): void {
  if (!endedForGood(id)) return;
  finishToast(id, "completed");
  notifyThread({ threadId: id, kind: "finished", key: String(Date.now()), title: notifyTitle(id) }, get());
}

/** A thread's title, for what the system shows of it. */
export function threadName(id: string): string {
  const s = get();
  return sessionById(s, id)?.title || s.threads[id]?.title || "A thread";
}

/**
 * A notification's title: the thread's, and the computer it runs on when that
 * is not this one, so two machines' "Run the tests" are told apart.
 */
export function notifyTitle(id: string): string {
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

export function applySnooze(id: string, at: number | null) {
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
export function wakeOnWrite(id: string) {
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

export function without(map: Record<string, number>, id: string): Record<string, number> {
  if (!(id in map)) return map;
  const { [id]: _, ...rest } = map;
  return rest;
}

/** You saw it: opening the thread (or coming back to the window on it) settles the flag. */
export function clearFinished(id: string | null) {
  if (!id) return;
  seeFailure(id);
  if (!(id in get().finished || id in get().failed)) return;
  set((s) => ({ finished: without(s.finished, id), failed: without(s.failed, id) }));
}

/**
 * The thread's failure was seen: opened, or its banner dismissed. Its "Needs you" row is put away
 * quietly, from both of its sources — the store's `failed` map and a replayed transcript that ends
 * in a failed turn — and a later failure, which arrives after this moment, is a row again
 * (state/inboxDismissed.ts `isDismissed`). A thread with no failure records nothing.
 */
export function seeFailure(id: string): void {
  const s = get();
  const items = s.threads[id]?.items;
  const tail = items?.[items.length - 1];
  const failedTail = tail?.kind === "turnEnd" && tail.outcome === "failed";
  if (!(id in s.failed) && !failedTail) return;
  const key = `${id}:failed`;
  const at = Math.max(s.failed[id] ?? 0, failedTail ? (tail.at ?? 0) : 0);
  if ((s.inboxDismissed[key] ?? -1) >= at) return;
  const next = dismissRows(s.inboxDismissed, [{ key, kind: "failed", at }]);
  set({ inboxDismissed: next });
  store(STORAGE.inboxDismissed, JSON.stringify(next));
}
