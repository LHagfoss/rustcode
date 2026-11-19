// Usage across this computer and the machines it is paired with. A thread run
// on a server lands in that server's data store and reads that server's
// sign-ins, so this computer alone never sees it (issue: usage never syncs to
// the Mac that drives a VPS). Statistics reads every connected machine's
// `_rustcode/data/usage` and adds the reports up here; Settings › Usage reads
// each machine's `_rustcode/usage/shared` (numbers only, no sign-in leaves it).

import type { Json } from "../acp/jsonrpc";
import { parsePlans, type UsagePlan, type UsageReport, type UsageRow } from "../acp/rustcode";
import { addTotals, emptyTotals, totalsOf } from "./statistics";
import { LOCAL_HOST, threadKey } from "./threadKeys";

/** Which store Statistics reads: every connected machine, or one (`""` is this computer). */
export type MachineScope = "all" | string;

/** The hosts a scope reads: this computer first, then each connected machine. */
export function scopeHosts(scope: MachineScope, connected: readonly string[]): string[] {
  if (scope === "all") return [LOCAL_HOST, ...connected];
  return scope === LOCAL_HOST || connected.includes(scope) ? [scope] : [LOCAL_HOST];
}

/** A row named as it is on this computer: a thread on a machine by its key there. */
function onHost(row: UsageRow, host: string): UsageRow {
  if (!host) return row;
  return {
    ...row,
    ...(row.thread ? { thread: threadKey(host, row.thread) } : {}),
    ...(row.parentId ? { parentId: threadKey(host, row.parentId) } : {}),
  };
}

/** What makes two rows of one grouping the same row. */
function rowKey(row: UsageRow, groupBy: string): string {
  switch (groupBy) {
    case "thread":
      return `t\n${row.thread ?? ""}`;
    case "day":
      return `d\n${row.day ?? ""}`;
    case "project":
      return `p\n${row.project ?? ""}`;
    case "model":
      return `m\n${row.harness}\n${row.model ?? ""}`;
    default:
      return `h\n${row.harness}`;
  }
}

/**
 * The reports of several machines as one: rows of the same key (a model, a
 * day) added, a thread kept apart by the machine it ran on. Counts a machine
 * never reported stay absent unless another one reported them, as in one store.
 */
export function mergeUsageReports(parts: readonly { host: string; report: UsageReport }[]): UsageReport {
  const [only] = parts;
  if (parts.length === 1 && only && !only.host) return only.report;
  const groupBy = parts.find((part) => part.report.groupBy)?.report.groupBy ?? "";
  const rows = new Map<string, UsageRow>();
  let totals = emptyTotals();
  for (const { host, report } of parts) {
    totals = addTotals(totals, report.totals);
    for (const raw of report.rows) {
      const row = onHost(raw, host);
      const key = rowKey(row, groupBy);
      const known = rows.get(key);
      if (!known) {
        rows.set(key, row);
        continue;
      }
      // Two machines' rows of one model or day: the same pair only while both agree.
      const modelHarness = known.modelHarness === row.modelHarness ? known.modelHarness : undefined;
      rows.set(key, { ...known, ...addTotals(totalsOf(known), totalsOf(row)), modelHarness });
    }
  }
  return {
    groupBy,
    rows: [...rows.values()],
    totals,
    truncated: parts.some((part) => part.report.truncated),
    parentsReported: parts.some((part) => part.report.parentsReported),
  };
}

/** One machine's plans as Settings › Usage draws them: only the rows it read. */
export function sharedPlans(result: Json | undefined): UsagePlan[] {
  return parsePlans(result).filter((plan) => plan.status !== "notAsked");
}
