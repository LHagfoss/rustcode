// Timed approvals as the panel reads them.

import { describe, expect, test } from "bun:test";
import { activeGrants, grantActive, grantRemaining, parseGrants, type TimedGrant } from "./grants";

const NOW = 1_760_000_000_000; // ms; a fixed clock keeps the strings still
const at = (secondsFromNow: number): TimedGrant => ({
  id: "prefix-v1:cargo test --lib",
  prefix: "cargo test --lib",
  expiresAt: Math.floor(NOW / 1000) + secondsFromNow,
  remainingSecs: secondsFromNow,
});

describe("grants", () => {
  test("an answer lists its grants and drops rows it cannot read", () => {
    expect(
      parseGrants({
        grants: [
          { id: "a", prefix: "cargo test", expiresAt: 100, remainingSecs: 42 },
          { id: "b", prefix: "bun run check" },
          { prefix: "no id", expiresAt: 100 },
          "nope",
        ],
      }),
    ).toEqual([{ id: "a", prefix: "cargo test", expiresAt: 100, remainingSecs: 42 }]);
    expect(parseGrants(null)).toEqual([]);
    expect(parseGrants({})).toEqual([]);
  });

  test("what is left reads in minutes, then seconds", () => {
    expect(grantRemaining(at(42 * 60), NOW)).toBe("42m");
    expect(grantRemaining(at(59), NOW)).toBe("59s");
    expect(grantRemaining(at(60), NOW)).toBe("1m");
    expect(grantRemaining(at(0), NOW)).toBe("expired");
    expect(grantRemaining(at(-5), NOW)).toBe("expired");
  });

  test("an expired grant is not in force", () => {
    expect(grantActive(at(1), NOW)).toBe(true);
    expect(grantActive(at(0), NOW)).toBe(false);
    expect(activeGrants([at(600), at(60), at(-1)], NOW).map((g) => g.remainingSecs)).toEqual([60, 600]);
  });
});
