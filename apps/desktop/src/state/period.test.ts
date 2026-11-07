// What Settings › Statistics's period switch asks for and how it says a number
// changed: one test per rule the page leans on (state/period.ts).

import { describe, expect, test } from "bun:test";
import type { UsageRow } from "../acp/rustcode";
import {
  dailySeries,
  daysAgo,
  previousRows,
  rangeDays,
  sparklinePath,
  sparklineSeries,
  statsRange,
  sumRows,
  trend,
} from "./period";

const at = new Date("2026-11-10T15:30:00Z");

const row = (over: Partial<UsageRow> = {}): UsageRow => ({
  harness: "codex",
  model: "gpt-6-luna",
  input: 100,
  output: 10,
  cached: 50,
  cacheWrite: undefined,
  cost: undefined,
  turns: 1,
  turnsWithoutTokens: 0,
  turnsWithoutAnyTokens: 0,
  turnsWithoutInputTokens: 0,
  turnsWithoutOutputTokens: 0,
  turnsWithoutCachedTokens: 0,
  turnsWithoutCacheWriteTokens: 0,
  turnsWithoutCost: 1,
  ...over,
});

describe("a period's days", () => {
  test("today is one day, seven days starts six days back", () => {
    expect(statsRange("today", at)).toEqual({ since: "2026-11-10", previousSince: "2026-11-09", days: 1 });
    expect(statsRange("7d", at)).toEqual({ since: "2026-11-04", previousSince: "2026-10-28", days: 7 });
    expect(statsRange("30d", at)).toEqual({ since: "2026-10-12", previousSince: "2026-09-12", days: 30 });
  });

  test("every day recorded asks for no day and has no period before it", () => {
    expect(statsRange("all", at)).toEqual({ days: 0 });
  });

  test("days are UTC, so the store's date() and the range agree", () => {
    expect(daysAgo(new Date("2026-01-01T00:30:00Z"), 0)).toBe("2026-01-01");
    expect(daysAgo(new Date("2026-01-01T00:30:00Z"), 1)).toBe("2025-12-31");
    expect(rangeDays("2026-11-08", 3)).toEqual(["2026-11-08", "2026-11-09", "2026-11-10"]);
    expect(rangeDays(undefined, 30)).toEqual([]);
  });
});

describe("the change against the period before", () => {
  test("up, down and level are read from the two numbers", () => {
    expect(trend(120, 100)).toEqual({ direction: "up", percent: 20 });
    expect(trend(75, 100)).toEqual({ direction: "down", percent: -25 });
    expect(trend(100.2, 100)?.direction).toBe("level");
    expect(trend(0, 0)).toEqual({ direction: "level" });
  });

  test("a period that recorded nothing is up with no percentage", () => {
    expect(trend(3, 0)).toEqual({ direction: "up" });
  });

  test("a number either side never reported has no change", () => {
    expect(trend(undefined, 3)).toBeUndefined();
    expect(trend(3, undefined)).toBeUndefined();
  });
});

describe("the rows of the period before", () => {
  test("a row in both periods is the difference, never negative", () => {
    const before = previousRows(
      [row({ input: 300, output: 30, cost: 2, turns: 3 })],
      [row({ input: 400, output: 40, cost: 2.5, turns: 4 })],
    );
    // The current period reported more than the wider read holds: the store changed between reads.
    expect(before).toEqual([]);
  });

  test("a row only the wider read has belongs to the period before", () => {
    const before = previousRows(
      [row({ harness: "pi", model: "deepseek/flash", input: 500, output: 50, turns: 5 })],
      [],
    );
    expect(before).toHaveLength(1);
    expect(before[0]).toMatchObject({ harness: "pi", input: 500, turns: 5 });
  });

  test("an unreported count stays absent through the subtraction", () => {
    const [before] = previousRows(
      [row({ input: undefined, output: 30, cached: undefined, cost: 2, turns: 3 })],
      [row({ input: undefined, output: 10, cached: undefined, cost: 1, turns: 1 })],
    );
    expect(before).toMatchObject({ input: undefined, output: 20, cached: undefined, cost: 1, turns: 2 });
  });

  test("sumRows adds the counts only a row reported", () => {
    const totals = sumRows([
      row({ input: 100, output: 10, turns: 1 }),
      row({ input: undefined, output: 30, turns: 2, turnsWithoutInputTokens: 2 }),
    ]);
    expect(totals).toMatchObject({ input: 100, output: 40, turns: 3, turnsWithoutInputTokens: 2 });
  });
});

describe("a day at a time", () => {
  const days = ["2026-11-08", "2026-11-09", "2026-11-10"];
  const rows = [
    row({ day: "2026-11-08", input: 100, output: 10, turns: 2 }),
    row({ day: "2026-11-10", input: 50, output: undefined, turns: 1, cost: 0.5 }),
  ];

  test("a day with nothing recorded is still a point, and a gap stays one", () => {
    expect(dailySeries(rows, days)).toEqual([
      { day: "2026-11-08", turns: 2, tokens: 110, cost: undefined },
      { day: "2026-11-09", turns: 0, tokens: undefined, cost: undefined },
      { day: "2026-11-10", turns: 1, tokens: 50, cost: 0.5 },
    ]);
  });

  test("every day recorded uses the days the rows name, oldest first", () => {
    expect(dailySeries(rows, []).map((point) => point.day)).toEqual(["2026-11-08", "2026-11-10"]);
  });

  test("the mock's shape: cost and tokens on a few days, blank on the rest", () => {
    // A month of days, as the mock answers `groupBy=day`: only a handful of days
    // have a row, and one of them reported no cost and no output.
    const days = rangeDays("2026-09-01", 30);
    const rows = [
      row({ day: days[2]!, cost: 0.4, input: 100, output: 10 }),
      row({ day: days[9]!, cost: 1.2, input: 200, output: 20 }),
      row({ day: days[27]!, cost: undefined, input: 300, output: undefined }),
    ];
    const points = dailySeries(rows, days);
    // Two days reported a cost; three reported tokens (the last one only input).
    expect(points.filter((point) => point.cost === undefined)).toHaveLength(28);
    expect(points.filter((point) => point.tokens === undefined)).toHaveLength(27);

    // The tile's line is full width: a blank day sits at zero, so no day is a gap.
    const cost = sparklineSeries(points.map((point) => point.cost));
    const tokens = sparklineSeries(points.map((point) => point.tokens));
    expect(cost).toHaveLength(30);
    expect(cost.filter((value) => value === 0)).toHaveLength(28);
    for (const series of [cost, tokens]) {
      const path = sparklinePath(series, 100, 24);
      expect(path.match(/[ML]/g)).toHaveLength(30);
      expect(path.startsWith("M0.00 ")).toBe(true);
      expect(path.includes("L100.00 ")).toBe(true);
    }
  });

  test("a sparkline breaks at a day with nothing and stays empty with no numbers", () => {
    const path = sparklinePath([1, undefined, 3], 20, 10);
    expect(path.startsWith("M0.00")).toBe(true);
    expect(path.match(/M/g)).toHaveLength(2);
    expect(sparklinePath([undefined, undefined], 20, 10)).toBe("");
    expect(sparklinePath([], 20, 10)).toBe("");
    expect(sparklinePath([0, 0], 20, 10)).toBe(
      `M0.00 ${(10 - 0.5).toFixed(2)}L20.00 ${(10 - 0.5).toFixed(2)}`,
    );
  });
});
