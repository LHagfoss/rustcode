// What the mock's threads spent, as `_rustcode/usage` reports it (acp/mock.ts).

import type { JsonObject } from "./jsonrpc";
import type { MockThread } from "./mockSeeds";

/** What a mock thread spent, as the data store would record it. */
export interface MockSpend {
  input: number;
  output: number;
  cached: number;
  cacheWrite: number | null;
  cost: number | null;
  turns: number;
  /** Turns with neither input nor output reported. */
  withoutTokens: number;
  /** Turns missing one of the four token counts. */
  withoutAny: number;
  withoutInput: number;
  withoutOutput: number;
  withoutCached: number;
  withoutCacheWrite: number;
  withoutCost: number;
}

/** Known threads get numbers that make the missing ones visible; the rest get a small canned spend. */
export const MOCK_SPEND: Record<string, MockSpend> = {
  "mock-thread-1": {
    input: 1_240_000,
    output: 32_100,
    cached: 980_000,
    cacheWrite: null,
    cost: null,
    turns: 8,
    withoutTokens: 0,
    withoutAny: 8,
    withoutInput: 0,
    withoutOutput: 0,
    withoutCached: 8,
    withoutCacheWrite: 8,
    withoutCost: 8,
  },
  "mock-thread-2": {
    input: 248_000,
    output: 9_100,
    cached: 201_000,
    cacheWrite: null,
    cost: 3.42,
    turns: 12,
    withoutTokens: 0,
    withoutAny: 12,
    withoutInput: 0,
    withoutOutput: 0,
    withoutCached: 0,
    withoutCacheWrite: 12,
    withoutCost: 0,
  },
  "mock-thread-3": {
    input: 88_000,
    output: 4_200,
    cached: 60_000,
    cacheWrite: 12_000,
    cost: null,
    turns: 4,
    withoutTokens: 0,
    withoutAny: 0,
    withoutInput: 0,
    withoutOutput: 0,
    withoutCached: 0,
    withoutCacheWrite: 0,
    withoutCost: 4,
  },
  "mock-thread-4": {
    input: 512_000,
    output: 22_000,
    cached: 400_000,
    cacheWrite: 0,
    cost: 5.1,
    turns: 9,
    withoutTokens: 0,
    withoutAny: 0,
    withoutInput: 0,
    withoutOutput: 0,
    withoutCached: 0,
    withoutCacheWrite: 0,
    withoutCost: 0,
  },
  "mock-thread-claude": {
    input: 112,
    output: 72_297,
    cached: 14_757_930,
    cacheWrite: 633_078,
    cost: 77.22,
    turns: 9,
    withoutTokens: 7,
    withoutAny: 7,
    withoutInput: 7,
    withoutOutput: 7,
    withoutCached: 0,
    withoutCacheWrite: 0,
    withoutCost: 0,
  },
  "mock-parent": {
    input: 820_000,
    output: 30_000,
    cached: 700_000,
    cacheWrite: null,
    cost: null,
    turns: 6,
    withoutTokens: 0,
    withoutAny: 6,
    withoutInput: 0,
    withoutOutput: 0,
    withoutCached: 0,
    withoutCacheWrite: 6,
    withoutCost: 6,
  },
  "mock-child": {
    input: 4_500_000,
    output: 180_000,
    cached: 3_900_000,
    cacheWrite: 420_000,
    cost: 8.4,
    turns: 14,
    withoutTokens: 0,
    withoutAny: 0,
    withoutInput: 0,
    withoutOutput: 0,
    withoutCached: 0,
    withoutCacheWrite: 0,
    withoutCost: 0,
  },
  "mock-orphan": {
    input: 1_200_000,
    output: 40_000,
    cached: 900_000,
    cacheWrite: null,
    cost: 4.1,
    turns: 7,
    withoutTokens: 0,
    withoutAny: 7,
    withoutInput: 0,
    withoutOutput: 0,
    withoutCached: 0,
    withoutCacheWrite: 7,
    withoutCost: 0,
  },
  "mock-plain": {
    input: 310_000,
    output: 12_000,
    cached: 250_000,
    cacheWrite: null,
    cost: null,
    turns: 5,
    withoutTokens: 0,
    withoutAny: 5,
    withoutInput: 0,
    withoutOutput: 0,
    withoutCached: 0,
    withoutCacheWrite: 5,
    withoutCost: 5,
  },
  // `?scenario=delegates`: a branch with reported costs, and one thread whose
  // model has no price, so the tree shows `—` rather than a made-up number.
  "mock-delegate-root": {
    input: 640_000,
    output: 24_000,
    cached: 520_000,
    cacheWrite: null,
    cost: 1.9,
    turns: 5,
    withoutTokens: 0,
    withoutAny: 5,
    withoutInput: 0,
    withoutOutput: 0,
    withoutCached: 0,
    withoutCacheWrite: 5,
    withoutCost: 0,
  },
  "mock-delegate-research": {
    input: 1_450_000,
    output: 58_000,
    cached: 1_200_000,
    cacheWrite: null,
    cost: 2.4,
    turns: 9,
    withoutTokens: 0,
    withoutAny: 9,
    withoutInput: 0,
    withoutOutput: 0,
    withoutCached: 0,
    withoutCacheWrite: 9,
    withoutCost: 0,
  },
  "mock-delegate-tests": {
    input: 520_000,
    output: 26_000,
    cached: 410_000,
    cacheWrite: null,
    cost: 0.85,
    turns: 4,
    withoutTokens: 0,
    withoutAny: 4,
    withoutInput: 0,
    withoutOutput: 0,
    withoutCached: 0,
    withoutCacheWrite: 4,
    withoutCost: 0,
  },
  "mock-delegate-docs": {
    input: 190_000,
    output: 8_000,
    cached: 150_000,
    cacheWrite: 21_000,
    cost: null,
    turns: 3,
    withoutTokens: 0,
    withoutAny: 0,
    withoutInput: 0,
    withoutOutput: 0,
    withoutCached: 0,
    withoutCacheWrite: 0,
    withoutCost: 3,
  },
  "mock-delegate-review": {
    input: 2_100_000,
    output: 61_000,
    cached: 1_800_000,
    cacheWrite: null,
    cost: 3.1,
    turns: 11,
    withoutTokens: 0,
    withoutAny: 11,
    withoutInput: 0,
    withoutOutput: 0,
    withoutCached: 0,
    withoutCacheWrite: 11,
    withoutCost: 0,
  },
  "mock-delegate-wait": {
    input: 380_000,
    output: 15_000,
    cached: 300_000,
    cacheWrite: 48_000,
    cost: 1.05,
    turns: 4,
    withoutTokens: 0,
    withoutAny: 4,
    withoutInput: 0,
    withoutOutput: 0,
    withoutCached: 0,
    withoutCacheWrite: 0,
    withoutCost: 0,
  },
  "mock-delegate-stop": {
    input: 60_000,
    output: 2_000,
    cached: 40_000,
    cacheWrite: null,
    cost: 0.2,
    turns: 1,
    withoutTokens: 0,
    withoutAny: 1,
    withoutInput: 0,
    withoutOutput: 0,
    withoutCached: 0,
    withoutCacheWrite: 1,
    withoutCost: 0,
  },
};

export function spendOf(thread: MockThread, index: number): MockSpend {
  const known = MOCK_SPEND[thread.id];
  if (known) return known;
  const input = 40_000 + (index % 7) * 12_000;
  return {
    input,
    output: Math.round(input / 10),
    cached: Math.round(input / 3),
    cacheWrite: null,
    cost: null,
    turns: 1,
    withoutTokens: 0,
    withoutAny: 1,
    withoutInput: 0,
    withoutOutput: 0,
    withoutCached: 0,
    withoutCacheWrite: 1,
    withoutCost: 1,
  };
}

export const sumKnown = (values: (number | null)[]): number | null => {
  const known = values.filter((value): value is number => value !== null);
  return known.length ? known.reduce((sum, value) => sum + value, 0) : null;
};

/** The days in the last month a mock thread worked, so the daily chart has gaps and variation. */
export const MOCK_DAY_OFFSETS = [0, 1, 1, 3, 6, 11, 18, 24];

export function mockUsageDays(index: number): string[] {
  const count = 1 + (index % 3);
  const days = new Set<string>();
  for (let step = 0; step < count; step++) {
    const offset = MOCK_DAY_OFFSETS[(index + step * 3) % MOCK_DAY_OFFSETS.length] ?? 0;
    days.add(new Date(Date.now() - offset * 86_400_000).toISOString().slice(0, 10));
  }
  return [...days];
}

/** A thread's spend split over the days it worked, so a window of days keeps what it holds. */
export function mockUsageParts(spend: MockSpend, index: number): { day: string; spend: MockSpend }[] {
  const days = mockUsageDays(index);
  const parts = days.length;
  // Counts are whole; the last part takes the remainder so the parts add up to the thread.
  const count = (value: number | null, at: number): number | null =>
    value === null
      ? null
      : at === parts - 1
        ? value - Math.floor(value / parts) * (parts - 1)
        : Math.floor(value / parts);
  // A cost is a fraction of a dollar: share it evenly.
  const usd = (value: number | null): number | null => (value === null ? null : value / parts);
  return days.map((day, at) => ({
    day,
    spend: {
      input: count(spend.input, at)!,
      output: count(spend.output, at)!,
      cached: count(spend.cached, at)!,
      cacheWrite: count(spend.cacheWrite, at),
      cost: usd(spend.cost),
      turns: count(spend.turns, at)!,
      withoutTokens: count(spend.withoutTokens, at)!,
      withoutAny: count(spend.withoutAny, at)!,
      withoutInput: count(spend.withoutInput, at)!,
      withoutOutput: count(spend.withoutOutput, at)!,
      withoutCached: count(spend.withoutCached, at)!,
      withoutCacheWrite: count(spend.withoutCacheWrite, at)!,
      withoutCost: count(spend.withoutCost, at)!,
    },
  }));
}

/**
 * `_rustcode/data/usage`, grouped like the engine groups the store
 * (`engine/src/acp/data.rs`). `since` keeps only the days it names, as the
 * engine's own comparison does, so the period switch has something to switch.
 */
export function mockUsage(threads: MockThread[], groupBy: string, since?: string): JsonObject {
  const groups = new Map<string, { keys: JsonObject; spends: MockSpend[] }>();
  for (const [index, thread] of threads.entries()) {
    const parts = mockUsageParts(spendOf(thread, index), index).filter((part) => !since || part.day >= since);
    if (!parts.length) continue;
    if (groupBy === "day") {
      for (const part of parts) {
        const group = groups.get(part.day) ?? { keys: { day: part.day }, spends: [] };
        group.spends.push(part.spend);
        groups.set(part.day, group);
      }
      continue;
    }
    let key: string;
    let keys: JsonObject;
    switch (groupBy) {
      case "model":
        key = `${thread.harness}\u0000${thread.model}`;
        keys = { harness: thread.harness, model: thread.model };
        break;
      case "project":
        key = thread.cwd;
        keys = { project: thread.cwd };
        break;
      case "thread":
        key = thread.id;
        keys = {
          thread: thread.id,
          title: thread.title ?? "",
          project: thread.cwd,
          harness: thread.harness,
          model: thread.model,
          // The mock's turns all run the thread's own harness and model.
          model_harness: thread.harness,
          parent_id: thread.parent?.id ?? null,
          parent_title: thread.parent?.title ?? null,
        };
        break;
      default:
        key = thread.harness;
        keys = { harness: thread.harness };
    }
    const group = groups.get(key) ?? { keys, spends: [] };
    for (const part of parts) group.spends.push(part.spend);
    groups.set(key, group);
  }
  const rows = [...groups.values()]
    .map(({ keys, spends }) => ({
      ...keys,
      input_tokens: sumKnown(spends.map((spend) => spend.input)),
      output_tokens: sumKnown(spends.map((spend) => spend.output)),
      cached_tokens: sumKnown(spends.map((spend) => spend.cached)),
      cache_write_tokens: sumKnown(spends.map((spend) => spend.cacheWrite)),
      cost_usd: sumKnown(spends.map((spend) => spend.cost)),
      turns: spends.reduce((sum, spend) => sum + spend.turns, 0),
      turns_without_tokens: spends.reduce((sum, spend) => sum + spend.withoutTokens, 0),
      turns_without_any_tokens: spends.reduce((sum, spend) => sum + spend.withoutAny, 0),
      turns_without_input_tokens: spends.reduce((sum, spend) => sum + spend.withoutInput, 0),
      turns_without_output_tokens: spends.reduce((sum, spend) => sum + spend.withoutOutput, 0),
      turns_without_cached_tokens: spends.reduce((sum, spend) => sum + spend.withoutCached, 0),
      turns_without_cache_write_tokens: spends.reduce((sum, spend) => sum + spend.withoutCacheWrite, 0),
      turns_without_cost: spends.reduce((sum, spend) => sum + spend.withoutCost, 0),
    }))
    .sort(
      (one, other) =>
        Number(other.input_tokens ?? 0) +
          Number(other.output_tokens ?? 0) -
          (Number(one.input_tokens ?? 0) + Number(one.output_tokens ?? 0)) || 0,
    );
  const total = (key: string) =>
    sumKnown(
      rows.map((row) =>
        typeof row[key as keyof typeof row] === "number" ? (row[key as keyof typeof row] as number) : null,
      ),
    );
  const turns = rows.reduce((sum, row) => sum + Number(row.turns ?? 0), 0);
  return {
    group_by: groupBy,
    since: null,
    project: null,
    scope: "every project",
    rows,
    totals: {
      input_tokens: total("input_tokens"),
      output_tokens: total("output_tokens"),
      cached_tokens: total("cached_tokens"),
      cache_write_tokens: total("cache_write_tokens"),
      cost_usd: total("cost_usd"),
      turns,
      turns_without_tokens: rows.reduce((sum, row) => sum + Number(row.turns_without_tokens ?? 0), 0),
      turns_without_any_tokens: rows.reduce((sum, row) => sum + Number(row.turns_without_any_tokens ?? 0), 0),
      turns_without_input_tokens: rows.reduce(
        (sum, row) => sum + Number(row.turns_without_input_tokens ?? 0),
        0,
      ),
      turns_without_output_tokens: rows.reduce(
        (sum, row) => sum + Number(row.turns_without_output_tokens ?? 0),
        0,
      ),
      turns_without_cached_tokens: rows.reduce(
        (sum, row) => sum + Number(row.turns_without_cached_tokens ?? 0),
        0,
      ),
      turns_without_cache_write_tokens: rows.reduce(
        (sum, row) => sum + Number(row.turns_without_cache_write_tokens ?? 0),
        0,
      ),
      turns_without_cost: rows.reduce((sum, row) => sum + Number(row.turns_without_cost ?? 0), 0),
    },
    truncated: false,
    note: "tokens are what each harness reported; cost is the USD a harness reported, never a price RustCode computed",
    version: 1,
  };
}
