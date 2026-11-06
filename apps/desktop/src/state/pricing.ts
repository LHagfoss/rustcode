// What Settings › Statistics estimates a row would have cost at API list
// prices. Harnesses disagree about what `input_tokens` holds, so a raw
// `input × rate + output × rate` overcounts cache reads (most of an agent's
// input): see the engine, where the built-in harness and Codex report cached
// input inside the prompt (`network/anthropic_messages.rs:137` adds
// `input + cached + cache_write`; `harness/codex.rs:817` takes Codex's
// `total.inputTokens` with `cachedInputTokens` a part of it), while Claude
// Code's and Pi's own logs keep cache reads and writes in fields of their own
// (`harness_usage.rs:271` and `:283`). Each harness is normalized to
// `{uncachedInput, cacheRead, cacheWrite, output}` before any rate is applied,
// and a harness whose convention is unknown yields no estimate at all.
//
// This is an estimate, kept apart from the cost a harness reported: it never
// replaces, corrects or is added to that number. Prices are public list prices
// per million tokens, written by hand below on PRICE_DATE; there is no fetch at
// runtime, so a price change is a one-line edit here (and a new PRICE_DATE). A
// model the table does not know yields no estimate, never a zero: a missing
// price and a free model would otherwise look the same.
//
// Pure (no React, no store), so the page and its tests share one answer.

import type { UsageCounts } from "../acp/rustcode";
import type { HarnessTotal } from "./statistics";

/** The day the prices below were last checked against the providers' list prices. */
export const PRICE_DATE = "2026-11-06";

/** USD per million tokens. */
export interface Price {
  input: number;
  cacheRead: number;
  cacheWrite: number;
  output: number;
}

const price = (input: number, cacheRead: number, cacheWrite: number, output: number): Price => ({
  input,
  cacheRead,
  cacheWrite,
  output,
});

/**
 * List prices per model id, as the store records it (`turns.model`). The ids
 * marked with a harness are the aliases a harness writes instead of the full
 * model name; they are mapped below rather than priced twice.
 */
const PRICES: Record<string, Price> = {
  // Anthropic. Opus 5.5 and Sonnet 5 are what Claude Code runs on this Mac.
  "claude-opus-5-5": price(4, 0.2, 5, 20),
  "claude-sonnet-5": price(2, 0.2, 2.5, 10),
  "claude-sonnet-5.5": price(2, 0.2, 2.5, 10),
  // OpenAI, as Codex and the built-in harness run them.
  "gpt-6-luna": price(0.1, 0.01, 0.125, 0.5),
  "gpt-6.1-terra": price(1, 0.05, 1.25, 5),
  "gpt-6.1-sol": price(2, 0.1, 2.5, 10),
  "gpt-6-astra": price(10, 1, 12.5, 50),
  // DeepSeek, as Pi runs it.
  "deepseek/deepseek-flash": price(0.3, 0.006, 0, 1.2),
  "deepseek/deepseek-v4-pro": price(1.32, 0.132, 0, 3.96),
  "deepseek/deepseek-v4.1-flash": price(0.3, 0.007, 0, 1.2),
};

/**
 * A harness that records a short name for the model it ran: Claude Code's
 * `opus` and `sonnet` aliases, and the provider spelling Pi and the built-in
 * harness write.
 */
const ALIASES: Record<string, string> = {
  opus: "claude-opus-5-5",
  sonnet: "claude-sonnet-5",
  "deepseek/flash": "deepseek/deepseek-v4.1-flash",
  "chatgpt/gpt-6-luna": "gpt-6-luna",
};

/** The price of a model the table knows, or undefined (never zero) for one it does not. */
export function priceOf(model: string | undefined): Price | undefined {
  if (!model) return undefined;
  return PRICES[ALIASES[model] ?? model];
}

/**
 * Whether a harness counts cached input inside `input_tokens`.
 *
 * - `inside`: the built-in harness sends `prompt_tokens` = input + cache read +
 *   cache write (`network/anthropic_messages.rs:137`), and Codex's
 *   `total.inputTokens` has `cachedInputTokens` as a part of it
 *   (`harness/codex.rs:817`);
 * - `outside`: Claude Code's and Pi's logs keep cache reads and writes in
 *   their own fields, next to an `input_tokens` that already excludes them
 *   (`harness_usage.rs:271`, `:283`).
 *
 * A harness not listed here records turns whose meaning this page cannot
 * establish (Gemini and OpenCode report no token totals of their own), so it
 * is never priced.
 */
const CACHE_INSIDE: Record<string, boolean> = {
  rustcode: true,
  codex: true,
  "claude-code": false,
  pi: false,
};

/** The four counts a price applies to, once a harness's own convention is undone. */
export interface Normalized {
  uncachedInput: number;
  cacheRead: number;
  cacheWrite: number;
  output: number;
}

/**
 * A harness's recorded counts as uncached input, cache read, cache write and
 * output. Undefined when the harness's convention is unknown, or when the
 * counts needed to undo it are missing: `input_tokens` that may hold cached
 * input cannot be split without the cached count, and a guessed zero would be
 * a made-up number. Output is required for the same reason: charging nothing
 * for the reply understates every row that did not report it.
 *
 * Cache read and cache write are 0 where a harness keeps them outside
 * `input_tokens` and a row reported none (the tokens are then simply absent
 * from the estimate, which the row's * already says).
 */
export function normalize(harness: string, counts: UsageCounts): Normalized | undefined {
  const inside = CACHE_INSIDE[harness];
  if (inside === undefined || counts.input === undefined) return undefined;
  if (counts.output === undefined) return undefined;
  if (inside) {
    // Codex and the built-in harness never record a cache write, so it is 0
    // here; any cache-write tokens the provider counted stay in the input
    // remainder and are priced at the input rate.
    const cacheRead = counts.cached;
    if (cacheRead === undefined) return undefined;
    const cacheWrite = counts.cacheWrite ?? 0;
    return {
      uncachedInput: Math.max(0, counts.input - cacheRead - cacheWrite),
      cacheRead,
      cacheWrite,
      output: counts.output,
    };
  }
  return {
    uncachedInput: counts.input,
    cacheRead: counts.cached ?? 0,
    cacheWrite: counts.cacheWrite ?? 0,
    output: counts.output,
  };
}

/**
 * What a row's tokens would cost at list prices, in USD, or undefined when it
 * cannot be estimated: an unknown harness, an unknown model, or a count the
 * normalization needs that no turn reported. Never a raw `input_tokens`: the
 * normalized counts are the only ones priced.
 */
export function estimateUsd(
  harness: string,
  model: string | undefined,
  counts: UsageCounts,
): number | undefined {
  const price = priceOf(model);
  if (!price) return undefined;
  const tokens = normalize(harness, counts);
  if (!tokens) return undefined;
  return (
    (tokens.uncachedInput * price.input +
      tokens.cacheRead * price.cacheRead +
      tokens.cacheWrite * price.cacheWrite +
      tokens.output * price.output) /
    1_000_000
  );
}

/** A harness's estimate: the sum of its model rows, and the turns left out of it. */
export interface GroupEstimate {
  /** The USD list prices put on this group's turns, or undefined when no model row could be priced. */
  usd?: number;
  /** Turns in rows with no price or with unknown token semantics, which this estimate leaves out. */
  turnsNotEstimated: number;
}

/**
 * A harness row priced model by model, since each model has its own rate. A
 * harness whose stores an unpriced model keeps the priced part and counts the
 * rest, rather than dropping the whole row.
 */
export function estimateGroup(group: HarnessTotal): GroupEstimate {
  let usd: number | undefined;
  let turnsNotEstimated = 0;
  for (const model of group.models) {
    const one = estimateUsd(group.harness, model.model, model.totals);
    if (one === undefined) {
      turnsNotEstimated += model.totals.turns;
      continue;
    }
    usd = (usd ?? 0) + one;
  }
  return { usd, turnsNotEstimated };
}

/** Every harness row's estimate, and the total of the rows that could be priced. */
export interface ReportEstimate extends GroupEstimate {
  byHarness: Map<string, GroupEstimate>;
}

export function estimateAll(groups: HarnessTotal[]): ReportEstimate {
  const byHarness = new Map<string, GroupEstimate>();
  let usd: number | undefined;
  let turnsNotEstimated = 0;
  for (const group of groups) {
    const estimate = estimateGroup(group);
    byHarness.set(group.harness, estimate);
    if (estimate.usd !== undefined) usd = (usd ?? 0) + estimate.usd;
    turnsNotEstimated += estimate.turnsNotEstimated;
  }
  return { usd, turnsNotEstimated, byHarness };
}
