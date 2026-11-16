// Thread status: looking a thread up, its review/done/idle status and the marks that keep it, and
// settling threads. Split out of state/store.ts; the state these write still lives in the app store.

import { type AcpConnection, RpcError } from "../acp/jsonrpc";
import type { SessionSummary } from "../acp/types";
import { type ThreadStatus, V2Method } from "../acp/v2";
import { scheduleAutoCompact } from "./autoCompactRun";
import { clearWaitingLinks } from "./host";
import { pendingStatus, queueChange, storedStatus } from "./machineOutbox";
import { engineBusy } from "./runState";
import {
  type AppState,
  connectionForThread,
  dismissToast,
  errorText,
  machineOfThread,
  remoteStatusPending,
  STORAGE,
  startNewThread,
  store,
  syncNote,
  threadIsRemote,
  toast,
  useApp,
  without,
} from "./store";
import { isContinueMessage, isWakeMessage } from "./thread";
import { splitThreadKey } from "./threadKeys";

// Read through `useApp` at call time: store.ts imports this module, so `useApp` is not defined yet
// while this one loads.
const get = () => useApp.getState();
const set = (patch: Partial<AppState> | ((s: AppState) => Partial<AppState>)) => useApp.setState(patch);

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

/**
 * What a thread is doing, for chips and the board: running or needs input
 * from the live turn, else review or done as the engine (or the app, on an
 * older engine) keeps it.
 */
export function statusOf(s: AppState, id: string): ThreadStatus {
  const t = s.threads[id];
  if (t?.permissions.length) return "needs_input";
  if (t?.running) return "running";
  // The engine's list says it works and this window has no turn for it (`reconcileRunState`).
  if (s.engineRunning[id]) return "running";
  const listed = sessionById(s, id)?.status;
  // A thread this window holds no stream for (never opened, or reset by a restart) has only the
  // engine's list to go by, and the list is read every few seconds (`startRunStatePoll`).
  if (!t?.loaded && !threadIsRemote(id) && engineBusy(listed)) return listed as ThreadStatus;
  const pending = remoteStatusPending.get(id);
  if (pending) return pending.status;
  const queued = threadIsRemote(id) ? pendingStatus(id) : undefined;
  if (queued) return queued;
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
export function reconcileStatusMarks(listed: SessionSummary[]) {
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

/** Settled threads keep user messages, but no longer wait for automatic notices. */
export function clearSettledNotifications(ids: string[]): void {
  clearWaitingLinks(ids.filter((id) => !threadIsRemote(id)));
  const wanted = new Set(ids);
  set((s) => ({
    threads: Object.fromEntries(
      Object.entries(s.threads).map(([id, thread]) => [
        id,
        wanted.has(id)
          ? {
              ...thread,
              queue: thread.queue.filter((row) => !isContinueMessage(row.text) && !isWakeMessage(row.text)),
            }
          : thread,
      ]),
    ),
    finished: Object.fromEntries(Object.entries(s.finished).filter(([id]) => !wanted.has(id))),
    failed: Object.fromEntries(Object.entries(s.failed).filter(([id]) => !wanted.has(id))),
  }));
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
  if (status === "done") clearSettledNotifications(ids);
  const remote = ids.filter(threadIsRemote);
  const remoteRequests = remote.map(async (id) => {
    const conn = connectionForThread(id);
    if (!conn) {
      // A paired machine that is away gets the change when it is back (`state/machineOutbox.ts`);
      // the row shows it now.
      if (!machineOfThread(id)) {
        toast("Could not save the status: That computer is not connected.", "error");
        return 1;
      }
      queueChange({
        kind: "status",
        key: id,
        value: status,
        base: storedStatus(statusOf(get(), id)),
        at: Date.now(),
      });
      set((s) => ({
        sessions: s.sessions.map((x) => (x.id === id ? { ...x, status } : x)),
        ...(status === "done"
          ? {
              failed: Object.fromEntries(Object.entries(s.failed).filter(([key]) => key !== id)),
              finished: without(s.finished, id),
            }
          : {}),
      }));
      return 0;
    }
    let pending = remoteStatusPending.get(id);
    if (!pending) {
      pending = { status, confirmed: statusOf(get(), id), tail: Promise.resolve(), version: 0 };
      remoteStatusPending.set(id, pending);
    }
    pending.status = status;
    const version = ++pending.version;
    const entry = pending;
    set((s) => ({ sessions: s.sessions.map((x) => (x.id === id ? { ...x, status } : x)) }));
    let failed = false;
    const request = entry.tail.then(async () => {
      try {
        await conn.request(V2Method.sessionStatus, { session_id: splitThreadKey(id).id, status }, 10_000);
        entry.confirmed = status;
        clearStatusMark(id);
        if (status === "done") {
          set((s) => ({
            failed: Object.fromEntries(Object.entries(s.failed).filter(([key]) => key !== id)),
            finished: without(s.finished, id),
          }));
        }
      } catch (error) {
        failed = true;
        toast(`Could not save the status: ${errorText(error)}`, "error");
      }
      if (entry.version === version) {
        remoteStatusPending.delete(id);
        set((s) => ({
          sessions: s.sessions.map((x) => (x.id === id ? { ...x, status: entry.confirmed } : x)),
        }));
      }
    });
    entry.tail = request;
    await request;
    return failed ? 1 : 0;
  });
  const remoteErrors = () =>
    Promise.all(remoteRequests).then((results) => results.reduce<number>((a, b) => a + b, 0));
  ids = ids.filter((id) => !threadIsRemote(id));
  if (!ids.length) return remoteErrors();
  const wanted = new Set(ids);
  set((s) => ({
    sessions: s.sessions.map((x) => (wanted.has(x.id) ? { ...x, status } : x)),
    // A settled thread asks nothing of the user: a turn of it that failed is no longer theirs to
    // retry (`cleanup_worktree`, a fan-out's losers, a delegate its parent replaced), so its
    // Inbox card goes with the settle.
    ...(status === "done"
      ? {
          failed: Object.fromEntries(Object.entries(s.failed).filter(([id]) => !wanted.has(id))),
          // A settled thread is read: its unseen finish is not news to come back to.
          finished: Object.fromEntries(Object.entries(s.finished).filter(([id]) => !wanted.has(id))),
        }
      : {}),
  }));
  if (status === "idle") for (const id of ids) clearStatusMark(id);
  else saveMarks({ ...get().statusMarks, ...Object.fromEntries(ids.map((id) => [id, status])) });
  for (const id of ids) scheduleAutoCompact(id);
  if (!get().features.status) return remoteErrors();
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
  return errors.length + (await remoteErrors());
}

/**
 * Leaves the thread on screen for the new-thread screen in the same folder when it is among `ids`,
 * which were just settled. Only the chat moves: the board, review and compare have no new-thread
 * screen, so a settle there stays on that view.
 */
export function leaveSettledThreads(ids: string[]): void {
  const { view, selectedId } = get();
  if (view.kind === "chat" && selectedId && ids.includes(selectedId)) startNewThread();
}

/**
 * Settles threads (marks them Done) because the user asked: one Undo toast puts
 * each back the way it was (review stays review, the rest go back to idle).
 *
 * Settling the thread on screen in the chat leaves it, with the panel, for the
 * new-thread screen in the same folder; the board, review and compare have no
 * new-thread screen, so settling there stays on that view. An agent's
 * `update_session` settle is the user's own ask too ("settle this") and leaves
 * the screen the same way (`leaveSettledThreads`). A caller that is not the
 * user — an agent's `organize`, a fan-out's cleanup, a thread merged on the
 * engine — calls `setThreadStatus` alone, which never moves the screen.
 */
export async function settleThreads(ids: string[]): Promise<void> {
  const state = get();
  const before = ids.map((id) => ({ id, status: statusOf(state, id) })).filter((x) => x.status !== "done");
  if (!before.length) {
    toast("Already settled", "info");
    return;
  }
  const optimistic = before.every((x) => threadIsRemote(x.id) && connectionForThread(x.id));
  const saving = setThreadsStatus(
    before.map((x) => x.id),
    "done",
  );
  if (!optimistic && (await saving)) return;
  if (get().selectedId === state.selectedId) leaveSettledThreads(before.map((x) => x.id));
  const title =
    before.length === 1
      ? sessionById(state, before[0]!.id)?.title || get().threads[before[0]!.id]?.title || "New thread"
      : "";
  const toastId = toast(
    `${before.length === 1 ? `Settled "${title}"` : `Settled ${before.length} threads`}${syncNote(
      before.map((x) => x.id),
      ".",
    )}`,
    "info",
    {
      action: {
        label: "Undo",
        run: () => {
          const review = before.filter((x) => x.status === "review").map((x) => x.id);
          const rest = before.filter((x) => x.status !== "review").map((x) => x.id);
          if (review.length) void setThreadsStatus(review, "review");
          if (rest.length) void setThreadsStatus(rest, "idle");
        },
      },
    },
  );
  if (optimistic && (await saving)) dismissToast(toastId);
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
  toast(
    `${settled.length === 1 ? "Unsettled" : `Unsettled ${settled.length} threads`}${syncNote(settled, ".")}`,
    "info",
  );
}
