// Which threads a restart cut off are continued, and which are left stopped. Pure, so the store and
// its tests share one answer (`state/store.ts` `resumeInterrupted`).
//
// A thread continues once per interruption: one the user stopped stays stopped, and one whose resume
// turn was itself cut off again within {@link RESUME_LOOP_MS} (a crash on startup, an update loop) is
// left for the user instead of being restarted in a loop.

/** At most this many threads continue at once after a restart. */
export const RESUME_MAX = 8;

/** A thread resumed this recently and interrupted again is not resumed a second time. */
export const RESUME_LOOP_MS = 10 * 60_000;

/** The first line of the resume prompt: the transcript shows only this as a divider (`wakeLine`). */
export const RESUME_NOTICE = "Resumed after restart";

export type ResumeVerdict = "resume" | "unknown" | "stopped" | "again";

export interface ResumeBook {
  /** The threads the user stopped and the app has not run a turn on since. */
  stopped: readonly string[];
  /** When the app last resumed a thread after a restart, by thread id (ms). */
  resumedAt: Readonly<Record<string, number>>;
}

/** Whether one interrupted thread continues, and the reason when it does not. */
export function resumeVerdict(
  id: string,
  known: ReadonlySet<string>,
  book: ResumeBook,
  now: number,
): ResumeVerdict {
  if (!known.has(id)) return "unknown";
  if (book.stopped.includes(id)) return "stopped";
  const last = book.resumedAt[id];
  if (last !== undefined && now - last >= 0 && now - last < RESUME_LOOP_MS) return "again";
  return "resume";
}

/** The resume record without the entries old enough that they no longer guard anything. */
export function pruneResumed(
  resumedAt: Readonly<Record<string, number>>,
  now: number,
): Record<string, number> {
  return Object.fromEntries(Object.entries(resumedAt).filter(([, at]) => now - at < RESUME_LOOP_MS));
}

/** `id` added to the stopped list once. */
export function markStopped(stopped: readonly string[], id: string): string[] {
  return stopped.includes(id) ? [...stopped] : [...stopped, id];
}
