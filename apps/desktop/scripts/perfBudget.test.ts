import { describe, expect, test } from "bun:test";
import budget from "./perf-budget.json";
import { overBudget } from "./perfBudget";

describe("overBudget", () => {
  test("a run within budget has no breaches", () => {
    expect(
      overBudget({ "idle.commitsPer5s": 0, "stream.commitsPerSec": 40 }, { "idle.commitsPer5s": 0 }),
    ).toEqual([]);
  });

  test("names metrics over budget, missing or unmeasurable", () => {
    expect(
      overBudget(
        { "idle.commitsPer5s": 3, "stream.backlogAfterEndMs": ">120000" },
        { "idle.commitsPer5s": 0, "stream.backlogAfterEndMs": 2000, "startup.heapMb": 80 },
      ),
    ).toEqual([
      "idle.commitsPer5s: 3 over its budget of 0",
      "stream.backlogAfterEndMs: >120000 (budget 2000)",
      "startup.heapMb: not measured (budget 80)",
    ]);
  });

  test("the checked-in budget only names numbers", () => {
    for (const value of Object.values(budget)) expect(typeof value).toBe("number");
  });
});
