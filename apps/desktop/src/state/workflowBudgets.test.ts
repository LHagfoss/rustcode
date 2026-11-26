import { expect, test } from "bun:test";
import { parseBudget, parseRunUsage, runUsageText } from "./workflowBudgets";

test("a budget field saves an empty value as no limit and refuses what the engine would not take", () => {
  expect(parseBudget("workflow-token-budget", "")).toBe(0);
  expect(parseBudget("workflow-token-budget", "5,000,000")).toBe(5_000_000);
  expect(parseBudget("workflow-token-budget", "2.5")).toBeNull();
  expect(parseBudget("workflow-token-budget", "99999999999")).toBeNull();
  expect(parseBudget("workflow-cost-budget-usd", "$20.50")).toBe(20.5);
  expect(parseBudget("workflow-cost-budget-usd", "abc")).toBeNull();
});

test("a run's usage reads from its status and names the budget it reached", () => {
  const usage = parseRunUsage(
    { tokens: 12_345, cost_usd: 0.42, cost_reported_by: "1 of 3 workers" },
    { tokens: 10_000, cost_usd: null, reached: "tokens" },
  );
  expect(usage).toEqual({
    tokens: 12_345,
    costUsd: 0.42,
    costReportedBy: "1 of 3 workers",
    reached: "tokens",
  });
  expect(runUsageText(usage!)).toBe("12.3k tokens · $0.42 reported (1 of 3 workers) · token budget reached");
  expect(runUsageText(parseRunUsage({ tokens: 10, cost_usd: null }, null)!)).toBe("10 tokens");
  expect(parseRunUsage(undefined, undefined)).toBeNull();
});
