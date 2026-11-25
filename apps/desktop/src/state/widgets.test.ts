// What the new-thread screen's widgets draw: the heatmap's columns and shades,
// a month's bounds, and a project's threads (state/widgets.ts).

import { describe, expect, test } from "bun:test";
import type { UsageRow } from "../acp/rustcode";
import type { SessionSummary } from "../acp/types";
import {
  activitySummary,
  heatmapWeeks,
  monthBounds,
  periodTotals,
  projectThreads,
  sharesBy,
  streakOf,
  turnsPerDay,
  weekdayTurns,
} from "./widgets";

const row = (over: Partial<UsageRow> = {}): UsageRow => ({
  harness: "codex",
  model: "gpt-6-luna",
  turns: 1,
  turnsWithoutTokens: 0,
  turnsWithoutAnyTokens: 0,
  turnsWithoutInputTokens: 0,
  turnsWithoutOutputTokens: 0,
  turnsWithoutCachedTokens: 0,
  turnsWithoutCacheWriteTokens: 0,
  turnsWithoutCost: 1,
  ...over,
});

const session = (over: Partial<SessionSummary> = {}): SessionSummary => ({
  id: "s1",
  cwd: "/Users/ada/code/rustcode",
  ...over,
});

describe("the activity heatmap", () => {
  // A Wednesday: the last column holds three days, so the days after today are blank.
  const today = new Date("2026-06-10T12:00:00Z");

  test("one column per week, Monday first, ending on the week of today", () => {
    const weeks = heatmapWeeks(new Map(), 4, today);
    expect(weeks).toHaveLength(4);
    expect(weeks.map((week) => week.length)).toEqual([7, 7, 7, 7]);
    // 4 weeks back from Wednesday 10 June is Monday 18 May.
    expect(weeks[0]![0]?.day).toBe("2026-05-18");
    // Monday and Tuesday of this week are recorded; Wednesday is today; the rest is not yet.
    expect(weeks[3]!.slice(0, 3).map((cell) => cell?.day)).toEqual([
      "2026-06-08",
      "2026-06-09",
      "2026-06-10",
    ]);
    expect(weeks[3]!.slice(3)).toEqual([null, null, null, null]);
  });

  test("a day's shade is its share of the busiest day, in four steps", () => {
    const turns = new Map([
      ["2026-06-08", 40],
      ["2026-06-09", 10],
      ["2026-06-10", 1],
    ]);
    const last = heatmapWeeks(turns, 1, today)[0]!;
    expect(last.slice(0, 3).map((cell) => cell?.level)).toEqual([4, 1, 1]);
    // A day nothing ran is the quiet shade, never a step.
    expect(last[0]?.turns).toBe(40);
    expect(heatmapWeeks(new Map(), 1, today)[0]![0]?.level).toBe(0);
  });

  test("a report's rows are added up per day, across harnesses and models", () => {
    const turns = turnsPerDay([
      row({ day: "2026-06-09", turns: 2 }),
      row({ day: "2026-06-09", harness: "claude", turns: 3 }),
      row({ day: "2026-06-10", turns: 1 }),
      row({}),
    ]);
    expect(turns.get("2026-06-09")).toBe(5);
    expect(turns.get("2026-06-10")).toBe(1);
    expect(turns.size).toBe(2);
  });
});

describe("a month's bounds", () => {
  test("the first day of this month and of the one before", () => {
    expect(monthBounds(new Date("2026-06-10T12:00:00Z"))).toEqual({
      since: "2026-06-01",
      previousSince: "2026-05-01",
    });
  });

  test("January compares with the December before it", () => {
    expect(monthBounds(new Date("2026-01-04T00:00:00Z"))).toEqual({
      since: "2026-01-01",
      previousSince: "2025-12-01",
    });
  });
});

describe("a project's threads", () => {
  test("its own folder and the worktrees cut from it, newest first", () => {
    const threads = projectThreads(
      [
        session({ id: "old", updatedAt: 100 }),
        session({ id: "new", updatedAt: 300 }),
        session({
          id: "worktree",
          cwd: "/tmp/rustcode-1",
          worktree: { path: "/tmp/rustcode-1", branch: "feat/x", repo: "/Users/ada/code/rustcode" },
          updatedAt: 200,
        }),
        session({ id: "elsewhere", cwd: "/Users/ada/code/other", updatedAt: 400 }),
      ],
      "/Users/ada/code/rustcode/",
    );
    expect(threads.map((thread) => thread.id)).toEqual(["new", "worktree", "old"]);
  });

  test("no project chosen has no threads", () => {
    expect(projectThreads([session()], "")).toEqual([]);
  });
});

describe("the activity streak", () => {
  const today = new Date("2026-06-10T12:00:00Z");
  const days = (...entries: [string, number][]) => new Map(entries);

  test("counts the days in a row up to today", () => {
    const streak = streakOf(days(["2026-06-08", 2], ["2026-06-09", 1], ["2026-06-10", 4]), today);
    expect(streak).toMatchObject({ current: 3, longest: 3, today: true });
    expect([...streak.days].sort()).toEqual(["2026-06-08", "2026-06-09", "2026-06-10"]);
  });

  test("a quiet today keeps yesterday's run alive", () => {
    expect(streakOf(days(["2026-06-08", 1], ["2026-06-09", 1]), today)).toMatchObject({
      current: 2,
      today: false,
    });
  });

  test("a missed day ends the run, and the longest one is remembered", () => {
    const streak = streakOf(
      days(["2026-05-01", 1], ["2026-05-02", 1], ["2026-05-03", 1], ["2026-05-04", 1], ["2026-06-08", 1]),
      today,
    );
    expect(streak).toMatchObject({ current: 0, longest: 4, today: false });
  });

  test("a day with no turns does not count", () => {
    expect(streakOf(days(["2026-06-09", 0], ["2026-06-10", 1]), today)).toMatchObject({
      current: 1,
      longest: 1,
    });
  });
});

describe("activitySummary", () => {
  test("counts the last seven days and finds the busiest day", () => {
    const turns = new Map([
      ["2026-11-25", 3],
      ["2026-11-19", 2],
      ["2026-11-18", 9],
      ["2026-10-01", 4],
    ]);
    expect(activitySummary(turns, new Date("2026-11-25T12:00:00Z"))).toEqual({
      week: 5,
      busiest: { day: "2026-11-18", turns: 9 },
    });
    expect(activitySummary(new Map())).toEqual({ week: 0, busiest: undefined });
  });
});

describe("the dashboard's splits", () => {
  test("adds turns up by weekday, Monday first", () => {
    const turns = new Map([
      ["2026-11-23", 3],
      ["2026-11-30", 2],
      ["2026-11-29", 5],
    ]);
    expect(weekdayTurns(turns)).toEqual([5, 0, 0, 0, 0, 0, 5]);
  });

  test("shares by harness or model, busiest first, from a day on", () => {
    const rows = [
      row({ day: "2026-11-01", harness: "codex", turns: 9, input: 10, output: 5 }),
      row({ day: "2026-11-20", harness: "codex", turns: 1, input: 1, output: 1 }),
      row({ day: "2026-11-21", harness: "claude-code", model: undefined, turns: 3 }),
    ];
    expect(sharesBy(rows, "harness", "2026-11-10")).toEqual([
      { key: "claude-code", turns: 3, tokens: 0, part: 0.75 },
      { key: "codex", turns: 1, tokens: 2, part: 0.25 },
    ]);
    expect(sharesBy(rows, "model").map((share) => [share.key, share.turns])).toEqual([
      ["gpt-6-luna", 10],
      ["Default model", 3],
    ]);
  });

  test("totals a stretch of days, both ends included", () => {
    const rows = [
      row({ day: "2026-11-01", turns: 2, input: 4 }),
      row({ day: "2026-11-02", turns: 3, output: 6 }),
      row({ day: "2026-11-03", turns: 7 }),
    ];
    expect(periodTotals(rows, "2026-11-01", "2026-11-02")).toEqual({ turns: 5, tokens: 10 });
    expect(periodTotals(rows, "2026-11-03")).toEqual({ turns: 7, tokens: 0 });
  });
});
