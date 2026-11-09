import { describe, expect, test } from "bun:test";
import type { UsagePlan } from "../acp/rustcode";
import { NO_PAUSES, type PlanPauses, pausesAfter, plansAskFor, queueAsks } from "./plans";

const plan = (over: Partial<UsagePlan>): UsagePlan => ({
  id: "claude",
  name: "Claude",
  via: "Claude Code",
  harness: "claude-code",
  account: null,
  status: "ok",
  windows: [],
  ...over,
});

const accountPlan = (
  id: string,
  status: UsagePlan["status"] = "ok",
  reason?: UsagePlan["reason"],
): UsagePlan => plan({ id: `claude:${id}`, name: id, account: id, status, reason });

const paused = (claude: boolean, ...accounts: string[]): PlanPauses => ({
  claude,
  accounts: new Set(accounts),
});

/** A thread on a named Claude account, after the first read. */
const onAccount = (over: Partial<Parameters<typeof plansAskFor>[0]> = {}) =>
  plansAskFor({
    allowed: true,
    harness: "claude-code",
    account: "work",
    pauses: NO_PAUSES,
    ...over,
  });

describe("what a refresh asks for", () => {
  test("the default login and the account of the thread on screen", () => {
    expect(onAccount()).toEqual({ claude: true, account: "work" });
    // A thread on the default login asks for that one only.
    expect(onAccount({ account: null })).toEqual({ claude: true });
    // With no thread, the next thread's account stands in (the caller passes it).
    expect(onAccount({ account: "home" })).toEqual({ claude: true, account: "home" });
  });

  test("nothing before the first read is requested", () => {
    expect(onAccount({ allowed: false })).toEqual({ claude: false });
    // An unstarted background read does not reach the Keychain.
    expect(onAccount({ allowed: false, explicit: true, pauses: paused(true, "work") })).toEqual({
      claude: false,
    });
  });

  test("only a Claude Code thread has an account to read", () => {
    expect(onAccount({ harness: "codex" })).toEqual({ claude: true });
    expect(onAccount({ harness: "rustcode" })).toEqual({ claude: true });
    expect(onAccount({ harness: undefined })).toEqual({ claude: true });
  });

  test("a paused default login still reads the account", () => {
    // The default login read signed out or was denied: the account's own
    // numbers are still worth having, and are a separate Keychain item.
    expect(onAccount({ pauses: paused(true) })).toEqual({ claude: false, account: "work" });
  });

  test("a paused account still reads the default login, and does not ask for it again", () => {
    expect(onAccount({ pauses: paused(false, "work") })).toEqual({ claude: true });
    expect(onAccount({ pauses: paused(false, "home") })).toEqual({ claude: true, account: "work" });
  });

  test("an explicit look is not held back by a pause, and forces the engine to read", () => {
    expect(onAccount({ pauses: paused(true, "work"), explicit: true })).toEqual({
      claude: true,
      account: "work",
      force: true,
    });
  });
});

describe("a plans answer pauses the login it could not read", () => {
  test("a denied account pauses that account only", () => {
    const before = NO_PAUSES;
    const after = pausesAfter(
      [plan({ status: "notAsked" }), accountPlan("work", "unavailable", "denied")],
      before,
    );
    expect(after).toEqual(paused(false, "work"));
    // The default login was not read, so it stays as it was: still asked for.
    expect(plansAskFor({ allowed: true, harness: "codex", pauses: after })).toEqual({ claude: true });
    // And another account is not affected.
    expect(plansAskFor({ allowed: true, harness: "claude-code", account: "home", pauses: after })).toEqual({
      claude: true,
      account: "home",
    });
  });

  test("a signed-out account pauses it too, and hides its numbers elsewhere", () => {
    expect(pausesAfter([accountPlan("work", "signedOut")], NO_PAUSES)).toEqual(paused(false, "work"));
  });

  test("the default login's failure pauses the default login only", () => {
    const after = pausesAfter(
      [plan({ status: "unavailable", reason: "denied" }), accountPlan("work")],
      NO_PAUSES,
    );
    expect(after).toEqual(paused(true));
    // The account is still read, and the default login is not.
    expect(onAccount({ pauses: after })).toEqual({ claude: false, account: "work" });
  });

  /**
   * The reported bug: Claude Code renewed the sign-in, but the app had paused
   * the login when it read "expired", so it never saw the new one until the
   * user visited Settings › Usage. Only a denial is paused now.
   */
  test("an expired sign-in is retried in the background, so a renewal is noticed", () => {
    const after = pausesAfter(
      [plan({ status: "unavailable", reason: "expired" }), accountPlan("work", "unavailable", "expired")],
      NO_PAUSES,
    );
    expect(after).toEqual(NO_PAUSES);
    // The next background read asks for both again.
    expect(onAccount({ pauses: after })).toEqual({ claude: true, account: "work" });
    // A refusal is repaired the same way, by signing in again in Claude Code.
    expect(pausesAfter([plan({ status: "unavailable", reason: "refused" })], NO_PAUSES)).toBe(NO_PAUSES);
    // So is a read that simply did not come back.
    expect(pausesAfter([plan({ status: "unavailable", reason: "transient" })], NO_PAUSES)).toBe(NO_PAUSES);
  });

  test("an older engine's failure without a reason is still paused, as it always was", () => {
    // Nothing on the wire says whether it was a Deny: never risk a prompt loop.
    expect(pausesAfter([plan({ status: "unavailable" })], NO_PAUSES)).toEqual(paused(true));
    expect(pausesAfter([accountPlan("work", "unavailable")], NO_PAUSES)).toEqual(paused(false, "work"));
  });

  test("a partial answer says nothing about the logins it did not read", () => {
    const before = paused(true, "work");
    // Only one account's row came back: the default login's pause stays.
    expect(pausesAfter([accountPlan("work"), accountPlan("home")], before)).toEqual(paused(true, "work"));
    expect(pausesAfter([], before)).toBe(before);
  });

  test("an account that reads fine is not unpaused by that alone", () => {
    // A Deny is cleared by an explicit look, not by the next background read.
    const before = paused(false, "work");
    expect(pausesAfter([accountPlan("work")], before)).toBe(before);
  });

  test("a forbidden sign-in is not retried, since renewing it does not help", () => {
    expect(pausesAfter([accountPlan("work", "unavailable", "forbidden")], NO_PAUSES)).toEqual(
      paused(false, "work"),
    );
    expect(pausesAfter([plan({ status: "unavailable", reason: "forbidden" })], NO_PAUSES)).toEqual(
      paused(true),
    );
    // The account is still read, and the forbidden default login is not.
    expect(onAccount({ pauses: paused(true) })).toEqual({ claude: false, account: "work" });
  });
});

describe("asks that arrive while one is in flight", () => {
  test("coalesce into the waiting request, and never pile up", () => {
    const first = [{ claude: true }];
    // The same ask again changes nothing.
    expect(queueAsks(first, { claude: true })).toBe(first);
    // The default login and an account fold into one call.
    expect(queueAsks(first, { claude: false, account: "work" })).toEqual([{ claude: true, account: "work" }]);
    // An ask the waiting request already carries is swallowed.
    const both = [{ claude: true, account: "work" }];
    expect(queueAsks(both, { claude: false, account: "work" })).toBe(both);
    expect(queueAsks(both, { claude: false, account: "other" })).toEqual([
      { claude: true, account: "work" },
      { claude: false, account: "other" },
    ]);
  });

  test("an empty ask queues nothing", () => {
    expect(queueAsks([], { claude: false })).toEqual([]);
  });

  test("an explicit look folded into a waiting ask still forces the read", () => {
    // Check in Settings › Agents while the poll is in flight: the request that
    // runs next must not be answered from the engine's cache.
    expect(queueAsks([{ claude: true }], { claude: false, account: "work", force: true })).toEqual([
      { claude: true, account: "work", force: true },
    ]);
    // A forced ask that already waits keeps its force.
    expect(queueAsks([{ claude: true, force: true }], { claude: false })).toEqual([
      { claude: true, force: true },
    ]);
  });

  test("a thread switch to another account is kept, in order", () => {
    const queued = queueAsks(queueAsks([], { claude: true, account: "work" }), {
      claude: true,
      account: "home",
    });
    expect(queued).toEqual([
      { claude: true, account: "work" },
      { claude: true, account: "home" },
    ]);
  });
});
