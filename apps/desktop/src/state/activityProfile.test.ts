import { describe, expect, test } from "bun:test";
import type { UsageRow } from "../acp/rustcode";
import { monthLabels, profileSummary, profileWeeks, shortTokens, tokensPerDay } from "./activityProfile";

const row = (over: Partial<UsageRow> = {}): UsageRow => ({
  harness: "codex",
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

const today = new Date("2026-06-10T12:00:00");

describe("the activity profile summary", () => {
  const rows = [
    row({ day: "2026-06-10", input: 100, output: 10, turns: 2, longestTurnMs: 60_000 }),
    row({ day: "2026-06-09", input: 900, output: 100, turns: 3, longestTurnMs: 3_600_000 }),
    row({ day: "2026-06-08", input: 50, turns: 1 }),
    row({ day: "2026-05-01", input: 10, turns: 1 }),
    row({ day: "2026-05-02", input: 10, turns: 1 }),
    row({ day: "2026-05-03", input: 10, turns: 1 }),
    row({ day: "2026-05-04", input: 10, turns: 1 }),
  ];

  test("adds up every day's input and output tokens and turns", () => {
    const summary = profileSummary(rows, today);
    expect(summary.lifetimeTokens).toBe(1200);
    expect(summary.lifetimeTurns).toBe(10);
    expect(summary.activeDays).toBe(7);
    expect(summary.since).toBe("2026-05-01");
  });

  test("the peak is the day with the most tokens", () => {
    expect(profileSummary(rows, today).peak).toEqual({ day: "2026-06-09", tokens: 1000 });
  });

  test("the longest turn is the longest any day reported", () => {
    expect(profileSummary(rows, today).longestTurnMs).toBe(3_600_000);
    expect(profileSummary([row({ day: "2026-06-10" })], today).longestTurnMs).toBeUndefined();
  });

  test("streaks run over every recorded day, not a window", () => {
    const { streak } = profileSummary(rows, today);
    expect(streak.current).toBe(3);
    expect(streak.longest).toBe(4);
    expect(streak.today).toBe(true);
  });

  test("an empty store has no peak and no streak", () => {
    const summary = profileSummary([], today);
    expect(summary.lifetimeTokens).toBe(0);
    expect(summary.peak).toBeUndefined();
    expect(summary.streak.current).toBe(0);
  });
});

describe("the activity profile grid", () => {
  test("a year of week columns ending on today", () => {
    const weeks = profileWeeks([], "daily", 53, today);
    expect(weeks).toHaveLength(53);
    expect(
      weeks
        .at(-1)!
        .slice(2)
        .map((cell) => cell?.day ?? null),
    ).toEqual(["2026-06-10", null, null, null, null]);
  });

  test("daily shades each day by its own tokens", () => {
    const weeks = profileWeeks(
      [row({ day: "2026-06-08", input: 100 }), row({ day: "2026-06-09", input: 25 })],
      "daily",
      1,
      today,
    );
    expect(weeks[0]!.slice(0, 3).map((cell) => cell?.level)).toEqual([4, 1, 0]);
  });

  test("a day with turns but no reported tokens still shows", () => {
    const weeks = profileWeeks([row({ day: "2026-06-09", turns: 2 })], "daily", 1, today);
    expect(weeks[0]![1]?.level).toBe(1);
  });

  test("weekly shades every day of a week alike", () => {
    const weeks = profileWeeks([row({ day: "2026-06-02", input: 40 })], "weekly", 2, today);
    expect(weeks[0]!.map((cell) => cell?.level)).toEqual([4, 4, 4, 4, 4, 4, 4]);
    expect(weeks[1]!.slice(0, 3).map((cell) => cell?.level)).toEqual([0, 0, 0]);
  });

  test("cumulative carries what ran before the grid and never falls", () => {
    const weeks = profileWeeks(
      [row({ day: "2020-01-01", input: 100 }), row({ day: "2026-06-09", input: 100 })],
      "cumulative",
      1,
      today,
    );
    expect(weeks[0]!.slice(0, 3).map((cell) => cell?.value)).toEqual([100, 200, 200]);
    expect(weeks[0]![0]?.level).toBe(2);
    expect(weeks[0]![2]?.level).toBe(4);
  });

  test("tokens per day adds rows of the same day", () => {
    expect(
      tokensPerDay([row({ day: "2026-06-09", input: 1 }), row({ day: "2026-06-09", output: 2 })]),
    ).toEqual(new Map([["2026-06-09", 3]]));
  });
});

describe("the activity profile month labels", () => {
  test("a label on the first week of each month, none on a partial first column", () => {
    const labels = monthLabels(profileWeeks([], "daily", 9, today));
    expect(labels.filter(Boolean)).toEqual(["May", "Jun"]);
    expect(labels[0]).toBe("");
  });
});

describe("short token counts", () => {
  test("billions, millions and thousands at three figures", () => {
    expect(shortTokens(22_800_000_000)).toBe("22.8B");
    expect(shortTokens(1_300_000_000)).toBe("1.3B");
    expect(shortTokens(940_400_000)).toBe("940M");
    expect(shortTokens(12_000)).toBe("12k");
    expect(shortTokens(512)).toBe("512");
  });
});
