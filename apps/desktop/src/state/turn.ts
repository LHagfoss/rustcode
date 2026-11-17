// The turn a thread runs, as one value: `ThreadState.turn` is a `ThreadTurn` while a turn runs and
// absent while the thread is idle, so nothing a turn owns (its kind, clock, usage samples and child
// processes) can outlive it. The transitions that start and end one are in `thread.ts`
// (`beginTurn`, `beginQuietTurn`, `wakeTurn`, `finishTurn`, `endTurn`).

import type { TurnUsage } from "./thread";

/** What the engine last saw of the processes a harness started, from `_rustcode/session/childActivity`. */
export interface ChildActivity {
  /** How many are running. */
  processes: number;
  /** CPU time they have used in total, in milliseconds. */
  cpuMs: number;
  /** The one that used the most (`rustc`). */
  busiest?: string;
  /** When they last added CPU, in this app's clock; absent while they have not. */
  activeAt?: number;
}

/**
 * Who started the running turn:
 * - `user`: a prompt the app sent (the user's, a queued one or a wake message);
 * - `woken`: the engine resumed the turn itself, when the background commands it parked on finished;
 * - `quiet`: the app's own turn (an automatic compaction). It writes no rows, so the answer the user
 *   has not read stays the last thing in the transcript and one notice below it says what the app
 *   did (`applyUpdate`).
 */
export type TurnKind = "user" | "woken" | "quiet";

/** A running turn: `ThreadState.turn`, absent while the thread is idle. */
export interface ThreadTurn {
  kind: TurnKind;
  /** When the app saw it begin, for the elapsed clock and the footer's duration. */
  startedAt: number;
  /** Its usage samples, handed to its footer when it ends. */
  usage?: TurnUsage;
  /**
   * The processes its harness started (a build, a test run), as the engine last sampled them. A
   * tool that prints nothing for minutes still counts as progress while they use CPU
   * (`orchestration.ts` `stallNotice`).
   */
  children?: ChildActivity;
}

/** Whether a turn runs on the thread. */
export function isRunning<T extends { turn?: ThreadTurn }>(
  thread: T | undefined,
): thread is T & { turn: ThreadTurn } {
  return thread?.turn !== undefined;
}

/** Whether the running turn is one the engine resumed itself, not a prompt the app sent. */
export const isWoken = (thread: { turn?: ThreadTurn } | undefined): boolean => thread?.turn?.kind === "woken";

/** Whether the running turn is the app's own (an automatic compaction). */
export const isQuiet = (thread: { turn?: ThreadTurn } | undefined): boolean => thread?.turn?.kind === "quiet";

/**
 * Whether the queue waits for Resume. Only an idle thread holds it: a turn that runs is already
 * taking from the queue, whatever hold an earlier stop left.
 */
export const queueHeld = (thread: { turn?: ThreadTurn; queueHeld?: boolean }): boolean =>
  !!thread.queueHeld && thread.turn === undefined;
