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

const accountPlan = (id: string, status: UsagePlan["status"] = "ok"): UsagePlan =>
  plan({ id: `claude:${id}`, name: id, account: id, status });

const paused = (claude: boolean, ...accounts: string[]): PlanPauses => ({
  claude,
  accounts: new Set(accounts),
});

/** A thread on a named Claude account, with the switch on. */
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

  test("nothing without consent", () => {
    expect(onAccount({ allowed: false })).toEqual({ claude: false });
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

  test("an explicit look is not held back by a pause", () => {
    expect(onAccount({ pauses: paused(true, "work"), explicit: true })).toEqual({
      claude: true,
      account: "work",
    });
  });
});

describe("a plans answer pauses the login it could not read", () => {
  test("a denied account pauses that account only", () => {
    const before = NO_PAUSES;
    const after = pausesAfter([plan({ status: "notAsked" }), accountPlan("work", "unavailable")], before);
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
    const after = pausesAfter([plan({ status: "unavailable" }), accountPlan("work")], NO_PAUSES);
    expect(after).toEqual(paused(true));
    // The account is still read, and the default login is not.
    expect(onAccount({ pauses: after })).toEqual({ claude: false, account: "work" });
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
