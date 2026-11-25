// The numbers under the composer on a new thread: a contribution-style
// heatmap of the days this Mac recorded, the month's headline figures, and the
// project's recent threads. Everything here is derived from reads the app
// already makes (`_rustcode/data/usage`, the session list), and everything is
// pure — no React, no store — so the screen and its tests share one answer.

import type { UsageRow } from "../acp/rustcode";
import type { SessionSummary } from "../acp/types";
import { sameProject } from "./orchestration";
import { DAY_MS, localDay } from "./period";

/** The most weeks the activity card reads and draws: a year. */
export const HEATMAP_WEEKS = 52;

/** One day of the heatmap: what ran on it, and how dark its cell is. */
export interface HeatCell {
  day: string;
  turns: number;
  /** 0 for a day nothing ran, 4 for the busiest day in the window. */
  level: 0 | 1 | 2 | 3 | 4;
}

/** The calendar day `n` days after `day`, for the local days the store groups by. */
export function plusDays(day: string, days: number): string {
  return new Date(Date.parse(`${day}T00:00:00Z`) + days * DAY_MS).toISOString().slice(0, 10);
}

/** The column a day belongs to: Monday is 0, as a contribution graph reads. */
export function weekday(day: string): number {
  return (new Date(`${day}T00:00:00Z`).getUTCDay() + 6) % 7;
}

/** What each day in a report ran, added up over its harness and model rows. */
export function turnsPerDay(rows: readonly UsageRow[]): Map<string, number> {
  const turns = new Map<string, number>();
  for (const row of rows) {
    if (!row.day) continue;
    turns.set(row.day, (turns.get(row.day) ?? 0) + row.turns);
  }
  return turns;
}

/**
 * How many of the heatmap's week columns reach back to the first day anything ran, the week of
 * today included; 0 when nothing ran. A short history then draws its own weeks instead of a year
 * of blank cells before it.
 */
export function weeksOfHistory(turns: ReadonlyMap<string, number>, today: Date = new Date()): number {
  let first: string | undefined;
  for (const [day, value] of turns) if (value > 0 && (!first || day < first)) first = day;
  if (!first) return 0;
  const last = localDay(today);
  const days = Math.round((Date.parse(`${last}T00:00:00Z`) - Date.parse(`${first}T00:00:00Z`)) / DAY_MS);
  return Math.floor((days + 6 - weekday(last)) / 7) + 1;
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
  const last = localDay(today);
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

/** A run of days in a row with at least one turn, as the activity card counts it. */
export interface Streak {
  /** Days in a row up to today, or up to yesterday while today has nothing yet. */
  current: number;
  /** The longest run in the days read, the current one included. */
  longest: number;
  /** Whether today already counts. */
  today: boolean;
  /** The days of the current run, so the heatmap can mark them. */
  days: ReadonlySet<string>;
}

/**
 * The streak in a day-by-day report. A quiet today does not break it: the run
 * still counts up to yesterday until the day is over, so the card says what
 * keeps it going instead of showing zero every morning.
 */
export function streakOf(turns: ReadonlyMap<string, number>, today: Date = new Date()): Streak {
  const active = (day: string) => (turns.get(day) ?? 0) > 0;
  const last = localDay(today);
  const days = new Set<string>();
  const ranToday = active(last);
  for (let day = ranToday ? last : plusDays(last, -1); active(day); day = plusDays(day, -1)) days.add(day);
  let longest = days.size;
  let run = 0;
  let previous = "";
  for (const day of [...turns.keys()].filter(active).sort()) {
    run = previous && plusDays(previous, 1) === day ? run + 1 : 1;
    previous = day;
    if (run > longest) longest = run;
  }
  return { current: days.size, longest, today: ranToday, days };
}

/** The local days a month starts on: this one, and the one before it. */
export function monthBounds(now: Date = new Date()): { since: string; previousSince: string } {
  const year = now.getFullYear();
  const month = now.getMonth();
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

/** The activity card's small numbers: turns in the last seven days, today's included, and the busiest day read. */
export function activitySummary(
  turns: ReadonlyMap<string, number>,
  today: Date = new Date(),
): { week: number; busiest?: { day: string; turns: number } } {
  const from = new Date(Date.parse(`${localDay(today)}T00:00:00Z`) - 6 * DAY_MS).toISOString().slice(0, 10);
  let week = 0;
  let busiest: { day: string; turns: number } | undefined;
  for (const [day, count] of turns) {
    if (day >= from) week += count;
    if (count > 0 && (!busiest || count > busiest.turns)) busiest = { day, turns: count };
  }
  return { week, busiest };
}

/** Turns per weekday over the days read, Monday first, for the rhythm card. */
export function weekdayTurns(turns: ReadonlyMap<string, number>): number[] {
  const totals = [0, 0, 0, 0, 0, 0, 0];
  for (const [day, value] of turns) totals[weekday(day)] = (totals[weekday(day)] ?? 0) + value;
  return totals;
}

/** One group's share of a report: what it ran, and how much of the whole that is. */
export interface Share {
  key: string;
  turns: number;
  tokens: number;
  /** Of the turns in every group, 0 to 1. */
  part: number;
}

/**
 * A report's rows added up by harness or by model from `since` on, busiest first. A model row
 * that names no model is the harness's default, so it is counted under "Default model".
 */
export function sharesBy(rows: readonly UsageRow[], key: "harness" | "model", since = ""): Share[] {
  const groups = new Map<string, { turns: number; tokens: number }>();
  let all = 0;
  for (const row of rows) {
    if (since && (row.day ?? "") < since) continue;
    const name =
      (key === "model" ? row.model : row.harness) || (key === "model" ? "Default model" : "unknown");
    const group = groups.get(name) ?? { turns: 0, tokens: 0 };
    group.turns += row.turns;
    group.tokens += (row.input ?? 0) + (row.output ?? 0);
    groups.set(name, group);
    all += row.turns;
  }
  return [...groups]
    .map(([name, group]) => ({ key: name, ...group, part: all > 0 ? group.turns / all : 0 }))
    .filter((share) => share.turns > 0)
    .sort((one, other) => other.turns - one.turns || other.tokens - one.tokens);
}

/** A share as a whole percentage; a share too small to round to 1% says so instead of showing 0%. */
export function sharePercent(part: number): string {
  return part > 0 && part < 0.005 ? "<1%" : `${Math.round(part * 100)}%`;
}

/** What one stretch of days ran: its turns and its input and output tokens. */
export function periodTotals(
  rows: readonly UsageRow[],
  from: string,
  to = "9999-12-31",
): { turns: number; tokens: number } {
  let turns = 0;
  let tokens = 0;
  for (const row of rows) {
    const day = row.day ?? "";
    if (day < from || day > to) continue;
    turns += row.turns;
    tokens += (row.input ?? 0) + (row.output ?? 0);
  }
  return { turns, tokens };
}
