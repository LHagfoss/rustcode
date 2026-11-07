// What the new-thread screen's widgets draw: the heatmap's columns and shades,
// a month's bounds, and a project's threads (state/widgets.ts).

import { describe, expect, test } from "bun:test";
import type { UsageRow } from "../acp/rustcode";
import type { SessionSummary } from "../acp/types";
import { heatmapWeeks, monthBounds, projectThreads, turnsPerDay } from "./widgets";

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

test("the overview defaults closed and persists its choice", async () => {
  const { overviewExpanded, setOverviewExpanded } = await import("./widgets");
  const scope = globalThis as typeof globalThis & { localStorage: Storage };
  const previous = scope.localStorage;
  const values = new Map<string, string>();
  scope.localStorage = {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => {
      values.set(key, value);
    },
  } as Storage;
  try {
    expect(overviewExpanded()).toBe(false);
    setOverviewExpanded(true);
    expect(overviewExpanded()).toBe(true);
    setOverviewExpanded(false);
    expect(overviewExpanded()).toBe(false);
  } finally {
    if (previous) scope.localStorage = previous;
    else Reflect.deleteProperty(scope, "localStorage");
  }
});
