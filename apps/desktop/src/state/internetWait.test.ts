import { describe, expect, test } from "bun:test";
import {
  INTERNET_GIVE_UP_MS,
  INTERNET_MAX_ATTEMPTS,
  INTERNET_POLL_MS,
  INTERNET_RESUME_TEXT,
  INTERNET_STABLE_MS,
  internetDue,
  internetGaveUp,
  internetHolds,
  nextInternetWait,
  stablePeriodMs,
} from "./internetWait";

const NOW = Date.UTC(2026, 10, 5, 13, 48);
const ON = { harness: "claude-code", account: "work" };

describe("waiting for the internet", () => {
  test("asks about the connection often enough to resume within seconds", () => {
    expect(INTERNET_POLL_MS).toBe(5_000);
    expect(INTERNET_STABLE_MS).toEqual([5_000, 30_000, 120_000, 300_000]);
    expect(INTERNET_RESUME_TEXT).toContain("Please continue.");
  });

  test("a wait keeps the first failure's time across retries", () => {
    const first = nextInternetWait(undefined, NOW, ON);
    expect(first).toEqual({ since: NOW, attempts: 0, ...ON });
    const again = nextInternetWait({ ...first, attempts: 2 }, NOW + 60_000, ON);
    expect(again.since).toBe(NOW);
    expect(again.attempts).toBe(2);
    // Moved to another login since: the wait follows what the thread runs on now.
    expect(nextInternetWait(first, NOW, { harness: "codex", account: null }).account).toBeNull();
  });

  test("each retry demands a longer stable connection, and the last period holds", () => {
    expect([0, 1, 2, 3, 4, 99].map(stablePeriodMs)).toEqual([
      5_000, 30_000, 120_000, 300_000, 300_000, 300_000,
    ]);
  });

  test("is due only once the connection has stayed up for its period", () => {
    const wait = nextInternetWait(undefined, NOW, ON);
    // Offline: never due, however long the wait has run.
    expect(internetDue(wait, null, NOW + 3_600_000)).toBe(false);
    // The connection came back a moment ago: the first retry waits 5 s.
    expect(internetDue(wait, NOW + 10_000, NOW + 14_999)).toBe(false);
    expect(internetDue(wait, NOW + 10_000, NOW + 15_000)).toBe(true);
    // A second retry needs 30 s of it, so flaky Wi-Fi cannot burn them.
    const second = { ...wait, attempts: 1 };
    expect(internetDue(second, NOW + 10_000, NOW + 39_999)).toBe(false);
    expect(internetDue(second, NOW + 10_000, NOW + 40_000)).toBe(true);
  });

  test("gives up after six hours, and says nothing is due any more", () => {
    const wait = nextInternetWait(undefined, NOW, ON);
    expect(internetGaveUp(wait, NOW + INTERNET_GIVE_UP_MS - 1)).toBe(false);
    expect(internetGaveUp(wait, NOW + INTERNET_GIVE_UP_MS)).toBe(true);
    expect(INTERNET_GIVE_UP_MS).toBe(6 * 3_600_000);
    expect(internetDue(wait, NOW + 3_600_000, NOW + INTERNET_GIVE_UP_MS)).toBe(false);
  });

  test("gives up after its last retry, even while the connection is up", () => {
    // A local model server that is down says "connection refused" with the network fine.
    const wait = { ...nextInternetWait(undefined, NOW, ON), attempts: INTERNET_MAX_ATTEMPTS - 1 };
    expect(internetGaveUp(wait, NOW + 60_000)).toBe(false);
    const spent = { ...wait, attempts: INTERNET_MAX_ATTEMPTS };
    expect(internetGaveUp(spent, NOW + 60_000)).toBe(true);
    expect(internetDue(spent, NOW, NOW + 3_600_000)).toBe(false);
  });

  test("holds only on what lost the network", () => {
    const wait = nextInternetWait(undefined, NOW, ON);
    expect(internetHolds(wait, { harness: "claude-code", account: "work" })).toBe(true);
    expect(internetHolds(wait, { harness: "codex", account: "work" })).toBe(false);
    expect(internetHolds(wait, { harness: "claude-code", account: null })).toBe(false);
  });
});
