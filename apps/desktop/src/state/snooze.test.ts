import { describe, expect, test } from "bun:test";
import { dueSnoozes, snoozeAt, snoozePresets, wakeLabel } from "./snooze";

// A Wednesday.
const morning = new Date(2026, 10, 4, 10, 30).getTime();

describe("snooze", () => {
  test("presets are in the future, in order, and drop this evening once it has passed", () => {
    const presets = snoozePresets(morning);
    expect(presets.map((p) => p.label)).toEqual([
      "In 1 hour",
      "This evening",
      "Tomorrow morning",
      "Next week",
    ]);
    const times = presets.map((p) => p.at);
    expect(times).toEqual([...times].sort((a, b) => a - b));
    expect(new Date(presets[3]!.at).getDay()).toBe(1);
    const evening = new Date(2026, 10, 4, 20, 0).getTime();
    expect(snoozePresets(evening).map((p) => p.label)).not.toContain("This evening");
  });

  test("next week from a Monday is the Monday after", () => {
    const monday = new Date(2026, 10, 2, 10, 0).getTime();
    const next = snoozePresets(monday).at(-1)!.at;
    expect(new Date(next).getDate()).toBe(9);
  });

  test("the wake label says when in the fewest words", () => {
    expect(wakeLabel(new Date(2026, 10, 4, 18, 0).getTime(), morning)).toContain("18");
    expect(wakeLabel(new Date(2026, 10, 5, 9, 0).getTime(), morning)).toStartWith("Tomorrow");
    expect(wakeLabel(new Date(2026, 10, 20, 9, 0).getTime(), morning)).toContain("20");
  });

  test("only threads whose time has come are due", () => {
    expect(dueSnoozes({ a: morning - 1, b: morning + 1 }, morning)).toEqual(["a"]);
  });
});

describe("snoozeAt", () => {
  // A Wednesday, at 10:00 local time.
  const now = new Date(2026, 10, 4, 10, 0).getTime();
  const late = new Date(2026, 10, 4, 20, 0).getTime();

  test("reads the named choices", () => {
    expect(snoozeAt("1h", now)).toBe(now + 3_600_000);
    expect(snoozeAt("evening", now)).toBe(new Date(2026, 10, 4, 18, 0).getTime());
    expect(snoozeAt("tomorrow", now)).toBe(new Date(2026, 10, 5, 9, 0).getTime());
    expect(snoozeAt("next_week", now)).toBe(new Date(2026, 10, 9, 9, 0).getTime());
  });
  test("this evening, once it has passed, is the next one", () => {
    expect(snoozeAt("evening", late)).toBe(new Date(2026, 10, 5, 18, 0).getTime());
  });
  test("reads a time, and refuses anything else", () => {
    expect(snoozeAt("2030-01-01T00:00:00Z", now)).toBe(Date.UTC(2030, 0, 1));
    expect(snoozeAt("soonish", now)).toBeNaN();
  });
});
