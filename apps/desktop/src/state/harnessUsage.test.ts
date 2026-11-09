import { describe, expect, test } from "bun:test";
import { agentWindowLabel, usageOf } from "./harnessUsage";
import type { CachedPlan } from "./store";

const NOW = 1_762_500_000_000;
const iso = (ms: number) => new Date(ms).toISOString();

/** A login's block as an agent reads it at `NOW`, whatever the real clock says. */
const at = (plan: Parameters<typeof usageOf>[0]) => usageOf(plan, NOW);

/** A login's cached plan as a plans answer leaves it. */
const cached = (over: Partial<CachedPlan> = {}): CachedPlan => ({
  source: "claude-code",
  harness: "claude-code",
  account: null,
  name: "Claude",
  plan: "max",
  windows: [
    { label: "5-hour", usedPercent: 48, minutes: 300, resetsAt: NOW + 4 * 3_600_000 },
    { label: "Weekly", usedPercent: 12, minutes: 10_080 },
  ],
  fetchedAt: NOW,
  ...over,
});

describe("a login's usage as an agent reads it", () => {
  test("the windows, their labels and when they reset", () => {
    expect(at(cached())).toEqual({
      status: "ok",
      as_of: iso(NOW),
      windows: [
        { label: "5-hour", used_percent: 48, window_minutes: 300, resets_at: iso(NOW + 4 * 3_600_000) },
        { label: "Weekly", used_percent: 12, window_minutes: 10_080 },
      ],
    });
  });

  test("numbers RustCode recorded say when they were read, not when the app heard them", () => {
    const recorded = at(cached({ asOf: NOW - 3 * 3_600_000 }));
    expect(recorded?.as_of).toBe(iso(NOW - 3 * 3_600_000));
  });

  test("nothing read for the login yet says nothing at all", () => {
    expect(at(undefined)).toBeUndefined();
    expect(at(cached({ windows: [] }))).toBeUndefined();
  });

  test("a window whose reset passed reports nothing used, as the composer draws it", () => {
    // The numbers were read before the 5-hour window started over: the window is
    // fresh again, and its old reset time is no answer for an agent.
    const lapsed = at(
      cached({
        windows: [
          { label: "5-hour", usedPercent: 48, minutes: 300, resetsAt: NOW - 60_000 },
          { label: "Weekly", usedPercent: 12, minutes: 10_080, resetsAt: NOW + 3_600_000 },
        ],
      }),
    );
    expect(lapsed?.windows).toEqual([
      { label: "5-hour", used_percent: 0, window_minutes: 300 },
      { label: "Weekly", used_percent: 12, window_minutes: 10_080, resets_at: iso(NOW + 3_600_000) },
    ]);
    // Folding a reset does not make the read younger than it was.
    expect(lapsed?.as_of).toBe(iso(NOW));
  });

  test("a window the cache kept no name for is named by its length", () => {
    const windows = at(
      cached({
        windows: [
          { usedPercent: 71, minutes: 300 },
          { usedPercent: 4, minutes: 10_080 },
          { usedPercent: 1, minutes: 1_440 },
          { usedPercent: 0 },
        ],
      }),
    )?.windows;
    expect(windows?.map((w) => w.label)).toEqual(["5-hour", "Weekly", "Daily", "Usage"]);
    expect(windows?.[3]).toEqual({ label: "Usage", used_percent: 0 });
    expect(agentWindowLabel(undefined)).toBe("Usage");
    expect(agentWindowLabel(60)).toBe("Usage");
  });
});
