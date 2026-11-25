import { expect, test } from "bun:test";
import { paceLabel, planPace } from "./planPace";

const HOUR = 3_600_000;
const now = new Date(2026, 10, 25, 12, 0).getTime();
const fiveHour = (usedPercent: number, hoursIn: number) => ({
  usedPercent,
  minutes: 300,
  resetsAt: now + (5 - hoursIn) * HOUR,
});

test("a window filling faster than time passes runs out before its reset", () => {
  const pace = planPace(fiveHour(80, 2), now);
  expect(pace).toEqual({ kind: "runsOut", at: now + 0.5 * HOUR, evenLeft: 60 });
  expect(paceLabel(pace!, now)).toMatch(/^On pace to run out at \d{2}[:.]30/);
});

test("a window filling slower than time passes says what it reaches at the reset", () => {
  const pace = planPace(fiveHour(20, 2.5), now);
  expect(pace).toEqual({ kind: "atReset", percent: 40, evenLeft: 50 });
  expect(paceLabel(pace!, now)).toBe("On pace for 40% used at the reset");
});

test("no pace while the window is young, unused, spent or without a length", () => {
  expect(planPace(fiveHour(9, 0.25), now)).toBeUndefined();
  expect(planPace(fiveHour(0, 3), now)).toBeUndefined();
  expect(planPace(fiveHour(100, 3), now)).toBeUndefined();
  expect(planPace({ usedPercent: 40 }, now)).toBeUndefined();
  expect(planPace({ usedPercent: 40, minutes: 300, resetsAt: now - HOUR }, now)).toBeUndefined();
});

test("a run-out on another day names the weekday", () => {
  const weekly = { usedPercent: 50, minutes: 10080, resetsAt: now + 5 * 24 * HOUR };
  const pace = planPace(weekly, now);
  expect(pace?.kind).toBe("runsOut");
  expect(paceLabel(pace!, now)).toMatch(/^On pace to run out \p{L}{2,}\.? \d/u);
});
