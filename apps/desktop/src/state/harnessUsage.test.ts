import { describe, expect, test } from "bun:test";
import { agentWindowLabel, usageOf } from "./harnessUsage";
import type { CachedPlan } from "./store";

const NOW = 1_762_500_000_000;
const iso = (ms: number) => new Date(ms).toISOString();

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
    expect(usageOf(cached())).toEqual({
      status: "ok",
      as_of: iso(NOW),
      windows: [
        { label: "5-hour", used_percent: 48, window_minutes: 300, resets_at: iso(NOW + 4 * 3_600_000) },
        { label: "Weekly", used_percent: 12, window_minutes: 10_080 },
      ],
    });
  });

  test("numbers RustCode recorded say when they were read, not when the app heard them", () => {
    const recorded = usageOf(cached({ asOf: NOW - 3 * 3_600_000 }));
    expect(recorded?.as_of).toBe(iso(NOW - 3 * 3_600_000));
  });

  test("nothing read for the login yet says nothing at all", () => {
    expect(usageOf(undefined)).toBeUndefined();
    expect(usageOf(cached({ windows: [] }))).toBeUndefined();
  });

  test("Claude's logins say reading is off rather than look idle", () => {
    // Even a cache entry from before the switch was turned off draws nothing.
    expect(usageOf(cached(), false)).toEqual({ status: "not_enabled", windows: [] });
    expect(usageOf(undefined, false)).toEqual({ status: "not_enabled", windows: [] });
    // A harness whose plan RustCode reads without consent is unaffected.
    expect(usageOf(cached({ harness: "codex" }), true)?.status).toBe("ok");
  });

  test("a window the cache kept no name for is named by its length", () => {
    const windows = usageOf(
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
