import { describe, expect, test } from "bun:test";
import { LIMIT_GRACE_MS, LIMIT_MAX_ATTEMPTS, limitReset, nextWait, waitDue, waitHolds } from "./limitResume";

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
