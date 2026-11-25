import type { SessionUsage } from "../acp/types";
import { estimateUsd } from "../state/pricing";
import { logTotals, type TurnEntry } from "./overview";

/** The thread's headline numbers in the side panel: what it cost, how long it worked, its tokens. */
export interface ThreadTotals {
  usd?: number;
  /** Part of `usd` is the turns' tokens at list price, not what the harness reported. */
  estimated: boolean;
  workedMs: number;
  tokens?: number;
}

/** A turn's cost: what the harness reported, else its tokens at list price. */
export function turnCost(
  turn: TurnEntry,
  harness: string | undefined,
  model: string | undefined,
): { usd: number; estimated: boolean } | undefined {
  if (turn.cost !== undefined) return { usd: turn.cost, estimated: false };
  const counts = turn.counts;
  if (!harness || !counts) return undefined;
  const usd = estimateUsd(harness, model, {
    input: counts.inputTokens,
    output: counts.outputTokens,
    cached: counts.cachedTokens,
  });
  return usd === undefined ? undefined : { usd, estimated: true };
}

/**
 * The thread's own reported cost when it has one in dollars, else the sum of its turns' costs.
 * `liveMs` is how long the turn still running has gone so far, so the clock moves while it works.
 */
export function threadTotals(
  turns: readonly TurnEntry[],
  usage: SessionUsage | undefined,
  harness: string | undefined,
  model: string | undefined,
  liveMs = 0,
): ThreadTotals {
  const { tokens, agentMs } = logTotals(turns, usage);
  const workedMs = agentMs + liveMs;
  if (usage?.cost !== undefined && (!usage.currency || usage.currency === "USD"))
    return { usd: usage.cost, estimated: false, workedMs, tokens };
  let usd: number | undefined;
  let estimated = false;
  for (const turn of turns) {
    const cost = turnCost(turn, harness, model);
    if (!cost) continue;
    usd = (usd ?? 0) + cost.usd;
    estimated ||= cost.estimated;
  }
  return { usd, estimated, workedMs, tokens };
}

/** How full the context window is, as the meter's colour: calm, then a warning, then danger. */
export function contextTone(percent: number): "accent" | "warning" | "danger" {
  return percent >= 90 ? "danger" : percent >= 70 ? "warning" : "accent";
}
