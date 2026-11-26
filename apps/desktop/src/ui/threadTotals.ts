import type { Billing, Harness, SessionUsage } from "../acp/types";
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

/** How a thread's login is billed: its named account's, else the harness's default login's. */
export function threadBilling(harness: Harness | undefined, account: string | undefined): Billing {
  if (!harness) return "unknown";
  if (account) return harness.accounts?.find((a) => a.id === account)?.billing ?? "unknown";
  return harness.billing ?? "unknown";
}

/** The Cost total's label, value and tip. On a subscription the money is what the tokens would cost at API prices, not a bill. */
export function costTotal(
  totals: Pick<ThreadTotals, "usd" | "estimated">,
  billing: Billing,
  format: (usd: number) => string,
): { label: string; value: string; tip: string } {
  const subscription = billing === "subscription";
  const label = subscription ? "API value" : "Cost";
  if (totals.usd === undefined) return { label, value: "–", tip: "No cost reported or priced yet" };
  const value = `${totals.estimated ? "~" : ""}${format(totals.usd)}`;
  if (subscription) {
    const tip = "What these tokens would cost at API prices. Your subscription is billed separately.";
    return {
      label,
      value,
      tip: totals.estimated ? `${tip} Partly estimated from tokens at list price.` : tip,
    };
  }
  return {
    label,
    value,
    tip: totals.estimated ? "Partly estimated from tokens at list price" : "As the harness reported it",
  };
}

/** How full the context window is, as the meter's colour: calm, then a warning, then danger. */
export function contextTone(percent: number): "accent" | "warning" | "danger" {
  return percent >= 90 ? "danger" : percent >= 70 ? "warning" : "accent";
}
