import { readStorage, writeStorage } from "../lib/storage";
import { book, steeredLinks, wakeContext } from "./host";
import { steeredIntoTurn } from "./hostLinkTurns";
import { wakeStillOwed } from "./orchestration";
import {
  parseQueueBook,
  pendingRows,
  persistable,
  QUEUES_KEY,
  type QueueBook,
  queueOf,
  withQueue,
} from "./queues";
import { type AppState, openThread, resumeQueue, toast, updateThread, useApp } from "./store";
import { isLoaded, isLoading, isRunning, type QueuedPrompt } from "./thread";

// Keeping each thread's queue across restarts. Split out of host.ts, which re-exports it.

// ---------------------------------------------------------------- the queue across restarts

/**
 * A thread's queue is the app's memory, so a restart used to lose it: the messages a stopped turn
 * held, the follow-ups behind a running one, and a child's wake waiting for the parent's turn to
 * end. It is kept in app storage per thread id while the app runs and read back when a thread is
 * opened again (state/queues.ts), so a quit, a crash or an update does not take an instruction or a
 * child's result with it.
 *
 * Nothing is written for a thread the app has not finished loading: opening a thread starts it
 * empty, and those empty frames are not the user deleting their queue.
 */

/** The book as it is in memory: read once, on the first change after start-up. */
export let queueBook: QueueBook | undefined;
/** The book as storage last took it: a change that serialises the same is not written again. */
let keptQueues: string | undefined;
/** Putting a saved queue back changes the store, which notifies again: the nested pass is skipped. */
let restoring = false;
/** Whether this window is watching the store for queue changes: installed once, whatever reconnects. */
let watchingQueues = false;
/** At most this many threads are opened at start-up for the queue each kept. */
const RESTORE_MAX = 8;

// No storage (a private window): the queue is this window's only, as before. A web view whose
// storage is full keeps the queue in memory instead.
export function storedQueues(): string | null {
  return readStorage(QUEUES_KEY);
}

function keepQueues(text: string): void {
  writeStorage(QUEUES_KEY, text);
}

/**
 * Takes the queues the last run left, and watches this one: called once at start-up, after the
 * thread list is in (state/store.ts `connect`). A thread that kept a queue is opened, so its rows
 * are put back as it loads and, when the thread is idle, start running again — an instruction the
 * user queued before the restart is not left sitting in storage until they think to look.
 */
export function restoreQueuedThreads(): void {
  if (!queueBook) queueBook = parseQueueBook(storedQueues());
  if (!watchingQueues) {
    watchingQueues = true;
    // The state is read from the store, never diffed against a `previous` one: the store folds a run
    // of engine lines into one notification, so the pair it is given can already hold the change.
    useApp.subscribe((state) => queuesChanged(state));
  }
  const s = useApp.getState();
  const known = new Set(s.sessions.map((session) => session.id));
  const waiting = Object.keys(queueBook.threads).filter(
    (id) => known.has(id) && !isLoaded(s.threads[id]) && pendingRows(queueBook!, id).length > 0,
  );
  for (const id of waiting.slice(0, RESTORE_MAX)) void openThread(id);
}

/** Between tests: the queues are read again from storage, as a window that just started would. */
export function resetQueues(): void {
  queueBook = undefined;
  keptQueues = undefined;
  restoring = false;
  restoredQueues.clear();
  watchedQueues.clear();
  steeredLinks.clear();
  steeredIntoTurn.clear();
}

/** The threads whose saved queue this window has already put back. */
const restoredQueues = new Set<string>();
/** The queue array each thread had when it was last written, to notice a change without a diff. */
const watchedQueues = new Map<string, readonly QueuedPrompt[]>();

/**
 * One state change: a thread that has just finished loading gets its queue back, and a queue that
 * changed is written down. A thread that is loading is forgotten again, so a thread opened twice in
 * one run (a repair) reads its queue back the second time too.
 *
 * Only a loaded thread's queue is written — the empty frames a thread passes through while it loads
 * are not rows the user removed — and a row that left a loaded thread's queue was sent, so its id is
 * recorded and a restart cannot send it again.
 */
function queuesChanged(state: AppState): void {
  if (restoring) return;
  const loaded = queueBook;
  if (!loaded) return;
  const now = Date.now();
  let next = loaded;
  const drain: string[] = [];
  for (const [id, thread] of Object.entries(state.threads)) {
    if (!isLoaded(thread) || isLoading(thread)) {
      restoredQueues.delete(id);
      watchedQueues.delete(id);
      continue;
    }
    if (!restoredQueues.has(id)) {
      // First sight of it since it loaded: what it kept is put back beside anything queued meanwhile.
      restoredQueues.add(id);
      const rows = restorable(id, thread.queue);
      if (rows.length) {
        // A row whose pictures were too big to keep came back as its text alone: the queue waits
        // for the user rather than sending a message without what it was about.
        const back = new Set(rows.map((item) => item.id));
        const lost = pendingRows(loaded, id).some((row) => row.extrasLost && back.has(row.id));
        restoring = true;
        try {
          updateThread(id, (t) => ({
            ...t,
            queue: [...t.queue, ...rows],
            ...(lost ? { queueHeld: true } : {}),
          }));
        } finally {
          restoring = false;
        }
        if (lost) {
          toast(
            "A queued message lost its attachments in the restart, so its queue is held: check it, then Resume.",
            "error",
          );
        } else drain.push(id);
      }
      watchedQueues.set(id, useApp.getState().threads[id]?.queue ?? thread.queue);
      continue;
    }
    if (watchedQueues.get(id) === thread.queue) continue;
    watchedQueues.set(id, thread.queue);
    next = withQueue(next, id, thread.queue, now);
  }
  if (next !== loaded) {
    queueBook = next;
    const text = JSON.stringify(persistable(next));
    if (text !== keptQueues) {
      keptQueues = text;
      keepQueues(text);
    }
  }
  // Started after this pass, not inside it: a turn changes the store, and that change is a new pass.
  for (const id of drain) queueMicrotask(() => drainQueue(id));
}

/** The head of a restored queue starts a turn, once the thread is idle. */
function drainQueue(id: string): void {
  const thread = useApp.getState().threads[id];
  if (thread && !isRunning(thread) && !thread.queueHeld) void resumeQueue(id);
}

/**
 * The rows a thread's saved queue still owes, less anything it already has: the same id is never
 * twice, a wake whose links another waiting row already reports is the copy that was lost, and a
 * wake whose children the parent has heard about is nothing to run (`wakeStillOwed`).
 */
function restorable(id: string, queue: readonly QueuedPrompt[]): QueuedPrompt[] {
  const saved = queueBook;
  if (!saved) return [];
  const here = new Set(queue.map((item) => item.id));
  const carried = new Set(queue.flatMap((item) => (item.wake ? (item.wakeLinks ?? []) : [])));
  const ctx = wakeContext(Date.now());
  const out: QueuedPrompt[] = [];
  for (const item of queueOf(pendingRows(saved, id))) {
    if (here.has(item.id)) continue;
    const links = item.wakeLinks ?? [];
    if (item.wake) {
      if (links.length && links.every((link) => carried.has(link))) continue;
      if (!wakeStillOwed(book(), { links, asking: item.wakeAsking ?? [] }, ctx)) continue;
    }
    out.push(item);
    for (const link of links) carried.add(link);
  }
  return out;
}
