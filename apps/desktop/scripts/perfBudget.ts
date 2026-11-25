/** A budget: the most each metric of `bun run perf` may read. */
export type Budget = Record<string, number>;

/** One line per metric over its budget, missing, or not a number. Empty when the run is within budget. */
export function overBudget(results: Record<string, string | number>, budget: Budget): string[] {
  const breaches: string[] = [];
  for (const [metric, max] of Object.entries(budget)) {
    const value = results[metric];
    if (value === undefined) breaches.push(`${metric}: not measured (budget ${max})`);
    else if (typeof value !== "number") breaches.push(`${metric}: ${value} (budget ${max})`);
    else if (value > max) breaches.push(`${metric}: ${value} over its budget of ${max}`);
  }
  return breaches;
}
