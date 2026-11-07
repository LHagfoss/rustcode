// What Settings › Statistics puts on a row's cost: the USD a harness reported,
// or, where it reported none, an estimate at API list prices. Harnesses
// disagree about what `input_tokens` holds, so a raw
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
// A harness does not always report what a turn cost: Codex never does, so its
// rows would otherwise show no cost at all. A row's cost here is the USD the
// harness reported when it reported one, and otherwise the estimate below; a
// reported number always wins and is never altered. The page's headline total
// adds a row's reported cost or its estimate, never both for the same row, and
// keeps the reported and estimated parts visible beside it. A row with neither
// is unknown, never $0: a missing price and a free model must not look alike.
//
// This is an estimate, not a bill, and prices are public list prices per
// million tokens, written by hand below on PRICE_DATE; there is no fetch at
// runtime, so a price change is a one-line edit here (and a new PRICE_DATE). A
// model the table does not know yields no estimate.
//
// Pure (no React, no store), so the page and its tests share one answer.

import type { UsageCounts } from "../acp/rustcode";

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

/** Where a row's cost came from: the harness's own number, a list price, or neither. */
export type CostSource = "reported" | "estimated" | "unknown";

/** A row's best known cost, and which number it is. */
export interface BestCost {
  /** The reported cost, else the estimate; absent when the row has neither. */
  usd?: number;
  source: CostSource;
}

/** A row as the cost functions need it: its counts, its reported cost and its turns. */
export interface CostInput {
  harness: string;
  model?: string;
  totals: UsageCounts & {
    /** The USD the harness reported. Never a price RustCode computed. */
    cost?: number;
    turns: number;
    /** Turns that reported no cost, when the row knows it (a usage total does). */
    turnsWithoutCost?: number;
  };
}

/**
 * A row's best known cost: the USD its harness reported when it reported one,
 * otherwise its list-price estimate, otherwise nothing. A reported number is
 * never corrected or replaced by an estimate.
 */
export function bestCost(row: CostInput): BestCost {
  if (row.totals.cost !== undefined) return { usd: row.totals.cost, source: "reported" };
  const usd = estimateUsd(row.harness, row.model, row.totals);
  if (usd === undefined) return { source: "unknown" };
  return { usd, source: "estimated" };
}

/** What a set of rows costs: each row's best known number, split by where it came from. */
export interface CostTotals {
  /** The sum of each row's best known cost; absent when no row has one. */
  usd?: number;
  /** The reported part of `usd`. */
  reportedUsd?: number;
  /** The estimated part of `usd`. */
  estimatedUsd?: number;
  /** Turns in rows with neither a reported cost nor a price, which `usd` leaves out. */
  unknownTurns: number;
}

/** No rows yet: nothing known, and nothing left out. */
export function emptyCost(): CostTotals {
  return { unknownTurns: 0 };
}

const addOptional = (one?: number, other?: number): number | undefined =>
  one === undefined && other === undefined ? undefined : (one ?? 0) + (other ?? 0);

/** Two costs added, each part only when at least one side has it. */
export function addCost(one: CostTotals, other: CostTotals): CostTotals {
  return {
    usd: addOptional(one.usd, other.usd),
    reportedUsd: addOptional(one.reportedUsd, other.reportedUsd),
    estimatedUsd: addOptional(one.estimatedUsd, other.estimatedUsd),
    unknownTurns: one.unknownTurns + other.unknownTurns,
  };
}

/** One row's best known cost as a roll-up, so rows and sums share a shape. */
export function costOf(row: CostInput): CostTotals {
  const best = bestCost(row);
  if (best.source === "reported") {
    return { usd: best.usd, reportedUsd: best.usd, unknownTurns: 0 };
  }
  if (best.source === "estimated") {
    return { usd: best.usd, estimatedUsd: best.usd, unknownTurns: 0 };
  }
  return { unknownTurns: row.totals.turns };
}

/**
 * A report's cost, model row by model row: each row contributes its reported
 * cost or its estimate, never both, and the turns of rows with neither are
 * counted instead of being shown as $0.
 */
export function rollUpCosts(rows: readonly CostInput[]): CostTotals {
  let total = emptyCost();
  for (const row of rows) total = addCost(total, costOf(row));
  return total;
}
