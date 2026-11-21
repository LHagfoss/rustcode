// Thread list guards: a thread the app knows about but no list names yet, a thread just deleted
// that a stale list must not bring back, and announcements held back while a session is being
// created. Split out of state/store.ts.

import type { SessionSummary } from "../acp/types";
import { useApp } from "./store";
import { isRunning } from "./thread";
import { LOCAL_HOST, splitThreadKey } from "./threadKeys";
import { sessionById } from "./threadStatus";

// Read through `useApp` at call time: store.ts imports this module, so `useApp` is not defined yet
// while this one loads.
const get = () => useApp.getState();

/**
 * Threads the app has been told about that a list has not got to yet: one this window made here or
 * on a machine (`session/new` answers at once), and one another client made (`_rustcode/threads/created`).
 * Kept by key with the row to draw and when it was kept, until a list names it or a deletion takes
 * it away; a thread no list ever names expires after {@link KEEP_UNLISTED_MS}. Both lists read this
 * one set: `refreshSessions` here, `mergeMachineLists` and `machines.ts` `readThreads` for a
 * machine (issue 3523).
 */
export const keptUnlistedThreads = new Map<string, { summary: SessionSummary; at: number }>();

/** How long a thread the app knows about waits for a list to name it. */
const KEEP_UNLISTED_MS = 5 * 60_000;

/**
 * Whether a keep is still due. The engine lists a thread once its first turn saved history, so a
 * first turn that runs past {@link KEEP_UNLISTED_MS} must not lose the row of a thread that is
 * alive: a keep for a thread this window sees running is renewed, not expired (a delegate's
 * `wait_session` would otherwise say "That thread is gone" for it).
 */
function keepDue(key: string, kept: { at: number }): boolean {
  const now = Date.now();
  if (now - kept.at <= KEEP_UNLISTED_MS) return true;
  const s = get();
  if (!isRunning(s.threads[key]) && sessionById(s, key)?.status !== "running") return false;
  kept.at = now;
  return true;
}

/** Keep a thread's row until a list names it, or a deletion takes it away. */
export function keepUnlisted(summary: SessionSummary): void {
  keptUnlistedThreads.set(summary.id, { summary, at: Date.now() });
}

/**
 * Whether the app still keeps `key`'s row for a list that has not named it: the backstop for a
 * link whose thread is gone (`state/host.ts` `vanished`). An expired row is dropped here.
 */
export function isKeptUnlisted(key: string): boolean {
  const kept = keptUnlistedThreads.get(key);
  if (!kept) return false;
  if (keepDue(key, kept)) return true;
  keptUnlistedThreads.delete(key);
  return false;
}

/**
 * The kept rows for `host` that `listed` (that machine's own ids) does not name, without forgetting
 * any: `mergeMachineLists` draws them, and the cache it merges may itself hold an announced row.
 * Only an authoritative list read settles a keep (`settleKeptUnlisted`).
 */
export function keptUnlisted(host: string, listed: ReadonlySet<string>): SessionSummary[] {
  const rows: SessionSummary[] = [];
  for (const [key, kept] of keptUnlistedThreads) {
    if ((kept.summary.host ?? LOCAL_HOST) !== host) continue;
    if (listed.has(splitThreadKey(key).id) || !keepDue(key, kept)) continue;
    rows.push(kept.summary);
  }
  return rows;
}

/**
 * Forget the kept rows for `host` that a list now names, and ones whose keep expired. Called with
 * the engine's own `session/list` (or this computer's), never with a cache a client wrote to.
 */
export function settleKeptUnlisted(host: string, listed: ReadonlySet<string>): void {
  for (const [key, kept] of keptUnlistedThreads) {
    if ((kept.summary.host ?? LOCAL_HOST) !== host) continue;
    if (listed.has(splitThreadKey(key).id) || !keepDue(key, kept)) {
      keptUnlistedThreads.delete(key);
    }
  }
}

/**
 * Threads trashed a moment ago, by key. A `session/list` that was already in flight when the
 * deletion arrived still names the row, and a machine whose link was down never heard of it: the
 * list merges (`refreshSessions`, `mergeMachineLists`, `machines.ts` `readThreads`) skip these
 * until the tombstone expires, by which time every list has caught up.
 */
const deletedThreads = new Map<string, number>();

/** How long a trashed thread's row stays refused by the list merges. */
const TOMBSTONE_MS = 2 * 60_000;

/** Remember that `key` was trashed, so no list read brings its row back. */
export function tombstoneThread(key: string): void {
  deletedThreads.set(key, Date.now());
  keptUnlistedThreads.delete(key);
}

/** Forget a tombstone for `key`, e.g. when a moved thread returns to a machine it recently left. */
export function untombstoneThread(key: string): void {
  deletedThreads.delete(key);
}

/** Whether `key` was trashed a moment ago; an expired tombstone is dropped here. */
export function isTombstoned(key: string): boolean {
  const at = deletedThreads.get(key);
  if (at === undefined) return false;
  if (Date.now() - at <= TOMBSTONE_MS) return true;
  deletedThreads.delete(key);
  return false;
}

/**
 * Announcements (`_rustcode/threads/created`) that arrived while this window had a `session/new`
 * out. The engine tells the creating client before it answers, so the row an announcement adds
 * would sit beside the pending-setup row `createSession` holds back, or stay when the creation
 * fails afterwards. They wait here: the creation's own thread is dropped from them, and the others
 * (a thread another client made meanwhile) are applied once no creation is out.
 */
let creatingSessions = 0;

/** A session creation started (`createSession`): announcements wait until it answers. */
export function beginCreatingSession(): void {
  creatingSessions++;
}

/** A session creation answered or failed. */
export function endCreatingSession(): void {
  creatingSessions--;
}
const deferredAnnouncements: { key: string; apply: () => void }[] = [];

/**
 * Hold an announcement for `key` back while a creation is out. True means it was held and `apply`
 * runs later (or never, for the creation's own thread).
 */
export function deferAnnouncement(key: string, apply: () => void): boolean {
  if (creatingSessions === 0) return false;
  deferredAnnouncements.push({ key, apply });
  return true;
}

/** A creation ended: drop the announcements of its own thread, apply the rest when none is out. */
export function releaseAnnouncements(own: string | undefined): void {
  if (own) {
    for (let i = deferredAnnouncements.length - 1; i >= 0; i--) {
      if (deferredAnnouncements[i]?.key === own) deferredAnnouncements.splice(i, 1);
    }
  }
  if (creatingSessions > 0) return;
  for (const { apply } of deferredAnnouncements.splice(0)) apply();
}

/** Forgets every kept, deleted and held-back thread, for the store's reset between tests. */
export function resetThreadListGuards(): void {
  keptUnlistedThreads.clear();
  deletedThreads.clear();
  deferredAnnouncements.length = 0;
  creatingSessions = 0;
}
