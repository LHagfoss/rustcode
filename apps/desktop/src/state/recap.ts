// The while-you-were-away recap: what the threads did while the app was idle for
// hours, in three groups — what needs you, what failed, and what finished — so a
// return after lunch is one card rather than a hunt through the list. Pure, so the
// card, its settings and its tests agree on when a recap is due and what it says.

import type { SessionSummary } from "../acp/types";
import type { InboxItem } from "./inbox";

/** The default idle time before a return earns a recap (Settings › Notifications). */
export const RECAP_HOURS_DEFAULT = 4;

/** The idle times the setting offers. */
export const RECAP_HOUR_OPTIONS = [1, 4, 8, 24] as const;

const HOUR = 3_600_000;

/** A stored recap threshold, kept to one of {@link RECAP_HOUR_OPTIONS}. */
export function parseRecapHours(value: string | null): number {
  const n = value === null ? Number.NaN : Number.parseInt(value, 10);
  return (RECAP_HOUR_OPTIONS as readonly number[]).includes(n) ? n : RECAP_HOURS_DEFAULT;
}

/**
 * Whether the gap since the app was last used is long enough to have missed
 * something. A first run has no last time and never earns a recap.
 */
export function awayDue(lastActive: number | undefined, now: number, hours: number): boolean {
  if (!lastActive) return false;
  return now - lastActive >= hours * HOUR;
}

/** One thread in the failed or done group. */
export interface RecapThread {
  threadId: string;
  title: string;
  project: string;
  at: number;
}

/** What the recap card says. */
export interface Recap {
  /** When the user left, for "while you were away". */
  since: number;
  /** What still needs you, newest first. */
  needs: InboxItem[];
  /** Turns that failed since then, newest first. */
  failed: RecapThread[];
  /** Threads that finished since then, newest first. */
  done: RecapThread[];
}

/** What {@link recapOf} reads. */
export interface RecapRead {
  /** The inbox's current rows: what needs the user now. */
  items: readonly InboxItem[];
  /** Threads whose turn finished, with when (state/store.ts `finished`). */
  finished: Readonly<Record<string, number>>;
  /** The thread list, for titles and folders. */
  sessions: readonly SessionSummary[];
  /** Titles of loaded threads, which are fresher than the list's. */
  threads: Readonly<Record<string, { title?: string }>>;
  /** When the user left. */
  since: number;
}

function titleOf(read: RecapRead, id: string): string {
  return read.threads[id]?.title ?? read.sessions.find((s) => s.id === id)?.title ?? "Untitled thread";
}

function projectOf(read: RecapRead, id: string): string {
  const session = read.sessions.find((s) => s.id === id);
  return session?.worktree?.repo ?? session?.cwd ?? "";
}

function sinceGroup(read: RecapRead, at: Readonly<Record<string, number>>, from: number): RecapThread[] {
  return Object.entries(at)
    .filter(([, when]) => when >= from)
    .map(([threadId, when]) => ({
      threadId,
      title: titleOf(read, threadId),
      project: projectOf(read, threadId),
      at: when,
    }))
    .sort((a, b) => b.at - a.at);
}

/**
 * The card's three groups. "Needs you" is what waits right now (a request that
 * waited since before the break still needs you); failed and done are the events
 * since the user left, because those are what the away time produced.
 */
export function recapOf(read: RecapRead): Recap {
  return {
    since: read.since,
    // Still waiting, even when it waited before the break: it blocks the thread now.
    needs: read.items.filter((item) => item.kind !== "failed"),
    failed: read.items
      .filter((item) => item.kind === "failed" && item.at >= read.since)
      .map((item) => ({
        threadId: item.threadId,
        title: titleOf(read, item.threadId) || item.title,
        project: item.project,
        at: item.at,
      }))
      .sort((a, b) => b.at - a.at),
    done: sinceGroup(read, read.finished, read.since),
  };
}

/** Whether there is anything to recap. */
export function recapEmpty(recap: Recap): boolean {
  return !recap.needs.length && !recap.failed.length && !recap.done.length;
}
