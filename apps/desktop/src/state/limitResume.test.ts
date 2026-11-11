import { describe, expect, test } from "bun:test";
import {
  LIMIT_GRACE_MS,
  LIMIT_MAX_ATTEMPTS,
  limitReset,
  nextWait,
  OVERLOAD_FIRST_MS,
  OVERLOAD_JITTER,
  OVERLOAD_MAX_ATTEMPTS,
  OVERLOAD_MAX_MS,
  OVERLOAD_RESUME_TEXT,
  overloadDelay,
  overloadStop,
  resumeText,
  waitDue,
  waitHolds,
} from "./limitResume";

// 2026-11-05 14:48 in Oslo, which is UTC+1 in November.
const NOW = Date.UTC(2026, 10, 5, 13, 48);
const CLAUDE = "You've hit your 5-hour limit · resets 2:50pm (Europe/Oslo)";

describe("a turn that ended on a limit", () => {
  test("reads Claude Code's message in its own zone", () => {
    expect(limitReset(CLAUDE, NOW)).toBe(Date.UTC(2026, 10, 5, 13, 50));
    expect(limitReset("You've hit your 5-hour limit · resets 3pm (Asia/Tokyo)", NOW)).toBe(
      Date.UTC(2026, 10, 6, 6, 0),
    );
  });

  test("a weekly limit names its day", () => {
    expect(limitReset("You’ve hit your weekly limit · resets Nov 8, 3pm (Europe/Oslo)", NOW)).toBe(
      Date.UTC(2026, 10, 8, 14, 0),
    );
    expect(limitReset("You've hit your weekly limit · resets Nov 8 at 3:30pm (Europe/Oslo)", NOW)).toBe(
      Date.UTC(2026, 10, 8, 14, 30),
    );
  });

  test("a time just behind us tries again soon, and an older one means tomorrow", () => {
    const just = limitReset("You've hit your 5-hour limit · resets 2:40pm (Europe/Oslo)", NOW)!;
    expect(just).toBe(NOW + 5 * 60_000);
    expect(limitReset("You've hit your 5-hour limit · resets 8am (Europe/Oslo)", NOW)).toBe(
      Date.UTC(2026, 10, 6, 7, 0),
    );
  });

  test("an unknown zone reads as local time rather than losing the wait", () => {
    expect(limitReset("You've hit your 5-hour limit · resets 11:59pm (Not/AZone)", NOW)).not.toBeNull();
  });

  test("a reply that only talks about limits is not one", () => {
    for (const text of [
      undefined,
      "",
      "Done. The tests pass.",
      `The thread printed: ${CLAUDE}`,
      `${CLAUDE}\n\nHere is what I did so far.`,
      "You've hit your 5-hour limit · resets whenever",
      `${"long ".repeat(50)}usage limit reached`,
    ]) {
      expect(limitReset(text, NOW)).toBeNull();
    }
  });

  test("a limit with no time takes the latest full window of the plan", () => {
    const windows = [
      { usedPercent: 100, resetsAt: NOW + 1000 },
      { usedPercent: 100, resetsAt: NOW + 9000 },
      { usedPercent: 40, resetsAt: NOW + 99000 },
      { usedPercent: 100, resetsAt: NOW - 1000 },
    ];
    expect(limitReset("Codex: usage limit reached", NOW, windows)).toBe(NOW + 9000);
    expect(limitReset("Codex: usage limit reached", NOW, [])).toBeNull();
  });
});

describe("the wait", () => {
  const wait = { until: NOW, harness: "claude-code", account: "work", attempts: 1 };

  test("holds only on what hit the limit", () => {
    expect(waitHolds(wait, { harness: "claude-code", account: "work" })).toBe(true);
    expect(waitHolds(wait, { harness: "codex", account: "work" })).toBe(false);
    expect(waitHolds(wait, { harness: "claude-code", account: null })).toBe(false);
  });

  test("is due a little after the reset", () => {
    expect(waitDue(wait, NOW + LIMIT_GRACE_MS - 1)).toBe(false);
    expect(waitDue(wait, NOW + LIMIT_GRACE_MS)).toBe(true);
  });

  test("a limit that keeps coming back stops being retried", () => {
    let current = nextWait(undefined, { until: 1, account: null });
    for (let i = 1; i < LIMIT_MAX_ATTEMPTS; i++) current = nextWait(current!, { until: 1, account: null });
    expect(current?.attempts).toBe(LIMIT_MAX_ATTEMPTS);
    expect(nextWait(current!, { until: 1, account: null })).toBeNull();
  });
});

describe("a turn a provider overload stopped", () => {
  test("reads the overload each harness says", () => {
    for (const text of [
      // Codex wraps its own answer in JSON.
      'Codex: {"type":"error","error":{"type":"overloaded_error","message":"Our servers are currently overloaded. Please try again later."}}',
      // Claude Code: a status and a body, in its own words.
      'Claude Code: API Error: 529 {"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}',
      // The status on its own, as a proxy or a harness with no body writes it.
      "Codex: 503 Service Unavailable",
      "Pi: the model is at capacity",
      "Claude Code: API Error: 529",
    ]) {
      expect(overloadStop(text)).toBe(true);
    }
  });

  test("a credit or spend limit is never retried, whatever else it says", () => {
    for (const text of [
      'Claude Code: API Error: 400 {"type":"error","error":{"type":"invalid_request_error","message":"Your credit balance is too low to access the API. Please go to Plans & Billing to upgrade or purchase credits."}}',
      'Codex: {"error":{"message":"You exceeded your current quota, please check your plan and billing details.","type":"insufficient_quota"}}',
      "Pi: out of credits",
      "Claude Code: billing hard limit reached",
      "Claude Code: spend limit reached for this month",
    ]) {
      expect(overloadStop(text)).toBe(false);
    }
  });

  test("a limit, a network failure or an answer is not an overload", () => {
    for (const text of [
      undefined,
      "",
      "You've hit your 5-hour limit · resets 2:50pm (Europe/Oslo)",
      "Pi: fetch failed",
      "Claude Code: Can't reach the API server",
      'Codex: {"detail":"Bad Request"}',
      'Claude Code: API Error: 429 {"error":{"message":"Rate limited"}}',
      // A number that is a count, not a status.
      "Codex: the run used 503 tokens",
    ]) {
      expect(overloadStop(text)).toBe(false);
    }
  });

  test("the backoff doubles from 30 s, with jitter on top and a 4 m cap", () => {
    const plain = [1, 2, 3, 4].map((attempts) => overloadDelay(attempts, () => 0));
    expect(plain).toEqual([30_000, 60_000, 120_000, 240_000]);
    // The clock never rolls back: the delay is the backoff plus up to a quarter of it.
    for (const attempt of [1, 2, 3, 4, 9]) {
      const base = Math.min(OVERLOAD_FIRST_MS * 2 ** (attempt - 1), OVERLOAD_MAX_MS);
      for (const roll of [0, 0.25, 0.5, 1]) {
        const delay = overloadDelay(attempt, () => roll);
        expect(delay).toBeGreaterThanOrEqual(base);
        expect(delay).toBeLessThanOrEqual(base + base * OVERLOAD_JITTER);
      }
    }
    expect(overloadDelay(9, () => 0)).toBe(OVERLOAD_MAX_MS);
    expect(overloadDelay(9, () => 1)).toBe(OVERLOAD_MAX_MS * (1 + OVERLOAD_JITTER));
  });

  test("a real die stays inside the bounds", () => {
    for (let i = 0; i < 200; i++) {
      expect(overloadDelay(2)).toBeGreaterThanOrEqual(60_000);
      expect(overloadDelay(2)).toBeLessThanOrEqual(60_000 * (1 + OVERLOAD_JITTER));
    }
  });

  test("is due the moment it ends, with no limit's grace", () => {
    const wait = { until: NOW, harness: "codex", account: null, attempts: 1, reason: "overload" as const };
    expect(waitDue(wait, NOW - 1)).toBe(false);
    expect(waitDue(wait, NOW)).toBe(true);
  });

  test("gets four attempts, then stops", () => {
    let current = nextWait(undefined, { until: 1, account: null, reason: "overload" });
    for (let i = 1; i < OVERLOAD_MAX_ATTEMPTS; i++)
      current = nextWait(current!, { until: 1, account: null, reason: "overload" });
    expect(current?.attempts).toBe(OVERLOAD_MAX_ATTEMPTS);
    expect(nextWait(current!, { until: 1, account: null, reason: "overload" })).toBeNull();
  });

  test("a stop that changes kind starts its own count", () => {
    const limit = { until: 1, account: null, attempts: LIMIT_MAX_ATTEMPTS };
    // A full limit, then an overload: the limit's tries are not the overload's.
    expect(nextWait(limit, { until: 2, account: null, reason: "overload" })?.attempts).toBe(1);
    // The same kind again is one try too many.
    expect(nextWait(limit, { until: 2, account: null })).toBeNull();
  });

  test("says what stopped it in the words it is resumed with", () => {
    expect(resumeText({ until: 1, account: null, attempts: 1, reason: "overload" })).toBe(
      OVERLOAD_RESUME_TEXT,
    );
    expect(resumeText({ until: 1, account: null, attempts: 1 })).toContain("usage limit");
  });
});
