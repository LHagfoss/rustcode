import { describe, expect, test } from "bun:test";
import type { Harness } from "../acp/types";
import type { TurnEntry } from "./overview";
import { contextTone, costTotal, threadBilling, threadTotals } from "./threadTotals";

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

describe("billing", () => {
  const harness = {
    id: "claude-code",
    billing: "subscription",
    accounts: [{ id: "work", name: "Work", config_dir: "~/.claude-work", auth: "ok", billing: "api" }],
  } as unknown as Harness;
  const usd = (n: number) => `$${n.toFixed(2)}`;

  test("a thread takes its account's billing, else the default login's", () => {
    expect(threadBilling(harness, "work")).toBe("api");
    expect(threadBilling(harness, undefined)).toBe("subscription");
    expect(threadBilling(harness, "gone")).toBe("unknown");
    expect(threadBilling(undefined, undefined)).toBe("unknown");
  });

  test("a subscription thread shows API value, never Cost", () => {
    const reported = costTotal({ usd: 3, estimated: false }, "subscription", usd);
    expect(reported.label).toBe("API value");
    expect(reported.value).toBe("$3.00");
    expect(reported.tip).toContain("billed separately");
    const estimated = costTotal({ usd: 1, estimated: true }, "subscription", usd);
    expect(estimated.label).toBe("API value");
    expect(estimated.value).toBe("~$1.00");
    expect(estimated.tip).toContain("list price");
  });

  test("API and unknown billing keep Cost", () => {
    expect(costTotal({ usd: 2, estimated: false }, "api", usd)).toEqual({
      label: "Cost",
      value: "$2.00",
      tip: "As the harness reported it",
    });
    expect(costTotal({ usd: 2, estimated: true }, "unknown", usd).tip).toBe(
      "Partly estimated from tokens at list price",
    );
    expect(costTotal({ estimated: false }, "subscription", usd).value).toBe("–");
  });
});
