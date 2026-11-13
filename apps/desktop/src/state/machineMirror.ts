// The durable copy of other environments' threads (docs/data-store.md › Thread mirror).
//
// `state/machineCache.ts` keeps a bounded copy in the web view's storage so a machine that is away
// still draws. This keeps everything the app reads from a machine on this computer's disk, through
// this computer's own engine (`_rustcode/mirror`, engine/src/acp/mirror.rs): the machine's whole
// thread list and the transcript of each thread read from it. A wiped web view, a reinstalled app
// or a machine that is offline, wiped or forgotten no longer takes its threads with it.
//
// The machine stays the owner: nothing here is ever sent back to it, and a mirrored thread is
// read-only. Replication is incremental and throttled: a list is written at most every
// `LIST_MS` while it changes, a transcript at most as often as the store keeps one, and the engine
// skips a list or transcript it already holds. Every call is best effort and never throws: a
// mirror that cannot be written costs the user nothing they had.

import type { SessionSummary } from "../acp/types";
import type { CachedTranscript } from "./machineCache";
import type { ThreadState, TranscriptItem } from "./thread";
import { splitThreadKey } from "./threadKeys";

/** A mirrored list is written at most this often per machine while it changes. */
export const LIST_MS = 5_000;
/** A transcript larger than this is not sent; the copy kept before stays. */
export const MAX_MIRROR_TRANSCRIPT_CHARS = 8_000_000;
/** At most this many mirrored rows are drawn for a machine that is away. */
export const MAX_MIRRORED_ROWS = 1_000;

/** Sends one `_rustcode/mirror` request to this computer's engine. */
export type MirrorRequest = (params: Record<string, unknown>) => Promise<unknown>;

let send: MirrorRequest | null = null;
/** `null` until the engine said; `false` once it refused a write because the switch is off. */
let switchedOn: boolean | null = null;
let listDelayMs = LIST_MS;

/** The store gives the mirror its way to the local engine (it cannot import the store itself). */
export function setMirrorRequest(next: MirrorRequest | null): void {
  send = next;
  switchedOn = null;
}

const isObject = (value: unknown): value is Record<string, unknown> =>
  !!value && typeof value === "object" && !Array.isArray(value);

async function ask(params: Record<string, unknown>): Promise<unknown> {
  if (!send) return null;
  try {
    return await send(params);
  } catch (error) {
    // The engine refuses writes while the switch is off; an older engine does not know the method.
    if (/mirroring is off/i.test(error instanceof Error ? error.message : String(error))) {
      switchedOn = false;
    }
    return null;
  }
}

// ---------------------------------------------------------------- the switch

/** What the user chose, and what the mirror holds. */
export interface MirrorStatus {
  enabled: boolean;
  excludedProjects: string[];
  hosts: Record<string, { threads: number; transcripts: number; bytes: number }>;
}

/** The switch, the project opt-outs and what is held, or null if the engine does not answer. */
export async function readMirrorStatus(): Promise<MirrorStatus | null> {
  const answer = await ask({ action: "status" });
  return parseStatus(answer);
}

/** Changes the switch and/or the opt-outs; answers the new status. */
export async function setMirrorSettings(change: {
  enabled?: boolean;
  excludedProjects?: string[];
}): Promise<MirrorStatus | null> {
  const status = parseStatus(await ask({ action: "setSettings", ...change }));
  if (status) switchedOn = status.enabled;
  return status;
}

function parseStatus(value: unknown): MirrorStatus | null {
  if (!isObject(value) || typeof value.enabled !== "boolean") return null;
  const hosts: MirrorStatus["hosts"] = {};
  if (isObject(value.hosts)) {
    for (const [host, entry] of Object.entries(value.hosts)) {
      if (!isObject(entry)) continue;
      hosts[host] = {
        threads: Number(entry.threads) || 0,
        transcripts: Number(entry.transcripts) || 0,
        bytes: Number(entry.bytes) || 0,
      };
    }
  }
  const excluded = Array.isArray(value.excludedProjects)
    ? value.excludedProjects.filter((path): path is string => typeof path === "string")
    : [];
  switchedOn = value.enabled;
  return { enabled: value.enabled, excludedProjects: excluded, hosts };
}

// ---------------------------------------------------------------- writing

const listTimers = new Map<string, ReturnType<typeof setTimeout>>();
const pendingLists = new Map<string, readonly SessionSummary[]>();

/**
 * Mirrors what a machine listed. Writes are coalesced per machine: the newest list goes out once
 * `LIST_MS` has passed, so a stream of small changes costs one write.
 */
export function mirrorThreadList(host: string, sessions: readonly SessionSummary[]): void {
  if (!host || !send || switchedOn === false || !sessions.length) return;
  pendingLists.set(host, sessions);
  if (listTimers.has(host)) return;
  listTimers.set(
    host,
    setTimeout(() => {
      listTimers.delete(host);
      void flushList(host);
    }, listDelayMs),
  );
}

async function flushList(host: string): Promise<void> {
  const sessions = pendingLists.get(host);
  pendingLists.delete(host);
  if (!sessions?.length || switchedOn === false) return;
  await ask({ action: "putList", host, sessions: sessions.map(rowToMirror) });
}

/** A row as it is mirrored: what the machine reported, without what is only true while it answers. */
function rowToMirror(row: SessionSummary): SessionSummary {
  const { status, host: _host, ...rest } = row;
  return status === "done" || status === "review" ? { ...rest, status } : rest;
}

/** The part of a thread worth keeping for good: the conversation, minus attachment bytes. */
export function transcriptToMirror(thread: ThreadState): { items: TranscriptItem[] } | null {
  if (!thread.items.length) return null;
  const items = thread.items.map((item) => {
    if (item.kind === "user" && item.attachments) {
      const { attachments: _attachments, ...rest } = item;
      return rest;
    }
    return item;
  });
  return JSON.stringify(items).length > MAX_MIRROR_TRANSCRIPT_CHARS ? null : { items };
}

/** Mirrors the transcript just read from a thread on a machine. `key` is the thread's key. */
export function mirrorTranscript(thread: ThreadState, key: string, now = Date.now()): void {
  const { host, id } = splitThreadKey(key);
  if (!host || !send || switchedOn === false) return;
  const body = transcriptToMirror(thread);
  if (!body) return;
  void ask({
    action: "putTranscript",
    host,
    id,
    cwd: thread.cwd,
    revision: now,
    transcript: {
      cwd: thread.cwd,
      title: thread.title,
      harness: thread.harness,
      ...body,
    },
  });
}

// ---------------------------------------------------------------- reading

/** The rows the mirror holds for a machine, newest first (all of them, not only the 200 cached). */
export async function loadMirroredList(host: string): Promise<SessionSummary[]> {
  const answer = await ask({ action: "list", host });
  if (!isObject(answer) || !isObject(answer.hosts)) return [];
  const entry = answer.hosts[host];
  if (!isObject(entry) || !Array.isArray(entry.threads)) return [];
  return entry.threads
    .flatMap((thread) => (isObject(thread) ? [thread.session] : []))
    .filter(
      (row): row is SessionSummary =>
        isObject(row) && typeof row.id === "string" && typeof row.cwd === "string",
    )
    .slice(0, MAX_MIRRORED_ROWS);
}

/** The transcript the mirror holds for a thread key, or null if it holds none. */
export async function loadMirroredTranscript(key: string): Promise<CachedTranscript | null> {
  const { host, id } = splitThreadKey(key);
  if (!host) return null;
  const answer = await ask({ action: "transcript", host, id });
  if (!isObject(answer) || answer.found !== true || !isObject(answer.transcript)) return null;
  const body = answer.transcript;
  if (!Array.isArray(body.items)) return null;
  const cwd = typeof body.cwd === "string" ? body.cwd : typeof answer.cwd === "string" ? answer.cwd : "";
  return {
    key,
    cwd,
    title: typeof body.title === "string" ? body.title : undefined,
    harness: isObject(body.harness) ? (body.harness as unknown as ThreadState["harness"]) : undefined,
    items: body.items as TranscriptItem[],
    savedAt: typeof answer.savedAt === "number" ? answer.savedAt : Date.now(),
  };
}

/**
 * The mirror's rows that a machine's current rows lack: threads it listed once and no longer lists
 * (a machine that is away lists the 200 newest it was last read for, and a wiped one lists none).
 * The machine's own rows win; nothing is drawn twice.
 */
export function withMirroredRows(
  current: readonly SessionSummary[],
  mirrored: readonly SessionSummary[],
): SessionSummary[] {
  const known = new Set(current.map((row) => row.id));
  const extra = mirrored.filter((row) => !known.has(row.id));
  return extra.length ? [...current, ...extra] : [...current];
}

/** Tests only: how long a list waits before it is sent. */
export function setListDelayForTests(ms: number): void {
  listDelayMs = ms;
}

/** Tests only: forget timers, queued lists and the engine's answer. */
export function resetMirrorForTests(): void {
  for (const timer of listTimers.values()) clearTimeout(timer);
  listTimers.clear();
  pendingLists.clear();
  send = null;
  switchedOn = null;
  listDelayMs = LIST_MS;
}
