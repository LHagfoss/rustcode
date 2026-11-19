// Usage across paired machines (state/fleetUsage.ts): which stores a scope
// reads, how their reports add up, and which of a machine's plans are drawn.

import { describe, expect, test } from "bun:test";
import type { UsageReport, UsageRow } from "../acp/rustcode";
import { mergeUsageReports, scopeHosts, sharedPlans } from "./fleetUsage";
import { emptyTotals } from "./statistics";

const row = (over: Partial<UsageRow> = {}): UsageRow => ({
  harness: "codex",
  ...emptyTotals(),
  turns: 1,
  ...over,
});

const report = (groupBy: string, rows: UsageRow[], over: Partial<UsageReport> = {}): UsageReport => ({
  groupBy,
  rows,
  totals: rows.reduce(
    (sum, one) => ({
      ...sum,
      turns: sum.turns + one.turns,
      input: (sum.input ?? 0) + (one.input ?? 0),
    }),
    emptyTotals(),
  ),
  truncated: false,
  parentsReported: true,
  ...over,
});

describe("scopeHosts", () => {
  test("all reads this computer first, then each connected machine", () => {
    expect(scopeHosts("all", ["vps", "box"])).toEqual(["", "vps", "box"]);
  });
  test("one machine reads only it, and a gone one falls back to this computer", () => {
    expect(scopeHosts("vps", ["vps"])).toEqual(["vps"]);
    expect(scopeHosts("", ["vps"])).toEqual([""]);
    expect(scopeHosts("gone", ["vps"])).toEqual([""]);
  });
});

describe("mergeUsageReports", () => {
  test("this computer alone is its own report", () => {
    const one = report("model", [row({ model: "gpt-5.5" })]);
    expect(mergeUsageReports([{ host: "", report: one }])).toBe(one);
  });

  test("the same model on two machines is one row with both counts", () => {
    const merged = mergeUsageReports([
      { host: "", report: report("model", [row({ model: "gpt-5.5", input: 10, cost: 1 })]) },
      {
        host: "vps",
        report: report("model", [
          row({ model: "gpt-5.5", input: 5, turns: 2 }),
          row({ model: "o4", input: 1 }),
        ]),
      },
    ]);
    expect(merged.rows).toHaveLength(2);
    const shared = merged.rows.find((one) => one.model === "gpt-5.5");
    expect(shared?.input).toBe(15);
    expect(shared?.turns).toBe(3);
    expect(shared?.cost).toBe(1);
    expect(merged.totals.turns).toBe(4);
  });

  test("days add up across machines", () => {
    const merged = mergeUsageReports([
      { host: "", report: report("day", [row({ day: "2026-11-18", turns: 2 })]) },
      {
        host: "vps",
        report: report("day", [row({ day: "2026-11-18", turns: 3 }), row({ day: "2026-11-19" })]),
      },
    ]);
    expect(merged.rows.map((one) => [one.day, one.turns])).toEqual([
      ["2026-11-18", 5],
      ["2026-11-19", 1],
    ]);
  });

  test("a machine's threads keep their machine, parents included", () => {
    const merged = mergeUsageReports([
      { host: "", report: report("thread", [row({ thread: "a" })]) },
      { host: "vps", report: report("thread", [row({ thread: "a" }), row({ thread: "b", parentId: "a" })]) },
    ]);
    expect(merged.rows.map((one) => one.thread)).toEqual(["a", "vps/a", "vps/b"]);
    expect(merged.rows[2]?.parentId).toBe("vps/a");
  });

  test("a capped machine marks the whole read capped", () => {
    const merged = mergeUsageReports([
      { host: "", report: report("model", []) },
      { host: "vps", report: report("model", [], { truncated: true }) },
    ]);
    expect(merged.truncated).toBe(true);
  });
});

describe("sharedPlans", () => {
  test("drops the rows the machine did not read", () => {
    const plans = sharedPlans({
      plans: [
        { id: "claude", name: "Claude", status: "notAsked", windows: [] },
        {
          id: "codex",
          name: "ChatGPT",
          status: "ok",
          windows: [{ label: "5-hour", usedPercent: 40, windowMinutes: 300, resetsAt: 1 }],
        },
      ],
    });
    expect(plans.map((plan) => plan.id)).toEqual(["codex"]);
  });
});
