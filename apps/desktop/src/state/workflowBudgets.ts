import type { Json } from "../acp/jsonrpc";

/** The workflow spend limits and the most each may be set to (`rustcode/engine/src/workflow/budget.rs`). */
export const WORKFLOW_BUDGETS = {
  "workflow-token-budget": { max: 10_000_000_000, decimal: false },
  "workflow-cost-budget-usd": { max: 100_000, decimal: true },
} as const;

export type WorkflowBudgetKey = keyof typeof WORKFLOW_BUDGETS;

/**
 * What a budget field's text saves as: 0 for an empty field (no limit), the number when it is in
 * range, and `null` when it is not a number the engine takes.
 */
export function parseBudget(key: WorkflowBudgetKey, text: string): number | null {
  const trimmed = text
    .trim()
    .replace(/[\s,_]/g, "")
    .replace(/^\$/, "");
  if (!trimmed) return 0;
  const spec = WORKFLOW_BUDGETS[key];
  if (!(spec.decimal ? /^\d+(\.\d+)?$/ : /^\d+$/).test(trimmed)) return null;
  const value = Number(trimmed);
  if (!Number.isFinite(value) || value < 0 || value > spec.max) return null;
  return value;
}

/** What a run's workers spent, as `workflow_status` reports it under `usage`. */
export interface RunUsage {
  tokens: number;
  costUsd: number | null;
  /** "1 of 3 workers": how many reported a cost. */
  costReportedBy: string;
  /** Which budget the run reached, if any. */
  reached: "tokens" | "cost" | null;
}

/** Read `usage` and `budget` of a status answer; `null` when it has none. */
export function parseRunUsage(usage: Json | undefined, budget: Json | undefined): RunUsage | null {
  if (!usage || typeof usage !== "object" || Array.isArray(usage)) return null;
  const reached = budget && typeof budget === "object" && !Array.isArray(budget) ? budget.reached : undefined;
  return {
    tokens: typeof usage.tokens === "number" ? usage.tokens : 0,
    costUsd: typeof usage.cost_usd === "number" ? usage.cost_usd : null,
    costReportedBy: typeof usage.cost_reported_by === "string" ? usage.cost_reported_by : "",
    reached: reached === "tokens" || reached === "cost" ? reached : null,
  };
}

/** One line for a run card: "12.3k tokens · $0.42 reported (1 of 3 workers)". */
export function runUsageText(usage: RunUsage): string {
  const tokens =
    usage.tokens >= 1_000_000
      ? `${(usage.tokens / 1_000_000).toFixed(1)}M`
      : usage.tokens >= 1_000
        ? `${(usage.tokens / 1_000).toFixed(1)}k`
        : String(usage.tokens);
  const parts = [`${tokens} tokens`];
  if (usage.costUsd !== null) parts.push(`$${usage.costUsd.toFixed(2)} reported (${usage.costReportedBy})`);
  if (usage.reached) parts.push(usage.reached === "tokens" ? "token budget reached" : "cost budget reached");
  return parts.join(" · ");
}
