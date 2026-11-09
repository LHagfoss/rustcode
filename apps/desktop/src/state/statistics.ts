// What Settings › Statistics draws, from the two reads of the local data store
// (`_rustcode/data/usage`): one grouped by harness and model, one by thread.
// The thread read is rolled up along the delegation a thread records in its
// metadata (or the link that started it), so a parent's total includes the
// threads it started and every thread is counted once. Tokens and cost a
// harness never reported stay absent; a thread's cost is the USD its harness
// reported or, where it reported none, the list-price estimate
// (`state/pricing.ts`), with the reported and estimated parts kept apart. A
// thread is estimated only when all its turns ran one harness and model
// (`model_harness`/`model` on the row); otherwise one row's cache convention
// and rate would be applied to turns that never used them.
//
// Pure (no React, no store at runtime), so the page and its tests share one
// answer.

import type { UsageCounts, UsageReport, UsageRow, UsageTotals } from "../acp/rustcode";
import { delegateParent, type SessionSummary } from "../acp/types";
import type { Link } from "./orchestration";
import { addCost, type CostTotals, costOf, emptyCost } from "./pricing";
import { parentOf } from "./topLevel";

export function emptyTotals(): UsageTotals {
  return {
    turns: 0,
    turnsWithoutTokens: 0,
    turnsWithoutAnyTokens: 0,
    turnsWithoutInputTokens: 0,
    turnsWithoutOutputTokens: 0,
    turnsWithoutCachedTokens: 0,
    turnsWithoutCacheWriteTokens: 0,
    turnsWithoutCost: 0,
  };
}

const add = (one?: number, other?: number): number | undefined =>
  one === undefined && other === undefined ? undefined : (one ?? 0) + (other ?? 0);

/** Two totals added, each count only when at least one side reported it. */
export function addTotals(one: UsageTotals, other: UsageTotals): UsageTotals {
  return {
    input: add(one.input, other.input),
    output: add(one.output, other.output),
    cached: add(one.cached, other.cached),
    cacheWrite: add(one.cacheWrite, other.cacheWrite),
    cost: add(one.cost, other.cost),
    turns: one.turns + other.turns,
    turnsWithoutTokens: one.turnsWithoutTokens + other.turnsWithoutTokens,
    turnsWithoutAnyTokens: one.turnsWithoutAnyTokens + other.turnsWithoutAnyTokens,
    turnsWithoutInputTokens: one.turnsWithoutInputTokens + other.turnsWithoutInputTokens,
    turnsWithoutOutputTokens: one.turnsWithoutOutputTokens + other.turnsWithoutOutputTokens,
    turnsWithoutCachedTokens: one.turnsWithoutCachedTokens + other.turnsWithoutCachedTokens,
    turnsWithoutCacheWriteTokens: one.turnsWithoutCacheWriteTokens + other.turnsWithoutCacheWriteTokens,
    turnsWithoutCost: one.turnsWithoutCost + other.turnsWithoutCost,
  };
}

export function totalsOf(row: UsageRow): UsageTotals {
  return {
    input: row.input,
    output: row.output,
    cached: row.cached,
    cacheWrite: row.cacheWrite,
    cost: row.cost,
    turns: row.turns,
    turnsWithoutTokens: row.turnsWithoutTokens,
    turnsWithoutAnyTokens: row.turnsWithoutAnyTokens,
    turnsWithoutInputTokens: row.turnsWithoutInputTokens,
    turnsWithoutOutputTokens: row.turnsWithoutOutputTokens,
    turnsWithoutCachedTokens: row.turnsWithoutCachedTokens,
    turnsWithoutCacheWriteTokens: row.turnsWithoutCacheWriteTokens,
    turnsWithoutCost: row.turnsWithoutCost,
  };
}

/** What a row used, for ordering; cached tokens are shown but not counted twice. */
export function usedTokens(counts: UsageCounts): number {
  return (counts.input ?? 0) + (counts.output ?? 0);
}

export interface ModelTotal {
  model: string;
  totals: UsageTotals;
}

/** One harness and the models it ran, from a `group_by=model` report. */
export interface HarnessTotal {
  harness: string;
  totals: UsageTotals;
  models: ModelTotal[];
}

const byUse = (one: { totals: UsageTotals }, other: { totals: UsageTotals }) =>
  usedTokens(other.totals) - usedTokens(one.totals);

/** The model rows grouped by harness, both ordered by tokens used. */
export function harnessTotals(rows: UsageRow[]): HarnessTotal[] {
  const groups = new Map<string, { totals: UsageTotals; models: Map<string, UsageTotals> }>();
  for (const row of rows) {
    const harness = groups.get(row.harness) ?? { totals: emptyTotals(), models: new Map() };
    harness.totals = addTotals(harness.totals, totalsOf(row));
    const model = row.model ?? "";
    harness.models.set(model, addTotals(harness.models.get(model) ?? emptyTotals(), totalsOf(row)));
    groups.set(row.harness, harness);
  }
  return [...groups.entries()]
    .map(([harness, group]) => ({
      harness,
      totals: group.totals,
      models: [...group.models.entries()]
        .map(([model, totals]) => ({ model, totals }))
        .sort((one, other) => byUse(one, other) || one.model.localeCompare(other.model)),
    }))
    .sort((one, other) => byUse(one, other) || one.harness.localeCompare(other.harness));
}

/** A thread and everything it used, with the threads it started folded in. */
export interface ThreadTotal {
  id: string;
  title: string;
  harness: string;
  project?: string;
  /** This thread's own turns, without its delegates. */
  own: UsageTotals;
  /** Its own turns plus every thread it started, at any depth. */
  total: UsageTotals;
  /** This thread's own best known cost, without its delegates. */
  ownCost: CostTotals;
  /** Its own cost plus every delegate's, split into reported and estimated parts. */
  cost: CostTotals;
  /** How many threads are folded in, at any depth. */
  delegateCount: number;
  /**
   * It records a parent only by title, so it cannot be joined to that thread
   * and is counted on its own.
   */
  unlinked: boolean;
}

export interface ThreadReport {
  /** The top-level threads, ordered by their rolled-up tokens. */
  threads: ThreadTotal[];
  /** The read stopped at the store's row cap. */
  truncated: boolean;
  /** Parent records that formed a loop; the edge was ignored rather than followed. */
  brokenParents: number;
  /** Threads with a title-only parent. */
  unlinked: number;
  /** Parents came from the store snapshot, not a session list that may lag or be capped. */
  parentsFromStore: boolean;
  /** The app stopped paging the session list, so a fallback hierarchy may be incomplete. */
  sessionsCapped: boolean;
  /** Thread rows with no session summary, which a fallback hierarchy needs. */
  missingSessions: number;
  /** A thread fell back to a capped or incomplete session list, so its parent may be unknown. */
  hierarchyIncomplete: boolean;
}

/**
 * The `group_by=thread` rows rolled up by delegation. Each thread appears once:
 * as a top-level row, or inside the thread that started it. A parent outside
 * these rows (no recorded turns) cannot hold a child, so the child is shown on
 * its own; a title-only parent cannot be identified at all and is marked.
 */
export function threadReport(
  report: UsageReport,
  sessions: readonly SessionSummary[],
  links: readonly Link[],
  sessionsCapped = false,
): ThreadReport {
  const rows = new Map<string, UsageRow>();
  for (const row of report.rows) if (row.thread) rows.set(row.thread, row);
  // A thread row is priceable only when all its turns agreed on one harness
  // and model; `model_harness` is the store's answer to that, and its absence
  // leaves the row unestimated rather than guessing from the display harness.
  const costOfThread = (row: UsageRow, totals: UsageTotals): CostTotals =>
    costOf({
      harness: row.modelHarness ?? "",
      model: row.modelHarness ? row.model : undefined,
      totals,
    });
  const sessionById = new Map(sessions.map((session) => [session.id, session]));
  const parentsFromStore = report.parentsReported;

  const parent = new Map<string, string>();
  const titleOnly = new Set<string>();
  let missingSessions = 0;
  for (const [id, row] of rows) {
    if (!sessionById.has(id)) missingSessions++;
    if (parentsFromStore) {
      // The store value is the snapshot's own parent; the link book is the
      // fallback for a row written before the parent was recorded there.
      const session = sessionById.get(id);
      const above = row.parentId ?? parentOf({ id, parent: delegateParent(session?.parent) }, links);
      if (above && above !== id && rows.has(above)) parent.set(id, above);
      else if (!above && row.parentTitle) titleOnly.add(id);
      continue;
    }
    // An older engine has no parent on the row: fall back to the session list
    // and the link book, which may be capped or newer than the usage read.
    const session = sessionById.get(id);
    const startedByLink = links.some((link) => link.started && link.target === id);
    const above = parentOf({ id, parent: delegateParent(session?.parent) }, links);
    if (above && above !== id && rows.has(above)) parent.set(id, above);
    else if (delegateParent(session?.parent) && !session?.parent?.id && !startedByLink) titleOnly.add(id);
  }

  // A broken record (`a` started by `b` and `b` by `a`) must not recurse. One
  // edge of each loop is ignored, so every thread still lands in exactly one place.
  let brokenParents = 0;
  for (const start of [...parent.keys()]) {
    const seen = new Set<string>();
    let cursor: string | undefined = start;
    while (cursor !== undefined && parent.has(cursor)) {
      if (seen.has(cursor)) {
        parent.delete(start);
        brokenParents++;
        break;
      }
      seen.add(cursor);
      cursor = parent.get(cursor);
    }
  }

  const children = new Map<string, string[]>();
  for (const [id, above] of parent) {
    children.set(above, [...(children.get(above) ?? []), id]);
  }

  const rolled = new Map<string, { total: UsageTotals; delegates: number; cost: CostTotals }>();
  const visiting = new Set<string>();
  const visit = (id: string): { total: UsageTotals; delegates: number; cost: CostTotals } => {
    const known = rolled.get(id);
    if (known) return known;
    if (visiting.has(id)) return { total: emptyTotals(), delegates: 0, cost: emptyCost() };
    visiting.add(id);
    const row = rows.get(id)!;
    const own = totalsOf(row);
    let total = own;
    let delegates = 0;
    let cost = costOfThread(row, own);
    for (const child of children.get(id) ?? []) {
      const below = visit(child);
      total = addTotals(total, below.total);
      cost = addCost(cost, below.cost);
      delegates += below.delegates + 1;
    }
    visiting.delete(id);
    const answer = { total, delegates, cost };
    rolled.set(id, answer);
    return answer;
  };

  const threads = [...rows.entries()]
    .filter(([id]) => !parent.has(id))
    .map(([id, row]): ThreadTotal => {
      const session = sessionById.get(id);
      const answer = visit(id);
      const own = totalsOf(row);
      return {
        id,
        title: row.title || session?.title || id,
        harness: row.harness || session?.harness || "unknown",
        project: row.project || session?.cwd,
        own,
        total: answer.total,
        ownCost: costOfThread(row, own),
        cost: answer.cost,
        delegateCount: answer.delegates,
        unlinked: titleOnly.has(id),
      };
    })
    .sort(
      (one, other) =>
        usedTokens(other.total) - usedTokens(one.total) ||
        (other.total.cost ?? 0) - (one.total.cost ?? 0) ||
        one.title.localeCompare(other.title),
    );

  return {
    threads,
    truncated: report.truncated,
    brokenParents,
    unlinked: titleOnly.size,
    parentsFromStore,
    sessionsCapped,
    missingSessions,
    // With parents on the rows, the hierarchy and the usage figures come from
    // the same store snapshot. Falling back, a capped or incomplete session
    // list can hide a parent, so say so rather than placing a child wrongly.
    hierarchyIncomplete: !parentsFromStore && (sessionsCapped || missingSessions > 0),
  };
}
