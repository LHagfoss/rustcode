// Run state drift (issue 3652). The engine's thread list is the truth about what runs; the window
// keeps its own `running` flag per open thread, fed by a stream of updates. A window that missed
// the end of a turn (a sleep, a dropped connection, a reload) would show Working for good, and one
// that missed a start (another client, a wake) would show Done while the thread works. Each list
// read is compared with the flags here; a disagreement that holds across two reads, past a grace
// period for a turn that is only starting, is settled in the engine's favour.

import type { ThreadStatus } from "../acp/v2";

/** How long a turn may have run before a list that calls its thread idle is believed. */
export const DRIFT_GRACE_MS = 10_000;

/** Consecutive list reads that must disagree with the window before it gives way. */
export const DRIFT_READS = 2;

/** The gap between list reads while something runs or is suspected to. */
export const POLL_ACTIVE_MS = 5_000;

/** The gap between list reads when everything is idle: another client may still start a turn. */
export const POLL_IDLE_MS = 30_000;

/** How long a stop may wait for the turn to end before it is forced. */
export const FORCE_STOP_MS = 10_000;

/** What the window believes about one thread it holds. */
export interface LocalRun {
  id: string;
  running: boolean;
  /** When the running turn began; unknown reads as old enough. */
  startedAt?: number;
}

/** The disagreements seen so far, by thread: how many reads in a row each has lasted. */
export interface DriftReads {
  /** The window says running, the engine says it is not. */
  stale: Record<string, number>;
  /** The engine says running, the window says it is not. */
  missed: Record<string, number>;
}

export const noDrift: DriftReads = { stale: {}, missed: {} };

export interface DriftResult {
  /** Threads whose turn ended in the engine: the window's running flag is dropped. */
  settle: string[];
  /** Threads the engine runs and the window does not know of: shown as running until the engine says otherwise. */
  adopt: string[];
  reads: DriftReads;
}

/** A status that means the thread is doing something now. */
export function engineBusy(status: ThreadStatus | undefined): boolean {
  return status === "running" || status === "needs_input";
}

/**
 * Compares the window's flags with one read of the engine's list. `listed` holds only threads whose
 * status the engine named: an older engine that names none can neither settle nor adopt anything.
 * A thread the list no longer has loses its count, so a stale count never carries over to a thread
 * that came back.
 */
export function reconcileRuns(
  local: readonly LocalRun[],
  listed: readonly { id: string; status?: ThreadStatus }[],
  reads: DriftReads,
  now: number,
): DriftResult {
  const known = new Map(local.map((x) => [x.id, x]));
  const next: DriftReads = { stale: {}, missed: {} };
  const settle: string[] = [];
  const adopt: string[] = [];
  for (const entry of listed) {
    if (entry.status === undefined) continue;
    const mine = known.get(entry.id);
    const busy = engineBusy(entry.status);
    if (mine?.running && !busy) {
      const age = mine.startedAt === undefined ? Infinity : now - mine.startedAt;
      if (age < DRIFT_GRACE_MS) continue;
      const count = (reads.stale[entry.id] ?? 0) + 1;
      if (count >= DRIFT_READS) settle.push(entry.id);
      else next.stale[entry.id] = count;
    } else if (!mine?.running && busy) {
      const count = (reads.missed[entry.id] ?? 0) + 1;
      next.missed[entry.id] = Math.min(count, DRIFT_READS);
      if (count >= DRIFT_READS) adopt.push(entry.id);
    }
  }
  return { settle, adopt, reads: next };
}

/** How long to wait before the next list read: quick while anything runs or is in doubt. */
export function pollDelay(active: boolean): number {
  return active ? POLL_ACTIVE_MS : POLL_IDLE_MS;
}

/** Whether a stop asked at `askedAt` has waited long enough to be forced. */
export function stopOverdue(askedAt: number, now: number): boolean {
  return now - askedAt >= FORCE_STOP_MS;
}
