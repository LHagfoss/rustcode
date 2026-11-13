// The numbers under the composer on a new thread: a contribution-style
// heatmap of the days this Mac recorded, the month's headline figures, and the
// project's recent threads. Everything here is derived from reads the app
// already makes (`_rustcode/data/usage`, the session list), and everything is
// pure — no React, no store — so the screen and its tests share one answer.

import type { UsageRow } from "../acp/rustcode";
import type { SessionSummary } from "../acp/types";
import { readFlag, writeFlag } from "../lib/storage";
import { sameProject } from "./orchestration";
import { DAY_MS } from "./period";

/** How many weeks the activity card draws: about four months, in one row of columns. */
export const HEATMAP_WEEKS = 17;

/** One day of the heatmap: what ran on it, and how dark its cell is. */
export interface HeatCell {
  day: string;
  turns: number;
  /** 0 for a day nothing ran, 4 for the busiest day in the window. */
  level: 0 | 1 | 2 | 3 | 4;
}

/** The UTC day `n` days after `day`; the store groups by the same UTC day. */
function plusDays(day: string, days: number): string {
  return new Date(Date.parse(`${day}T00:00:00Z`) + days * DAY_MS).toISOString().slice(0, 10);
}

/** The column a day belongs to: Monday is 0, as a contribution graph reads. */
function weekday(day: string): number {
  return (new Date(`${day}T00:00:00Z`).getUTCDay() + 6) % 7;
}

/** What each UTC day in a report ran, added up over its harness and model rows. */
export function turnsPerDay(rows: readonly UsageRow[]): Map<string, number> {
  const turns = new Map<string, number>();
  for (const row of rows) {
    if (!row.day) continue;
    turns.set(row.day, (turns.get(row.day) ?? 0) + row.turns);
  }
  return turns;
}

/**
 * The heatmap's columns, one per week, Monday first and ending with the week
 * today is in. Days after today are `null`, so the grid stops at today instead
 * of running to the end of the week. A day's shade is its share of the busiest
 * day in the window, in four steps, so a quiet fortnight still shows its shape.
 */
export function heatmapWeeks(
  turns: ReadonlyMap<string, number>,
  weeks: number,
  today: Date = new Date(),
): (HeatCell | null)[][] {
  if (weeks <= 0) return [];
  const last = today.toISOString().slice(0, 10);
  const first = plusDays(last, -((weeks - 1) * 7 + weekday(last)));
  let busiest = 0;
  for (const value of turns.values()) if (value > busiest) busiest = value;
  const columns: (HeatCell | null)[][] = [];
  for (let week = 0; week < weeks; week++) {
    const column: (HeatCell | null)[] = [];
    for (let day = 0; day < 7; day++) {
      const key = plusDays(first, week * 7 + day);
      if (key > last) {
        column.push(null);
        continue;
      }
      const value = turns.get(key) ?? 0;
      const level =
        value <= 0 || busiest <= 0 ? 0 : (Math.min(4, Math.ceil((value / busiest) * 4)) as 1 | 2 | 3 | 4);
      column.push({ day: key, turns: value, level });
    }
    columns.push(column);
  }
  return columns;
}

/** The UTC days a month starts on: this one, and the one before it. */
export function monthBounds(now: Date = new Date()): { since: string; previousSince: string } {
  const year = now.getUTCFullYear();
  const month = now.getUTCMonth();
  const day = (at: number) => new Date(Date.UTC(year, at, 1)).toISOString().slice(0, 10);
  return { since: day(month), previousSince: day(month - 1) };
}

/**
 * The threads that ran in a project, newest first. A worktree thread belongs to
 * the repository it was cut from, not to its own folder, so a project's threads
 * are the ones that started in it and the worktrees under it.
 */
export function projectThreads(sessions: readonly SessionSummary[], project: string): SessionSummary[] {
  if (!project) return [];
  return sessions
    .filter((session) => sameProject(session.worktree?.repo || session.cwd, project))
    .sort((one, other) => (other.updatedAt ?? 0) - (one.updatedAt ?? 0));
}

/** The overview is opt-in and remembers its last setting on this device. */
const OVERVIEW_KEY = "rc-new-thread-overview";
export function overviewExpanded(): boolean {
  return readFlag(OVERVIEW_KEY, false);
}

export function setOverviewExpanded(expanded: boolean): void {
  // Storage can be unavailable; the view still works for this session.
  writeFlag(OVERVIEW_KEY, expanded);
}
