import { describe, expect, test } from "bun:test";
import type { TurnEntry } from "./overview";
import { contextTone, threadTotals } from "./threadTotals";

const turn = (over: Partial<TurnEntry>): TurnEntry => ({
  id: "t",
  prompt: "p",
  outcome: "completed",
  ...over,
});

describe("threadTotals", () => {
  test("takes the thread's reported dollars over its turns", () => {
    const totals = threadTotals(
      [turn({ cost: 1, elapsedMs: 1_000 })],
      { used: 1, size: 2, estimated: false, cost: 3 },
      undefined,
      undefined,
    );
    expect(totals).toMatchObject({ usd: 3, estimated: false, workedMs: 1_000 });
  });

  test("adds the turns' costs and the running turn's time when the thread reported none", () => {
    const totals = threadTotals(
      [turn({ cost: 0.5, elapsedMs: 2_000, tokens: 10 }), turn({ cost: 0.25, elapsedMs: 3_000, tokens: 5 })],
      undefined,
      undefined,
      undefined,
      4_000,
    );
    expect(totals).toEqual({ usd: 0.75, estimated: false, workedMs: 9_000, tokens: 15 });
  });

  test("knows no cost when no turn has one", () => {
    expect(threadTotals([turn({})], undefined, undefined, undefined).usd).toBeUndefined();
  });

  test("warms the context meter as it fills", () => {
    expect([contextTone(10), contextTone(75), contextTone(95)]).toEqual(["accent", "warning", "danger"]);
  });
});
