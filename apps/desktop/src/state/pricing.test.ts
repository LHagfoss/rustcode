// The price estimate behind Settings › Statistics: each harness's recorded
// counts normalized before a rate is applied, list prices for the models this
// machine runs, and a blank (never a zero) where either is unknown. The turn
// shapes are one row each, with the numbers the local store recorded.

import { describe, expect, test } from "bun:test";
import type { UsageCounts } from "../acp/rustcode";
import { estimateAll, estimateGroup, estimateUsd, normalize, PRICE_DATE, priceOf } from "./pricing";
import type { HarnessTotal, ModelTotal } from "./statistics";
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

describe("a harness row's estimate", () => {
  const model = (name: string, over: Partial<UsageCounts>, turns: number): ModelTotal => ({
    model: name,
    totals: {
      ...emptyTotals(),
      ...over,
      turns,
      turnsWithoutCost: turns,
    },
  });
  const group = (models: ModelTotal[]): HarnessTotal => ({
    harness: "codex",
    totals: models[0]!.totals,
    models,
  });

  test("sums the model rows that can be priced and counts the turns it leaves out", () => {
    const rows = group([
      model("gpt-6-luna", { input: 1_000_000, cached: 0, output: 0 }, 2),
      model("gpt-9-unknown", { input: 1_000_000, cached: 0, output: 0 }, 3),
    ]);
    const estimate = estimateGroup(rows);
    expect(estimate.usd).toBeCloseTo(0.1, 10);
    expect(estimate.turnsNotEstimated).toBe(3);
  });

  test("a harness with nothing priced has no estimate at all", () => {
    const estimate = estimateGroup(group([model("gpt-9-unknown", { input: 100, output: 1 }, 1)]));
    expect(estimate.usd).toBeUndefined();
    expect(estimate.turnsNotEstimated).toBe(1);
  });

  test("the report total adds the priced rows and keeps the rest counted", () => {
    const codexRows = group([model("gpt-6-luna", { input: 1_000_000, cached: 0, output: 0 }, 1)]);
    const geminiRows: HarnessTotal = {
      harness: "gemini",
      totals: { ...emptyTotals(), input: 500, output: 50, turns: 4 },
      models: [{ model: "gpt-6-luna", totals: { ...emptyTotals(), input: 500, output: 50, turns: 4 } }],
    };
    const all = estimateAll([codexRows, geminiRows]);
    expect(all.usd).toBeCloseTo(0.1, 10);
    expect(all.turnsNotEstimated).toBe(4);
    expect(all.byHarness.get("gemini")?.usd).toBeUndefined();
  });
});
