// The queue a thread keeps across restarts: the messages and wakes waiting on it, in the order they
// will run, written to app storage while the app runs and read back when the thread is opened again
// (state/host.ts). Bounded like drafts: a queue is a handful of messages, not a transcript, and one
// row nobody could read back is not worth a web view's whole storage.
//
// An id that left a queue is recorded as sent, so a restart between sending a row and writing the
// queue back cannot deliver it twice: the same id is never restored.
//
// No React and no I/O here, so the store wiring and the tests share it.

import type { ComposeExtras } from "./compose";
import type { QueuedPrompt } from "./thread";

/** Where the book lives in app storage. */
export const QUEUES_KEY = "rustcode.queues";
/** A row's text over this is not kept: a paste that big is not a message anyone queued to read. */
export const MAX_ROW_TEXT = 100 * 1024;
/** A row's command, links and attachments are kept up to this size in all: an image rides inline. */
export const MAX_ROW_EXTRAS = 64 * 1024;
/** At most this many rows per thread; the oldest go first. */
export const MAX_ROWS = 50;
/** At most this many threads keep a queue; the one queued longest ago goes first. */
export const MAX_THREADS = 100;
/** At most this many sent ids are remembered, newest last. */
export const MAX_ACKED = 200;

/** One message waiting on a thread, as it is kept. */
export interface QueueRow {
  id: string;
  text: string;
  /** `wake` is the app's own turn for its children; `user` is everything else, a link message too. */
  kind: "user" | "wake";
  /** When it was queued. The order is the array's; this is what the row says about itself. */
  created_at: number;
  /** What went with the text (a command, link chips, pictures), when it is small enough to keep. */
  extras?: ComposeExtras;
  fromView?: boolean;
  /** Delivered for another thread (orchestration link id). */
  link?: string;
  /** The links a wake reports, and of those the ones that wait on the user. */
  wakeLinks?: string[];
  wakeAsking?: string[];
  /** It stops the turn it waits behind instead of waiting for the next one. */
  restart?: boolean;
}

/** Queued rows per thread id, in the order they will run, and the ids already sent. */
export interface QueueBook {
  threads: Record<string, QueueRow[]>;
  acked: string[];
}

export const emptyQueues = (): QueueBook => ({ threads: {}, acked: [] });

const isRow = (value: unknown): value is QueueRow => {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  const row = value as Partial<QueueRow>;
  return (
    typeof row.id === "string" &&
    !!row.id &&
    typeof row.text === "string" &&
    row.text.length > 0 &&
    row.text.length <= MAX_ROW_TEXT &&
    (row.kind === "user" || row.kind === "wake")
  );
};

const strings = (value: unknown): string[] | undefined =>
  Array.isArray(value) ? value.filter((x): x is string => typeof x === "string") : undefined;

/** A parsed book: readable rows only, in the order they were kept, bounded. */
export function parseQueueBook(raw: string | null): QueueBook {
  if (!raw) return emptyQueues();
  let value: unknown;
  try {
    value = JSON.parse(raw);
  } catch {
    // A book that is not JSON (an older build, a hand edit) starts empty.
    return emptyQueues();
  }
  if (!value || typeof value !== "object" || Array.isArray(value)) return emptyQueues();
  const { threads, acked } = value as { threads?: unknown; acked?: unknown };
  const rows: Record<string, QueueRow[]> = {};
  if (threads && typeof threads === "object" && !Array.isArray(threads)) {
    for (const [id, list] of Object.entries(threads as Record<string, unknown>)) {
      if (!id || !Array.isArray(list)) continue;
      const kept = list.filter(isRow).map((row) => ({
        ...row,
        created_at: typeof row.created_at === "number" ? row.created_at : 0,
        wakeLinks: strings(row.wakeLinks),
        wakeAsking: strings(row.wakeAsking),
      }));
      if (kept.length) rows[id] = kept;
    }
  }
  return persistable({ threads: rows, acked: strings(acked) ?? [] });
}

/** A queued item as the row that keeps it, or null when it is too big to keep and read back. */
export function rowOf(item: QueuedPrompt, now: number, created?: number): QueueRow | null {
  const text = item.text;
  if (!text.length || text.length > MAX_ROW_TEXT) return null;
  const row: QueueRow = {
    id: item.id,
    text,
    kind: item.wake ? "wake" : "user",
    created_at: created ?? now,
  };
  if (item.extras && JSON.stringify(item.extras).length <= MAX_ROW_EXTRAS) row.extras = item.extras;
  if (item.fromView) row.fromView = true;
  if (item.link) row.link = item.link;
  if (item.wakeLinks?.length) row.wakeLinks = [...item.wakeLinks];
  if (item.wakeAsking?.length) row.wakeAsking = [...item.wakeAsking];
  if (item.restart) row.restart = true;
  return row;
}

/** A thread's queue as rows, keeping each row's first `created_at` when it waited before. */
export function rowsOf(
  queue: readonly QueuedPrompt[],
  now: number,
  previous: readonly QueueRow[] = [],
): QueueRow[] {
  const before = new Map(previous.map((row) => [row.id, row.created_at]));
  const rows: QueueRow[] = [];
  for (const item of queue) {
    const row = rowOf(item, now, before.get(item.id));
    if (row) rows.push(row);
  }
  return rows;
}

/** The queued items a thread kept, without the ids it already sent. */
export function queueOf(rows: readonly QueueRow[], acked: readonly string[] = []): QueuedPrompt[] {
  const sent = new Set(acked);
  const out: QueuedPrompt[] = [];
  for (const row of rows) {
    if (sent.has(row.id)) continue;
    const item: QueuedPrompt = { id: row.id, text: row.text };
    if (row.extras) item.extras = row.extras;
    if (row.fromView) item.fromView = true;
    if (row.link) item.link = row.link;
    if (row.kind === "wake") item.wake = true;
    if (row.wakeLinks?.length) item.wakeLinks = row.wakeLinks;
    if (row.wakeAsking?.length) item.wakeAsking = row.wakeAsking;
    if (row.restart) item.restart = true;
    out.push(item);
  }
  return out;
}

/** The rows a thread still has to run: what it kept, less the ones already sent. */
export function pendingRows(book: QueueBook, id: string): QueueRow[] {
  const sent = new Set(book.acked);
  return (book.threads[id] ?? []).filter((row) => !sent.has(row.id));
}

/**
 * The book with one thread's queue written. A row that was there and is not any more was sent or
 * dropped: its id is recorded as sent, so a restart cannot restore it (nothing is ever delivered
 * twice), and the record is trimmed to {@link MAX_ACKED}. A thread with nothing left keeps no entry,
 * and the thread goes to the newest end so the cap drops the one queued longest ago.
 */
export function withQueue(
  book: QueueBook,
  id: string,
  queue: readonly QueuedPrompt[],
  now: number,
): QueueBook {
  const previous = book.threads[id] ?? [];
  const rows = rowsOf(queue, now, previous);
  const here = new Set(rows.map((row) => row.id));
  const gone = previous.filter((row) => !here.has(row.id)).map((row) => row.id);
  const { [id]: _was, ...rest } = book.threads;
  const acked = [...book.acked.filter((x) => !gone.includes(x)), ...gone].slice(-MAX_ACKED);
  const threads = rows.length ? { ...rest, [id]: rows } : rest;
  return { threads, acked };
}

/** The book as it goes to storage: bounded rows, threads and sent ids, and nothing unreadable. */
export function persistable(book: QueueBook): QueueBook {
  const entries = Object.entries(book.threads)
    .map(([id, rows]) => [id, rows.slice(-MAX_ROWS)] as const)
    .filter(([, rows]) => rows.length)
    .slice(-MAX_THREADS);
  return { threads: Object.fromEntries(entries), acked: book.acked.slice(-MAX_ACKED) };
}
