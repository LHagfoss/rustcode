// What a delegated thread is doing, as the thread above it reads it: a state
// for the stack of marks on its row (ui/AgentStack.tsx) and for the names its
// tooltip lists. Pure, so the sidebar and its tests share one answer.

import type { ThreadStatus } from "../acp/v2";
import { isActive, type LinkState } from "./orchestration";

/** What a child is doing, as the sidebar says it. */
export type ChildState = "needs" | "working" | "finished" | "failed";

export interface ChildInfo {
  id: string;
  state: ChildState;
}

/**
 * A thread's state as its parent sees it: a thread waiting on you or working
 * is live, and anything else is finished as far as the parent is concerned
 * (`review` and `idle` children are not running either).
 */
export function childState(status: ThreadStatus, failed = false): ChildState {
  if (status === "needs_input") return "needs";
  if (status === "running") return "working";
  return failed ? "failed" : "finished";
}

/**
 * A child as its parent's stack reads it. A live link beats the thread's own
 * status, because a child that was just started or messaged has not reported
 * `running` yet.
 */
export function childInfo(
  id: string,
  read: { status?: ThreadStatus; link?: LinkState; failed?: boolean },
): ChildInfo {
  const live =
    read.link && isActive(read.link)
      ? read.link === "needs_user"
        ? "needs_input"
        : "running"
      : (read.status ?? "idle");
  return { id, state: childState(live, read.failed) };
}
