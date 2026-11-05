// A thread the app has sent but the engine has not made yet: `session/new` is checking out a
// worktree and starting a harness, which can take a minute. The row and the chat show it while
// that runs, and it becomes the real thread in one update when the session exists. Pure, so the
// store and its tests share one answer (store.ts `createThread`, ui/Sidebar.tsx, ui/ChatView.tsx).

import type { SessionSummary } from "../acp/types";

/** The worktree `session/new` was asked to make. */
export interface SetupWorktree {
  repo: string;
  branch?: string;
  title?: string;
}

export interface PendingSetup {
  /** A client id until the session exists: never persisted, never sent to the engine. */
  id: string;
  /** The message waiting to be sent once the thread exists. */
  text: string;
  /** What the row calls it: the message's title. */
  title: string;
  /** Where the thread starts, before the worktree exists. */
  cwd: string;
  worktree: SetupWorktree;
  /** When the send happened: the row and the chat tick from here. */
  startedAt: number;
  /** Set when `session/new` refused or failed: the row and the chat keep it until dismissed. */
  failure?: { text: string; at: number };
}

/** The setup to start: the store fills in the id and the clock. */
export type NewSetup = Omit<PendingSetup, "startedAt" | "failure">;

/** Whether the setup still runs: a failed one waits for a retry or a dismissal. */
export function setupInProgress(pending: PendingSetup | null): boolean {
  return !!pending && !pending.failure;
}

/**
 * Starts one setup, or nothing: a second send while the first runs must not make a second
 * worktree. A failed setup is replaced, which is the retry.
 */
export function beginSetup(
  current: PendingSetup | null,
  next: NewSetup,
  startedAt = Date.now(),
): PendingSetup | null {
  if (setupInProgress(current)) return null;
  return { ...next, startedAt };
}

/** The setup that failed: the message and its worktree stay, for a retry or a dismissal. */
export function setupFailed(pending: PendingSetup, error: string, at = Date.now()): PendingSetup {
  return { ...pending, failure: { text: error, at } };
}

/**
 * The setup that finished, or the one that is still going. Only the setup the session belongs
 * to goes: a newer one (the failed row retried) keeps its place.
 */
export function setupSucceeded(
  pending: PendingSetup | null,
  id: string | null | undefined,
): PendingSetup | null {
  return pending && pending.id === id ? null : pending;
}

/** Gives up a setup: its row and its error go, and nothing is sent. */
export function dismissSetup(): null {
  return null;
}

/** The row a setup takes in the list, in the shape the list places rows by. */
export function pendingSummary(pending: PendingSetup): SessionSummary {
  return {
    id: pending.id,
    cwd: pending.cwd,
    title: pending.title,
    updatedAt: pending.startedAt,
    // Its repository places it under the right project, as the real thread will be.
    worktree: {
      path: pending.cwd,
      branch: pending.worktree.branch ?? "",
      repo: pending.worktree.repo,
    },
  };
}
