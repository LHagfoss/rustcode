// The price estimate behind Settings › Statistics: each harness's recorded
// counts normalized before a rate is applied, list prices for the models this
// machine runs, and a blank (never a zero) where either is unknown. The turn
// shapes are one row each, with the numbers the local store recorded.

import { describe, expect, test } from "bun:test";
import type { UsageCounts } from "../acp/rustcode";
import {
  addCost,
  bestCost,
  type CostInput,
  costOf,
  emptyCost,
  estimateUsd,
  normalize,
  PRICE_DATE,
  priceOf,
  rollUpCosts,
} from "./pricing";
import { emptyTotals } from "./statistics";

const counts = (over: UsageCounts): UsageCounts => ({
  input: undefined,
  output: undefined,
  cached: undefined,
  cacheWrite: undefined,
  ...over,
});

/** A `turns` row as the store recorded it: a missing count is absent. */
const codex = () => counts({ input: 491_521, cached: 424_832, output: 2_542 });
const claudeCode = () => counts({ input: 390, cached: 27_730_447, cacheWrite: 232_690, output: 88_864 });
const pi = () => counts({ input: 17_568, cached: 2_445_824, cacheWrite: 0, output: 29_857 });
const builtin = () => counts({ input: 1_000, cached: 400, output: 100 });

describe("each harness's input_tokens, normalized", () => {
  test("codex counts cached input inside input_tokens", () => {
    // harness/codex.rs:817: total.inputTokens, with cachedInputTokens a part of it.
    expect(normalize("codex", codex())).toEqual({
      uncachedInput: 66_689,
      cacheRead: 424_832,
      cacheWrite: 0,
      output: 2_542,
    });
  });

  test("the built-in harness counts cache read and cache write inside it", () => {
    // network/anthropic_messages.rs:137 adds input + cached + cache_write.
    expect(normalize("rustcode", builtin())).toEqual({
      uncachedInput: 600,
      cacheRead: 400,
      cacheWrite: 0,
      output: 100,
    });
  });

  test("claude-code reports cache read and write outside it", () => {
    // harness_usage.rs:271 reads input_tokens next to cache_read/creation.
    expect(normalize("claude-code", claudeCode())).toEqual({
      uncachedInput: 390,
      cacheRead: 27_730_447,
      cacheWrite: 232_690,
      output: 88_864,
    });
  });

  test("pi reports cache read and write outside it", () => {
    // harness_usage.rs:283 reads Pi's input next to cacheRead/cacheWrite.
    expect(normalize("pi", pi())).toEqual({
      uncachedInput: 17_568,
      cacheRead: 2_445_824,
      cacheWrite: 0,
      output: 29_857,
    });
  });

  test("a harness whose semantics are unknown yields nothing, never a guess", () => {
    expect(normalize("gemini", counts({ input: 100, cached: 50, output: 10 }))).toBeUndefined();
    expect(normalize("opencode", counts({ input: 100, output: 10 }))).toBeUndefined();
    expect(normalize("some-harness", counts({ input: 100, output: 10 }))).toBeUndefined();
  });

  test("a partial column the split needs is absent, not zero", () => {
    // No input reported: there is nothing to price.
    expect(normalize("codex", counts({ cached: 50, output: 10 }))).toBeUndefined();
    // Cached input may sit inside input_tokens, and without it the split is a guess.
    expect(normalize("codex", counts({ input: 1_000, output: 10 }))).toBeUndefined();
    // A reply no turn reported is not charged at a zero rate either.
    expect(normalize("claude-code", counts({ input: 1_000 }))).toBeUndefined();
    // Outside the input, a cache read no turn reported is simply left out.
    expect(normalize("claude-code", counts({ input: 1_000, output: 10 }))).toEqual({
      uncachedInput: 1_000,
      cacheRead: 0,
      cacheWrite: 0,
      output: 10,
    });
  });
});

describe("list prices", () => {
  test("the models this machine runs are priced, aliases included", () => {
    expect(priceOf("gpt-6.1-sol")).toBeDefined();
    expect(priceOf("gpt-6.1-terra")).toBeDefined();
    expect(priceOf("gpt-6-luna")).toBeDefined();
    expect(priceOf("deepseek/deepseek-flash")).toBeDefined();
    expect(priceOf("deepseek/deepseek-v4-pro")).toBeDefined();
    // A harness's short alias resolves to the full model.
    expect(priceOf("opus")).toEqual(priceOf("claude-opus-5-5"));
    expect(priceOf("sonnet")).toEqual(priceOf("claude-sonnet-5"));
    expect(priceOf("deepseek/flash")).toBeDefined();
  });

  test("a model the table does not know has no price, and no estimate", () => {
    expect(priceOf("gpt-9-unknown")).toBeUndefined();
    expect(priceOf("")).toBeUndefined();
    expect(priceOf(undefined)).toBeUndefined();
    expect(estimateUsd("codex", "gpt-9-unknown", codex())).toBeUndefined();
    expect(estimateUsd("codex", "", codex())).toBeUndefined();
    expect(estimateUsd("codex", undefined, codex())).toBeUndefined();
  });

  test("the table is dated", () => {
    expect(PRICE_DATE).toBe("2026-11-06");
  });
});

describe("an estimate never prices raw input_tokens", () => {
  test("codex: cached input is priced at the cache-read rate", () => {
    // 100k uncached at $2 and 900k cached at $0.10 per million, not 1M at $2.
    const usd = estimateUsd("codex", "gpt-6.1-sol", counts({ input: 1_000_000, cached: 900_000, output: 0 }));
    expect(usd).toBeCloseTo(0.29, 10);
    expect(usd).not.toBeCloseTo(2, 2);
  });

  test("pi: input is already uncached, and the cache read is charged separately", () => {
    // 1M input at $0.30 plus 1M cache read at $0.006, and 1M output at $1.20.
    const usd = estimateUsd(
      "pi",
      "deepseek/deepseek-flash",
      counts({ input: 1_000_000, cached: 1_000_000, cacheWrite: 0, output: 1_000_000 }),
    );
    expect(usd).toBeCloseTo(1.506, 10);
  });

  test("claude-code: the cache write has its own rate", () => {
    // 1M input at $4, 1M cache read at $0.20, 1M cache write at $5.
    const usd = estimateUsd(
      "claude-code",
      "opus",
      counts({ input: 1_000_000, cached: 1_000_000, cacheWrite: 1_000_000, output: 0 }),
    );
    expect(usd).toBeCloseTo(9.2, 10);
  });

  test("no estimate for an unknown harness, even with a priced model", () => {
    expect(
      estimateUsd("gemini", "gpt-6-luna", counts({ input: 1_000, cached: 500, output: 10 })),
    ).toBeUndefined();
    expect(estimateUsd("opencode", "gpt-6-luna", counts({ input: 1_000, output: 10 }))).toBeUndefined();
  });
});

describe("a row's best known cost", () => {
  const spent = (
    over: Partial<UsageCounts> & { cost?: number; turns?: number },
    harness = "codex",
    model: string | undefined = "gpt-6-luna",
  ): CostInput => ({
    harness,
    model,
    totals: {
      ...emptyTotals(),
      ...over,
      turns: over.turns ?? 1,
    },
  });

  test("a reported cost wins and is never altered by the estimate", () => {
    // The same tokens would price at ~$2, but the harness reported $1.25.
    const row = spent({ input: 1_000_000, cached: 0, output: 0, cost: 1.25 }, "codex", "gpt-6.1-sol");
    expect(bestCost(row)).toEqual({ usd: 1.25, source: "reported" });
    expect(estimateUsd("codex", "gpt-6.1-sol", row.totals)).toBeCloseTo(2, 10);
  });

  test("a row the harness did not price is estimated", () => {
    const row = spent({ input: 1_000_000, cached: 0, output: 0 });
    expect(bestCost(row)).toEqual({ usd: 0.1, source: "estimated" });
    expect(costOf(row)).toEqual({ usd: 0.1, estimatedUsd: 0.1, unknownTurns: 0 });
  });

  test("a row with neither is unknown, and its turns are counted not zeroed", () => {
    const row = spent({ input: 1_000_000, cached: 0, output: 0, turns: 3 }, "codex", "gpt-9-unknown");
    expect(bestCost(row)).toEqual({ source: "unknown" });
    expect(costOf(row)).toEqual({ unknownTurns: 3 });
    // An unknown harness with a priced model is not priced either.
    expect(bestCost(spent({ input: 1_000, output: 10 }, "gemini"))).toEqual({ source: "unknown" });
  });

  test("a thread row with no model falls back to unknown", () => {
    expect(bestCost(spent({ input: 1_000, output: 10 }, "codex", undefined)).source).toBe("unknown");
  });

  test("the roll-up splits the parts and adds each row only once", () => {
    const total = rollUpCosts([
      spent({ input: 1_000, cached: 0, output: 0, cost: 2.5 }, "codex", "gpt-6-luna"),
      spent({ input: 1_000_000, cached: 0, output: 0 }),
      spent({ input: 1_000_000, cached: 0, output: 0, turns: 3 }, "codex", "gpt-9-unknown"),
    ]);
    expect(total.reportedUsd).toBe(2.5);
    expect(total.estimatedUsd).toBeCloseTo(0.1, 10);
    expect(total.usd).toBeCloseTo(2.6, 10);
    expect(total.unknownTurns).toBe(3);
  });

  test("nothing known has no total at all, never $0", () => {
    expect(rollUpCosts([])).toEqual({ unknownTurns: 0 });
    expect(rollUpCosts([spent({}, "codex", "gpt-9-unknown")]).usd).toBeUndefined();
  });

  test("adding keeps each part absent until one side has it", () => {
    expect(addCost(emptyCost(), { usd: 1.5, reportedUsd: 1.5, unknownTurns: 0 })).toEqual({
      usd: 1.5,
      reportedUsd: 1.5,
      unknownTurns: 0,
    });
    expect(addCost({ usd: 1, estimatedUsd: 1, unknownTurns: 2 }, { unknownTurns: 1 })).toEqual({
      usd: 1,
      estimatedUsd: 1,
      unknownTurns: 3,
    });
  });
});
