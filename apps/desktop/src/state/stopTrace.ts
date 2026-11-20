// A Stop press traced from intent to outcome (issue 4086). `session/cancel` is a notification, so
// when Stop seemed to do nothing nobody could say where the press went: whether it was sent, which
// way, whether the engine took it and the harness was told, and what ended the turn. Each press is
// an attempt with an id the engine logs under too (`stop.*` operational events, acp/stop.rs), and
// every stage goes to Settings › Diagnostics with its time and a bounded reason code. No prompt
// text: thread id, attempt id, stage and reason only.

import { create } from "zustand";

/** What happened to one Stop, in order. */
export type StopStage =
  /** The user pressed Stop on a running turn. */
  | "intent"
  /** Pressed again while this attempt was still pending: nothing more was sent. */
  | "coalesced"
  /** Nothing was sent (`reason`: `no_connection`, `not_running`). */
  | "suppressed"
  /** Sent to the engine (`reason`: `local`, `remote`). */
  | "dispatched"
  /** The engine answered (`reason`: `harness`, `builtin`, `none`, `idle`, `legacy`). */
  | "acknowledged"
  /** Sending failed (`reason`: `dropped`, `closed`, `rejected`, `write_failed`). */
  | "failed"
  /** No answer in time (`reason`: `no_ack`). */
  | "timed_out"
  /** The turn ignored it and the thread was closed in the engine. */
  | "forced"
  /** The turn ended (`reason`: how). */
  | "ended";

export interface StopEvent {
  stage: StopStage;
  /** Milliseconds since the epoch. */
  at: number;
  reason?: string;
}

export interface StopAttempt {
  id: string;
  threadId: string;
  /** The running turn's start, which tells this turn from the next. */
  turnStartedAt?: number;
  events: StopEvent[];
}

/** The attempts remembered, newest last: enough to read a session's Stops back. */
export const STOP_TRACE_LIMIT = 50;

/** How long the engine has to answer a Stop before the attempt says it went unanswered. */
export const STOP_ACK_MS = 5_000;

let ackMs = STOP_ACK_MS;

/** How long this window waits for the engine to answer a Stop. */
export function stopAckMs(): number {
  return ackMs;
}

/** A shorter wait for a test of a Stop nobody answers. */
export function setStopAckMsForTests(ms: number): void {
  ackMs = ms;
}

/** The threads with a Stop in flight, by attempt id: what draws "Stopping…". */
export const useStops = create<{ pending: Record<string, string> }>(() => ({ pending: {} }));

const attempts: StopAttempt[] = [];
let sequence = 0;
let sink: (line: string) => void = () => {};

/** Where trace lines go (the store's diagnostics log); tests read `stopTrace` instead. */
export function setStopSink(next: (line: string) => void): void {
  sink = next;
}

/** A short id, unique in this window, that the engine accepts (`[A-Za-z0-9_-]`, ≤ 64). */
function nextId(): string {
  sequence += 1;
  return `s${Date.now().toString(36)}-${sequence.toString(36)}`;
}

function record(attempt: StopAttempt, stage: StopStage, reason?: string): void {
  const event: StopEvent = { stage, at: Date.now(), ...(reason ? { reason } : {}) };
  attempt.events.push(event);
  sink(
    `desktop: stop ${attempt.id} thread=${attempt.threadId} ${stage}${reason ? ` reason=${reason}` : ""} at=${new Date(event.at).toISOString()}`,
  );
}

function pendingAttempt(threadId: string): StopAttempt | undefined {
  const id = useStops.getState().pending[threadId];
  return id ? attempts.find((a) => a.id === id) : undefined;
}

/**
 * A new attempt for a Stop press, or null when one for the same turn is still pending: that press
 * is recorded on the pending attempt as `coalesced` and nothing more is sent.
 */
export function beginStop(threadId: string, turnStartedAt: number | undefined): StopAttempt | null {
  const pending = pendingAttempt(threadId);
  if (pending && pending.turnStartedAt === turnStartedAt) {
    record(pending, "coalesced");
    return null;
  }
  // A pending Stop for an earlier turn is over: that turn ended without the trace seeing it.
  if (pending) settleStop(threadId, "ended", "turn_changed");
  const attempt: StopAttempt = { id: nextId(), threadId, turnStartedAt, events: [] };
  attempts.push(attempt);
  if (attempts.length > STOP_TRACE_LIMIT) attempts.splice(0, attempts.length - STOP_TRACE_LIMIT);
  record(attempt, "intent");
  useStops.setState((s) => ({ pending: { ...s.pending, [threadId]: attempt.id } }));
  return attempt;
}

/** A stage on the way that leaves the attempt pending. */
export function markStop(attempt: StopAttempt, stage: StopStage, reason?: string): void {
  record(attempt, stage, reason);
}

/** The attempt's last stage: the Stop is no longer pending, and "Stopping…" goes. */
export function finishStop(attempt: StopAttempt, stage: StopStage, reason?: string): void {
  record(attempt, stage, reason);
  if (useStops.getState().pending[attempt.threadId] !== attempt.id) return;
  useStops.setState((s) => {
    const { [attempt.threadId]: _done, ...rest } = s.pending;
    return { pending: rest };
  });
}

/** Ends the thread's pending Stop, if it has one: the turn ended (or was forced) however it did. */
export function settleStop(threadId: string, stage: StopStage, reason?: string): void {
  const pending = pendingAttempt(threadId);
  if (pending) finishStop(pending, stage, reason);
}

/** Whether the thread has a Stop in flight. */
export function stopPending(threadId: string | null | undefined): boolean {
  return !!threadId && !!useStops.getState().pending[threadId];
}

/** The remembered attempts, oldest first; one thread's when `threadId` is given. */
export function stopTrace(threadId?: string): readonly StopAttempt[] {
  return threadId ? attempts.filter((a) => a.threadId === threadId) : [...attempts];
}

/** The last stage an attempt reached. */
export function lastStage(attempt: StopAttempt): StopEvent | undefined {
  return attempt.events.at(-1);
}

/** Forget every attempt (tests, a fresh window). */
export function resetStops(): void {
  attempts.length = 0;
  ackMs = STOP_ACK_MS;
  useStops.setState({ pending: {} });
}
