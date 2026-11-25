/**
 * Settings › Statistics › Your activity: lifetime figures and a year of days,
 * read from one all-time `_rustcode/data/usage` report grouped by day. Pure, so
 * the section and its tests share one answer.
 */

import type { UsageRow } from "../acp/rustcode";
import { usedTokens } from "./statistics";
import { plusDays, type Streak, streakOf, turnsPerDay, weekday } from "./widgets";

/** A year of columns, so the grid ends on this week and starts a year back. */
export const PROFILE_WEEKS = 53;

/** How the grid shades a day: its own tokens, its week's, or everything up to it. */
export type ProfileMode = "daily" | "weekly" | "cumulative";

export const PROFILE_MODES: readonly { value: ProfileMode; label: string }[] = [
  { value: "daily", label: "Daily" },
  { value: "weekly", label: "Weekly" },
  { value: "cumulative", label: "Cumulative" },
];

/** One day as the profile grid draws it. */
export interface ProfileCell {
  day: string;
  tokens: number;
  turns: number;
  /** What the mode shades by: the day's tokens, its week's, or the running total. */
  value: number;
  /** 0 for nothing, 4 for the most in the year shown. */
  level: 0 | 1 | 2 | 3 | 4;
}

/** The lifetime figures above the grid. */
export interface ProfileSummary {
  lifetimeTokens: number;
  lifetimeTurns: number;
  /** The day with the most tokens, or absent when no day reported any. */
  peak?: { day: string; tokens: number };
  /** The longest single turn's wall time; absent when no turn measured one. */
  longestTurnMs?: number;
  streak: Streak;
  activeDays: number;
  /** The first day anything ran. */
  since?: string;
}

/** Input and output tokens per UTC day, added over every row of that day. */
export function tokensPerDay(rows: readonly UsageRow[]): Map<string, number> {
  const tokens = new Map<string, number>();
  for (const row of rows) {
    if (!row.day) continue;
    tokens.set(row.day, (tokens.get(row.day) ?? 0) + usedTokens(row));
  }
  return tokens;
}

export function profileSummary(rows: readonly UsageRow[], today: Date = new Date()): ProfileSummary {
  const turns = turnsPerDay(rows);
  const tokens = tokensPerDay(rows);
  let lifetimeTokens = 0;
  let peak: ProfileSummary["peak"];
  for (const [day, value] of tokens) {
    lifetimeTokens += value;
    if (value > 0 && (!peak || value > peak.tokens)) peak = { day, tokens: value };
  }
  let longestTurnMs: number | undefined;
  for (const row of rows) {
    if (
      row.longestTurnMs !== undefined &&
      (longestTurnMs === undefined || row.longestTurnMs > longestTurnMs)
    ) {
      longestTurnMs = row.longestTurnMs;
    }
  }
  const active = [...turns].filter(([, value]) => value > 0).map(([day]) => day);
  return {
    lifetimeTokens,
    lifetimeTurns: [...turns.values()].reduce((sum, value) => sum + value, 0),
    peak,
    longestTurnMs,
    streak: streakOf(turns, today),
    activeDays: active.length,
    since: active.sort()[0],
  };
}

function levelOf(value: number, most: number, ran: boolean): ProfileCell["level"] {
  if (value <= 0 || most <= 0) return ran ? 1 : 0;
  return Math.max(1, Math.min(4, Math.ceil((value / most) * 4))) as ProfileCell["level"];
}

/**
 * The grid's columns, one per week, Monday first and ending with this week.
 * Days after today are `null`. A day that ran turns without reporting tokens
 * still gets the lightest shade, so the grid agrees with the streak.
 */
export function profileWeeks(
  rows: readonly UsageRow[],
  mode: ProfileMode,
  weeks: number = PROFILE_WEEKS,
  today: Date = new Date(),
): (ProfileCell | null)[][] {
  if (weeks <= 0) return [];
  const tokens = tokensPerDay(rows);
  const turns = turnsPerDay(rows);
  const last = today.toISOString().slice(0, 10);
  const first = plusDays(last, -((weeks - 1) * 7 + weekday(last)));
  let running = 0;
  for (const [day, value] of tokens) if (day < first) running += value;
  const columns: (ProfileCell | null)[][] = [];
  for (let week = 0; week < weeks; week++) {
    const column: (ProfileCell | null)[] = [];
    let weekTokens = 0;
    for (let day = 0; day < 7; day++) weekTokens += tokens.get(plusDays(first, week * 7 + day)) ?? 0;
    for (let day = 0; day < 7; day++) {
      const key = plusDays(first, week * 7 + day);
      if (key > last) {
        column.push(null);
        continue;
      }
      const dayTokens = tokens.get(key) ?? 0;
      running += dayTokens;
      const value = mode === "daily" ? dayTokens : mode === "weekly" ? weekTokens : running;
      column.push({ day: key, tokens: dayTokens, turns: turns.get(key) ?? 0, value, level: 0 });
    }
    columns.push(column);
  }
  let most = 0;
  for (const column of columns) for (const cell of column) if (cell && cell.value > most) most = cell.value;
  for (const column of columns) {
    for (const cell of column) {
      if (cell) cell.level = levelOf(cell.value, most, mode !== "cumulative" && cell.turns > 0);
    }
  }
  return columns;
}

/** Each column's month label: the short month name on the first week that starts in it, else empty. */
export function monthLabels(columns: readonly (readonly (ProfileCell | null)[])[]): string[] {
  let previous = "";
  return columns.map((column, index) => {
    const day = column.find(Boolean)?.day;
    if (!day) return "";
    const month = day.slice(0, 7);
    if (month === previous) return "";
    previous = month;
    if (index === 0 && Number(day.slice(8, 10)) > 7) return "";
    return new Date(`${day}T00:00:00Z`).toLocaleDateString("en-US", { month: "short", timeZone: "UTC" });
  });
}

/** A token count in three significant figures at most: 22.8B, 1.3B, 940M, 12k. */
export function shortTokens(n: number): string {
  const units: [number, string][] = [
    [1e9, "B"],
    [1e6, "M"],
    [1e3, "k"],
  ];
  for (const [size, unit] of units) {
    if (n >= size) {
      const value = n / size;
      return `${value >= 100 ? Math.round(value) : value.toFixed(1).replace(/\.0$/, "")}${unit}`;
    }
  }
  return String(Math.round(n));
}
