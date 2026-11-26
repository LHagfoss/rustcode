import { describe, expect, test } from "bun:test";
import { boundedStamp, MAX_CLOCK_SKEW_MS, renameStamp } from "./machineLabels";

const NOW = Date.parse("2026-11-13T10:00:00.000Z");

describe("rename stamps", () => {
  test("a stamp far in the future is taken as the allowed skew ahead", () => {
    expect(boundedStamp("2036-11-13T10:00:00.000Z", NOW)).toBe(
      new Date(NOW + MAX_CLOCK_SKEW_MS).toISOString(),
    );
    expect(boundedStamp("2026-11-13T09:00:00.000Z", NOW)).toBe("2026-11-13T09:00:00.000Z");
  });

  test("a rename made now is stamped after the one it replaces", () => {
    expect(renameStamp(null, NOW)).toBe("2026-11-13T10:00:00.000Z");
    expect(renameStamp("2026-11-13T09:00:00.000Z", NOW)).toBe("2026-11-13T10:00:00.000Z");
    expect(renameStamp("2026-11-13T10:04:00.000Z", NOW)).toBe("2026-11-13T10:04:00.001Z");
  });
});
