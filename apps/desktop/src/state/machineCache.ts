// What this computer last read from each paired machine, kept in app storage so a machine that is
// away (or an app that just started while it is) still shows its threads: the sidebar rows from
// its last thread list, and the transcript of each thread that was opened. A copy is only ever
// shown read-only while its machine is offline; a live read replaces it (docs/desktop.md ›
// Several machines).
//
// Bounded, so a long-lived install cannot fill the web view's storage: the 200 newest threads per
// machine, transcripts only for threads opened in the last 30 days, each capped and all of them
// together capped. Nothing here holds a credential: a list is the rows the sidebar draws, a
// transcript is the conversation the machine already showed. Pure over a storage, so the store
// and the tests share it (state/machines.ts, state/store.ts).

import type { SessionSummary } from "../acp/types";
import type { ThreadState, TranscriptItem } from "./thread";
import { splitThreadKey } from "./threadKeys";

/** Where a machine's thread list lives in app storage: this prefix, then its id. */
export const LIST_KEY_PREFIX = "rustcode.machineThreads.";
/** Where a thread's transcript lives: this prefix, then its key (`‹machine›/‹id›`). */
export const TRANSCRIPT_KEY_PREFIX = "rustcode.machineTranscript.";
/** What is kept, and when each was opened and how big it is, by thread key. */
export const TRANSCRIPT_INDEX_KEY = "rustcode.machineTranscripts";

/** At most this many threads are kept per machine, newest first. */
export const MAX_CACHED_THREADS = 200;
/** A transcript of a thread not opened for this long is dropped. */
export const TRANSCRIPT_MAX_AGE_MS = 30 * 24 * 60 * 60 * 1000;
/** One transcript larger than this keeps only its newest rows. */
export const MAX_TRANSCRIPT_CHARS = 250_000;
/** All transcripts together stay under this; the ones opened longest ago go first. */
export const MAX_TRANSCRIPTS_CHARS = 2_000_000;
/** A tool's output and detail are cut to this much: the conversation matters, not the log. */
const MAX_TOOL_TEXT_CHARS = 4_000;

/** The part of the web view's storage this reads and writes. */
export interface CacheStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
}

let backing: CacheStorage | null | undefined;

function storage(): CacheStorage | null {
  if (backing !== undefined) return backing;
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

/** Tests only: keep the cache in this storage (or none) instead of the web view's. */
export function useCacheStorage(next: CacheStorage | null | undefined): void {
  backing = next;
}

function read(key: string): unknown {
  try {
    const raw = storage()?.getItem(key);
    return raw ? (JSON.parse(raw) as unknown) : null;
  } catch {
    return null;
  }
}

function write(key: string, value: unknown): boolean {
  try {
    const target = storage();
    if (!target) return false;
    target.setItem(key, JSON.stringify(value));
    return true;
  } catch {
    // Storage can be full or unavailable: the copy is just not kept.
    return false;
  }
}

function remove(key: string): void {
  try {
    storage()?.removeItem(key);
  } catch {
    // Nothing to remove.
  }
}

const isObject = (value: unknown): value is Record<string, unknown> =>
  !!value && typeof value === "object" && !Array.isArray(value);

// ---------------------------------------------------------------- thread lists

/**
 * A machine's thread list as it is kept: the newest threads, without what is only true while the
 * machine answers (a running status, the host the list came through).
 */
export function listToKeep(sessions: readonly SessionSummary[]): SessionSummary[] {
  return [...sessions]
    .sort((a, b) => (b.updatedAt ?? 0) - (a.updatedAt ?? 0))
    .slice(0, MAX_CACHED_THREADS)
    .map(({ status: _status, host: _host, ...row }) => row);
}

/** Keeps what a machine last listed. */
export function saveThreadList(host: string, sessions: readonly SessionSummary[]): void {
  if (!host) return;
  const rows = listToKeep(sessions);
  if (!rows.length) {
    remove(LIST_KEY_PREFIX + host);
    return;
  }
  write(LIST_KEY_PREFIX + host, { savedAt: Date.now(), sessions: rows });
}

/** What a machine last listed, or nothing if it never did. */
export function loadThreadList(host: string): SessionSummary[] {
  const value = read(LIST_KEY_PREFIX + host);
  if (!isObject(value) || !Array.isArray(value.sessions)) return [];
  return value.sessions
    .filter(
      (row): row is SessionSummary =>
        isObject(row) && typeof row.id === "string" && typeof row.cwd === "string",
    )
    .slice(0, MAX_CACHED_THREADS);
}

// ---------------------------------------------------------------- transcripts

/** What of a thread is kept to show it again: the conversation, not the live state. */
export interface CachedTranscript {
  key: string;
  cwd: string;
  title?: string;
  harness?: ThreadState["harness"];
  items: TranscriptItem[];
  /** When the thread was last read from its machine, in ms. */
  savedAt: number;
}

interface IndexEntry {
  openedAt: number;
  chars: number;
}

function readIndex(): Record<string, IndexEntry> {
  const value = read(TRANSCRIPT_INDEX_KEY);
  if (!isObject(value)) return {};
  const index: Record<string, IndexEntry> = {};
  for (const [key, entry] of Object.entries(value)) {
    if (isObject(entry) && typeof entry.openedAt === "number" && typeof entry.chars === "number") {
      index[key] = { openedAt: entry.openedAt, chars: entry.chars };
    }
  }
  return index;
}

const cut = (text: string | undefined) =>
  text !== undefined && text.length > MAX_TOOL_TEXT_CHARS ? `${text.slice(0, MAX_TOOL_TEXT_CHARS)}…` : text;

/** A row as it is kept: attachments (their bytes) are left out, and a tool's log is cut short. */
function rowToKeep(item: TranscriptItem): TranscriptItem {
  if (item.kind === "user" && item.attachments) {
    const { attachments: _attachments, ...rest } = item;
    return rest;
  }
  if (item.kind === "tool") return { ...item, output: cut(item.output), detail: cut(item.detail) };
  return item;
}

/**
 * The transcript to keep from a thread, or null when it has nothing to show. A transcript over the
 * cap keeps its newest rows: that is the end of the conversation, where the user left it.
 */
export function transcriptToKeep(
  thread: ThreadState,
  key: string,
  now = Date.now(),
): CachedTranscript | null {
  if (!thread.items.length) return null;
  const full: CachedTranscript = {
    key,
    cwd: thread.cwd,
    title: thread.title,
    harness: thread.harness,
    items: thread.items.map(rowToKeep),
    savedAt: now,
  };
  let size = JSON.stringify(full).length;
  if (size <= MAX_TRANSCRIPT_CHARS) return full;
  // Drop rows from the start in steps until it fits, rather than measuring each one again.
  let items = full.items;
  while (items.length > 1 && size > MAX_TRANSCRIPT_CHARS) {
    items = items.slice(Math.max(1, Math.ceil(items.length / 4)));
    size = JSON.stringify({ ...full, items }).length;
  }
  return size <= MAX_TRANSCRIPT_CHARS ? { ...full, items } : null;
}

/**
 * Keeps a thread's transcript, opened now, then makes room: transcripts of threads not opened in
 * 30 days go, and so do the ones opened longest ago while all of them are over the cap.
 */
export function saveTranscript(thread: ThreadState, key: string, now = Date.now()): boolean {
  const kept = transcriptToKeep(thread, key, now);
  if (!kept) return false;
  const body = JSON.stringify(kept);
  if (!write(TRANSCRIPT_KEY_PREFIX + key, kept)) return false;
  const index = readIndex();
  index[key] = { openedAt: now, chars: body.length };
  writeIndex(pruned(index, now));
  return true;
}

/** The index without what aged out or does not fit; the transcripts it drops leave storage. */
function pruned(index: Record<string, IndexEntry>, now: number): Record<string, IndexEntry> {
  const keep = { ...index };
  const drop = (key: string) => {
    delete keep[key];
    remove(TRANSCRIPT_KEY_PREFIX + key);
  };
  for (const [key, entry] of Object.entries(keep)) {
    if (now - entry.openedAt > TRANSCRIPT_MAX_AGE_MS) drop(key);
  }
  let total = Object.values(keep).reduce((sum, entry) => sum + entry.chars, 0);
  const oldestFirst = Object.entries(keep).sort((a, b) => a[1].openedAt - b[1].openedAt);
  for (const [key, entry] of oldestFirst) {
    if (total <= MAX_TRANSCRIPTS_CHARS) break;
    total -= entry.chars;
    drop(key);
  }
  return keep;
}

function writeIndex(index: Record<string, IndexEntry>): void {
  if (Object.keys(index).length) write(TRANSCRIPT_INDEX_KEY, index);
  else remove(TRANSCRIPT_INDEX_KEY);
}

/** The transcript last kept for a thread key, or null if there is none (or it aged out). */
export function loadTranscript(key: string, now = Date.now()): CachedTranscript | null {
  const entry = readIndex()[key];
  if (!entry || now - entry.openedAt > TRANSCRIPT_MAX_AGE_MS) return null;
  const value = read(TRANSCRIPT_KEY_PREFIX + key);
  if (!isObject(value) || value.key !== key || !Array.isArray(value.items) || typeof value.cwd !== "string") {
    return null;
  }
  return {
    key,
    cwd: value.cwd,
    title: typeof value.title === "string" ? value.title : undefined,
    harness: isObject(value.harness) ? (value.harness as unknown as ThreadState["harness"]) : undefined,
    items: value.items as TranscriptItem[],
    savedAt: typeof value.savedAt === "number" ? value.savedAt : entry.openedAt,
  };
}

/** The thread as the store holds it while its machine is away: the kept rows, read-only. */
export function threadFromTranscript(cached: CachedTranscript): ThreadState {
  const toolIndex: Record<string, number> = {};
  cached.items.forEach((item, i) => {
    if (item.kind === "tool") toolIndex[item.id] = i;
  });
  return {
    id: cached.key,
    cwd: cached.cwd,
    title: cached.title,
    items: cached.items,
    toolIndex,
    plan: [],
    planClocks: {},
    steers: [],
    configOptions: [],
    permissions: [],
    queue: [],
    running: false,
    loading: false,
    loaded: true,
    harness: cached.harness,
  };
}

/** Drops one thread's transcript (it was deleted on its machine). */
export function forgetTranscript(key: string): void {
  remove(TRANSCRIPT_KEY_PREFIX + key);
  const index = readIndex();
  if (index[key]) {
    delete index[key];
    writeIndex(index);
  }
}

/** Drops everything kept for a machine (it was removed): its list and every transcript. */
export function forgetMachineCache(host: string): void {
  if (!host) return;
  remove(LIST_KEY_PREFIX + host);
  const index = readIndex();
  let changed = false;
  for (const key of Object.keys(index)) {
    if (splitThreadKey(key).host !== host) continue;
    remove(TRANSCRIPT_KEY_PREFIX + key);
    delete index[key];
    changed = true;
  }
  if (changed) writeIndex(index);
}

// ---------------------------------------------------------------- the offline notice

/**
 * What the chat says over a remote thread whose machine is away. With a copy it says the thread is
 * the last one this computer saw; without one, that nothing was kept. `since` is the time the
 * machine went away (`offlineSinceLabel`), `reconnecting` that the shell is already retrying it.
 */
export function offlineNotice(options: {
  machine?: string;
  since?: string;
  reconnecting: boolean;
  hasCopy: boolean;
}): string {
  const name = options.machine ?? "The machine";
  const lead = options.reconnecting
    ? `Reconnecting to ${name}…`
    : `${name} is offline${options.since ? ` since ${options.since}` : ""}.`;
  return options.hasCopy
    ? `${lead} This is the last copy this computer saw; it updates when the machine is back.`
    : `${lead} This computer has no copy of this thread; it opens when the machine is back.`;
}
