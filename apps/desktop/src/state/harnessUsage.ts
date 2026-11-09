// The plan usage windows an agent reads in `list_harnesses`
// (docs/orchestrator.md): how much of each login's window is used, so an
// orchestrator can pick an account with room left before it delegates.
//
// The numbers are the ones the app already read for Settings › Usage and the
// composer's meter. A tool call never starts a Keychain read or an Anthropic
// request of its own: the poll (`state/store.ts` `startPlanPolling`) keeps the
// cache current, and `as_of` says how old the answer is. A window whose reset
// passed since the read is folded as in the composer, so an agent is
// never told an account is busy on a window that has already started over.

import { type CachedPlan, shownPlan } from "./store";

/**
 * One plan window as an agent reads it. A type alias, not an interface, so the
 * answer's `Json` shape takes it without an index signature.
 */
export type AgentWindow = {
  /** The window's name: `5-hour`, `Weekly`, `Weekly · Opus`. */
  label: string;
  used_percent: number;
  window_minutes?: number;
  /** ISO 8601, when it resets. */
  resets_at?: string;
};

/** A login's plan usage as `list_harnesses` reports it. */
export type AgentUsage = {
  status: "ok";
  /** ISO 8601, when these numbers were read; for recorded ones, when that read was. */
  as_of?: string;
  windows: AgentWindow[];
};

/**
 * A window's name for numbers the cache kept without one: a thread's own
 * `_rustcode/usage/limits` answer has lengths only. Kept in step with the
 * engine's `window_label` (`engine/src/acp/usage.rs`).
 */
export function agentWindowLabel(minutes: number | undefined): string {
  switch (minutes) {
    case 300:
      return "5-hour";
    case 10_080:
      return "Weekly";
    case 1_440:
      return "Daily";
    default:
      return "Usage";
  }
}

const iso = (ms: number | undefined): string | undefined =>
  ms === undefined ? undefined : new Date(ms).toISOString();

/**
 * One login's usage block, or undefined when nothing was read for it (and it
 * has not yet been read).
 */
export function usageOf(plan: CachedPlan | undefined, now = Date.now()): AgentUsage | undefined {
  // A window whose reset passed is nothing used and no reset time, the way the
  // composer's meter draws it (`shownPlan`, then `planNow` in `state/store.ts`):
  // the numbers were read before the window started over.
  const live = shownPlan(undefined, plan, now).plan;
  if (!live?.windows.length) return undefined;
  // Numbers recorded before the login's access token lapsed carry when they
  // were read; a read made now carries when the app got it. Folding a reset
  // changes no window's times, so the entry's own are still right.
  const as_of = iso(plan?.asOf ?? plan?.fetchedAt);
  return {
    status: "ok",
    ...(as_of ? { as_of } : {}),
    windows: live.windows.map((w) => ({
      label: w.label ?? agentWindowLabel(w.minutes),
      used_percent: w.usedPercent,
      ...(w.minutes !== undefined ? { window_minutes: w.minutes } : {}),
      ...(w.resetsAt !== undefined ? { resets_at: iso(w.resetsAt) } : {}),
    })),
  };
}
