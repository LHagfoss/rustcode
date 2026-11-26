// Timed approvals as the panel reads them.

import { describe, expect, test } from "bun:test";
import {
  activeGrants,
  browserChipLabel,
  browserGrantId,
  grantActive,
  grantRemaining,
  parseBrowserGrants,
  parseGrants,
  type TimedGrant,
} from "./grants";

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

describe("browser site grants", () => {
  test("the list reads each thread's hosts and skips rows without any", () => {
    const answer = {
      grants: [],
      browser: [
        {
          id: "browser:a",
          sessionId: "a",
          origins: ["https://staging.example.com"],
          hosts: ["staging.example.com"],
        },
        { id: "browser:b", sessionId: "b", origins: [], hosts: [] },
        { id: "browser:c", hosts: ["x.test"] },
      ],
    };
    expect(parseBrowserGrants(answer)).toEqual({ a: ["staging.example.com"] });
    expect(parseBrowserGrants({ grants: [] })).toEqual({});
  });

  test("the chip names the first site and counts the rest, and hides without a grant", () => {
    expect(browserChipLabel(["staging.example.com"])).toBe("Browser: staging.example.com");
    expect(browserChipLabel(["a.test", "b.test", "c.test"])).toBe("Browser: a.test +2");
    expect(browserChipLabel([])).toBeNull();
    expect(browserChipLabel(undefined)).toBeNull();
  });

  test("a revoke names the thread", () => {
    expect(browserGrantId("t1")).toBe("browser:t1");
  });
});
