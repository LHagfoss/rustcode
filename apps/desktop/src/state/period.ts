// The period Settings › Statistics draws (Today, 7 days, 30 days, every day
// recorded), and the numbers the page puts on top of it: the same-length period
// before, the change between the two, and one point per UTC day for a sparkline
// or the daily chart.
//
// The store groups turns by one key at a time (`_rustcode/data/usage`), so the
// period before is read as a second report that starts earlier: its rows hold
// both periods, and its rows minus the current period's are the previous one's.
// Every field stays absent when neither report reported it, so a blank is never
// shown as a zero.
//
// Pure (no React, no store at runtime), so the page and its tests share one
// answer.

import type { UsageRow, UsageTotals } from "../acp/rustcode";
import { addTotals, emptyTotals, totalsOf } from "./statistics";

export type StatsPeriod = "today" | "7d" | "30d" | "all";

/** The choices a user picks between, shortest first. */
export const STATS_PERIODS: readonly { value: StatsPeriod; label: string }[] = [
  { value: "today", label: "Today" },
  { value: "7d", label: "7 days" },
  { value: "30d", label: "30 days" },
  { value: "all", label: "All" },
];

export const DAY_MS = 86_400_000;

/** The UTC day the store groups by (`date(started_at)`), `days` days before `now`. */
export function daysAgo(now: Date, days: number): string {
  const today = Date.parse(`${now.toISOString().slice(0, 10)}T00:00:00Z`);
  return new Date(today - days * DAY_MS).toISOString().slice(0, 10);
}

/** What one period asks the store for, and the period it is compared with. */
export interface StatsRange {
  /** The first UTC day in the period; absent for every day recorded. */
  since?: string;
  /** The first UTC day of the same-length period before it; absent when there is none. */
  previousSince?: string;
  /** How many days the period holds; 0 for every day recorded. */
  days: number;
}

/**
 * The day a period starts on, and the day its comparison period starts on. A
 * period is whole UTC days, so "Today" is one day and "7 days" is today and the
 * six days before it (the store's own `since` is a day or month, compared as a
 * string against `started_at`).
 */
export function statsRange(period: StatsPeriod, now = new Date()): StatsRange {
  const days = period === "today" ? 1 : period === "7d" ? 7 : period === "30d" ? 30 : 0;
  if (!days) return { days };
  return { since: daysAgo(now, days - 1), previousSince: daysAgo(now, days * 2 - 1), days };
}

/** The UTC days a range covers, oldest first; empty for every day recorded. */
export function rangeDays(since: string | undefined, days: number): string[] {
  if (!since || days <= 0) return [];
  const start = Date.parse(`${since}T00:00:00Z`);
  return Array.from({ length: days }, (_, index) =>
    new Date(start + index * DAY_MS).toISOString().slice(0, 10),
  );
}

/** Which way a number moved, and how much. */
export interface Trend {
  direction: "up" | "down" | "level";
  /** The change in percent; absent when the period before had nothing to compare. */
  percent?: number;
}

/**
 * How `current` changed from `previous`. Unknown on either side is no trend at
 * all; a period that recorded nothing before is up when something is there now,
 * with no percentage to show. Under half a percent reads as level, because the
 * tile would round it to `0%`.
 */
export function trend(current: number | undefined, previous: number | undefined): Trend | undefined {
  if (current === undefined || previous === undefined) return undefined;
  if (previous === 0) return { direction: current > 0 ? "up" : "level" };
  const percent = ((current - previous) / previous) * 100;
  const direction = Math.abs(percent) < 0.5 ? "level" : percent > 0 ? "up" : "down";
  return { direction, percent };
}

/** A count the wider window holds minus the narrower one: absent stays absent. */
function subtract(wider: number | undefined, narrower: number | undefined): number | undefined {
  if (wider === undefined) return undefined;
  if (narrower === undefined) return wider;
  return Math.max(0, wider - narrower);
}

const rowKey = (row: UsageRow): string => `${row.harness}\u0000${row.model ?? ""}`;

/**
 * The rows of the period before. `wider` was read from `previousSince`, so it
 * holds that period and the current one; `current` holds the current one. A row
 * missing from `current` was entirely in the period before.
 */
export function previousRows(wider: readonly UsageRow[], current: readonly UsageRow[]): UsageRow[] {
  const now = new Map(current.map((row) => [rowKey(row), row]));
  const rows: UsageRow[] = [];
  for (const row of wider) {
    const sub = now.get(rowKey(row));
    if (!sub) {
      rows.push(row);
      continue;
    }
    const next: UsageRow = {
      ...row,
      input: subtract(row.input, sub.input),
      output: subtract(row.output, sub.output),
      cached: subtract(row.cached, sub.cached),
      cacheWrite: subtract(row.cacheWrite, sub.cacheWrite),
      cost: subtract(row.cost, sub.cost),
      turns: Math.max(0, row.turns - sub.turns),
      turnsWithoutTokens: Math.max(0, row.turnsWithoutTokens - sub.turnsWithoutTokens),
      turnsWithoutAnyTokens: Math.max(0, row.turnsWithoutAnyTokens - sub.turnsWithoutAnyTokens),
      turnsWithoutInputTokens: Math.max(0, row.turnsWithoutInputTokens - sub.turnsWithoutInputTokens),
      turnsWithoutOutputTokens: Math.max(0, row.turnsWithoutOutputTokens - sub.turnsWithoutOutputTokens),
      turnsWithoutCachedTokens: Math.max(0, row.turnsWithoutCachedTokens - sub.turnsWithoutCachedTokens),
      turnsWithoutCacheWriteTokens: Math.max(
        0,
        row.turnsWithoutCacheWriteTokens - sub.turnsWithoutCacheWriteTokens,
      ),
      turnsWithoutCost: Math.max(0, row.turnsWithoutCost - sub.turnsWithoutCost),
    };
    // A model the current period used is not a row of the period before.
    if (next.turns > 0) rows.push(next);
  }
  return rows;
}

/** A report's rows added up, the way the engine totals them. */
export function sumRows(rows: readonly UsageRow[]): UsageTotals {
  let total = emptyTotals();
  for (const row of rows) total = addTotals(total, totalsOf(row));
  return total;
}

/** One UTC day of the page: what was recorded that day. */
export interface DayPoint {
  day: string;
  turns: number;
  /** Input and output tokens that day; absent when no turn reported either. */
  tokens?: number;
  /** The USD the harnesses reported that day. Never an estimate. */
  cost?: number;
}

const sumOptional = (values: readonly (number | undefined)[]): number | undefined => {
  let total = 0;
  let any = false;
  for (const value of values) {
    if (value === undefined) continue;
    any = true;
    total += value;
  }
  return any ? total : undefined;
};

/**
 * One point per day in `days`, or every day the rows name when `days` is empty
 * (the page's "All"). Days the store recorded nothing for are still a point, so
 * a gap stays a gap on the chart.
 */
export function dailySeries(rows: readonly UsageRow[], days: readonly string[]): DayPoint[] {
  const byDay = new Map<string, UsageRow[]>();
  for (const row of rows) {
    if (!row.day) continue;
    byDay.set(row.day, [...(byDay.get(row.day) ?? []), row]);
  }
  const keys = days.length ? [...days] : [...byDay.keys()].sort();
  return keys.map((day) => {
    const of = byDay.get(day) ?? [];
    return {
      day,
      turns: of.reduce((sum, row) => sum + row.turns, 0),
      tokens: sumOptional(
        of.map((row) =>
          row.input === undefined && row.output === undefined
            ? undefined
            : (row.input ?? 0) + (row.output ?? 0),
        ),
      ),
      cost: sumOptional(of.map((row) => row.cost)),
    };
  });
}

/**
 * A tile's sparkline values. A day nothing was reported for is a zero, never a
 * gap: the line has to span the period, or a store whose cost was reported on
 * two days and blank on the rest would draw a dash at one edge. The exact
 * figures and their caveats are in the tile's value and note, not the line.
 */
export function sparklineSeries(values: readonly (number | undefined)[]): number[] {
  return values.map((value) => value ?? 0);
}

/** A path over the values that are known, at `width` × `height`; empty when none is. */
export function sparklinePath(
  values: readonly (number | undefined)[],
  width: number,
  height: number,
): string {
  const known = values.filter((value): value is number => value !== undefined);
  if (!known.length || width <= 0 || height <= 0) return "";
  const peak = Math.max(...known);
  const count = values.length;
  const x = (index: number) => (count <= 1 ? width : (index / (count - 1)) * width);
  // Half a pixel inside the box, so the stroke is not cut in half at the edges.
  const y = (value: number) => (peak > 0 ? height - 0.5 - (value / peak) * (height - 1) : height - 0.5);
  const at = (value: number) => value.toFixed(2);
  let path = "";
  let pen = false;
  values.forEach((value, index) => {
    if (value === undefined) {
      // A day with nothing recorded breaks the line instead of drawing through 0.
      pen = false;
      return;
    }
    path += `${pen ? "L" : "M"}${at(x(index))} ${at(y(value))}`;
    pen = true;
  });
  return path;
}
