import { describe, expect, test } from "bun:test";
import type { UsagePlan } from "../acp/rustcode";
import { PAGE_FRESH_MS, type PageAnswer } from "./pageCache";
import { answeredPlans, failureText, type UsageAnswer, usageView } from "./usagePage";

const NOW = 10_000_000;

const plan = (over: Partial<UsagePlan>): UsagePlan => ({
  id: "claude",
  name: "Claude",
  via: "Claude Code",
  harness: "claude-code",
  account: null,
  status: "ok",
  windows: [{ label: "5-hour", usedPercent: 10, minutes: 300 }],
  ...over,
});

const claude = plan({ id: "claude", name: "Claude" });
const work = plan({ id: "claude:work", name: "Work", account: "work" });
const codex = plan({
  id: "codex",
  name: "ChatGPT",
  via: "Codex CLI",
  harness: "codex",
  windows: [{ label: "Weekly", usedPercent: 5, minutes: 10_080 }],
});

const answer = (plans: UsagePlan[], asked = true, readAt = NOW): PageAnswer<UsageAnswer> => ({
  data: { plans, asked },
  readAt,
});

describe("what Settings › Usage shows", () => {
  test("first open: nothing yet, so the skeleton and a read", () => {
    expect(usageView(null, true, null, NOW)).toEqual({
      plans: null,
      readAt: null,
      fetch: true,
      force: false,
      fatal: null,
      note: null,
    });
  });

  test("reopen within 30 seconds: the same list at once, and no read", () => {
    const cached = answer([claude, codex], true, NOW - PAGE_FRESH_MS + 1);
    const view = usageView(cached, true, null, NOW);
    expect(view.plans).toBe(cached.data.plans);
    expect(view.readAt).toBe(cached.readAt);
    expect(view.fetch).toBe(false);
    expect(view.fatal).toBeNull();
  });

  test("reopen later: the same list at once, and a quiet read behind it", () => {
    const cached = answer([claude, codex], true, NOW - PAGE_FRESH_MS);
    const view = usageView(cached, true, null, NOW);
    expect(view.plans).toBe(cached.data.plans);
    expect(view.fetch).toBe(true);
    expect(view.readAt).toBe(cached.readAt);
  });

  test("consent off: Claude's numbers go at once, and the switch's own answer is read", () => {
    const cached = answer([claude, work, codex], true);
    const view = usageView(cached, false, null, NOW);
    // The one Claude row that carries the switch stays, with nothing to draw;
    // named accounts add no row once consent is off.
    expect(view.plans?.map((p) => p.id)).toEqual(["claude", "codex"]);
    expect(view.plans?.[0]).toMatchObject({ id: "claude", status: "notAsked", windows: [] });
    expect(view.fetch).toBe(true);
    // An answer read with consent off is fresh for consent off.
    expect(usageView(answer([codex], false), false, null, NOW).fetch).toBe(false);
  });

  test("a failed read with numbers keeps them and says so quietly", () => {
    const cached = answer([claude, codex]);
    const view = usageView(cached, true, "Keychain denied", NOW);
    expect(view.plans).toBe(cached.data.plans);
    expect(view.fatal).toBeNull();
    expect(view.note).toBe("Keychain denied");
  });

  test("a failed read with nothing to show is the error page", () => {
    const view = usageView(null, true, "Keychain denied", NOW);
    expect(view.plans).toBeNull();
    expect(view.fatal).toBe("Keychain denied");
    expect(view.note).toBeNull();
  });

  test("a denied Keychain read, a forbidden sign-in or a transient failure keeps the ordinary freshness", () => {
    for (const reason of ["denied", "forbidden", "transient", undefined] as const) {
      const cached = answer(
        [plan({ status: "unavailable", reason, error: "no", windows: [] }), codex],
        true,
        NOW - 1,
      );
      const view = usageView(cached, true, null, NOW);
      // A Deny must not turn every page open into another prompt, and a 403 is
      // not fixed by asking again.
      expect(view.fetch).toBe(false);
      expect(view.force).toBe(false);
    }
  });

  test("an expired answer behind consent off never forces a Keychain read", () => {
    // The page still holds the row it drew before the switch went off; opening
    // it must not read the Keychain, and must not draw the old numbers.
    const cached = answer(
      [plan({ status: "unavailable", reason: "expired", error: "no", windows: [] }), codex],
      false,
    );
    const view = usageView(cached, false, null, NOW);
    expect(view.force).toBe(false);
    expect(view.fetch).toBe(false);
    expect(view.plans?.[0]).toMatchObject({ status: "notAsked", windows: [] });
    expect(view.plans?.[0]?.reason).toBeUndefined();
  });

  test("an expired or refused Claude sign-in reads again at once, past the engine's cache", () => {
    // Even an answer younger than the page's own 30 seconds: the user may have
    // just renewed the sign-in in Claude Code, and the numbers on screen are
    // the ones that stopped working.
    for (const reason of ["expired", "refused"] as const) {
      const cached = answer(
        [plan({ status: "unavailable", reason, error: "no", windows: [] }), codex],
        true,
        NOW - 1,
      );
      const view = usageView(cached, true, null, NOW);
      expect(view.fetch).toBe(true);
      expect(view.force).toBe(true);
      expect(view.plans).toBe(cached.data.plans);
    }
  });

  test("a denied Keychain read or a transient failure keeps the ordinary freshness", () => {
    for (const reason of ["denied", "transient", undefined] as const) {
      const cached = answer(
        [plan({ status: "unavailable", reason, error: "no", windows: [] }), codex],
        true,
        NOW - 1,
      );
      const view = usageView(cached, true, null, NOW);
      // A Deny must not turn every page open into another prompt.
      expect(view.fetch).toBe(false);
      expect(view.force).toBe(false);
    }
  });

  test("a row drawn from recorded numbers is not retried on every open", () => {
    // Numbers RustCode recorded are numbers: nothing to force a Keychain read
    // for, and the ordinary background poll refreshes them when Claude Code
    // renews the token.
    const cached = answer([{ ...work, status: "ok", asOf: NOW - 3 * 60 * 60 * 1000 }], true);
    expect(usageView(cached, true, null, NOW)).toMatchObject({ fetch: false, force: false });
  });

  test("a named account's expired row is retried as well", () => {
    const cached = answer(
      [claude, plan({ ...work, status: "unavailable", reason: "expired", error: "no", windows: [] })],
      true,
    );
    expect(usageView(cached, true, null, NOW)).toMatchObject({ fetch: true, force: true });
  });
});

describe("what a failed row says", () => {
  test("each reason has its own words, and the engine's message wins beside them", () => {
    const failed = (reason: UsagePlan["reason"]) =>
      plan({ status: "unavailable", reason, error: `engine: ${reason ?? "none"}`, windows: [] });
    // A lapsed access token does not make the sign-in expire: Claude Code
    // renews it when it runs, so the words ask for a run on that login.
    expect(failureText(failed("expired")).title).toMatch(/refresh this sign-in/);
    expect(failureText(failed("expired")).subtitle).toBe("Run a Claude Code thread to refresh its usage.");
    // An account's row says which account has to run.
    expect(failureText({ ...failed("expired"), account: "personal", id: "claude:personal" }).subtitle).toBe(
      "Run a Claude Code thread on this account to refresh its usage.",
    );
    expect(failureText(failed("refused")).title).toMatch(/did not accept/);
    expect(failureText(failed("forbidden")).title).toMatch(/does not allow/);
    expect(failureText(failed("forbidden")).subtitle).toMatch(/does not change/);
    expect(failureText(failed("denied")).title).toMatch(/Keychain/);
    expect(failureText(failed("transient")).title).toBe("Could not read the usage");
    // An older engine's row keeps the old words.
    expect(failureText(failed(undefined))).toEqual({ title: "Could not read the usage" });
  });
});

describe("a partial plans answer", () => {
  test("keeps the rows it did not ask, and replaces the ones it did", () => {
    const shown = [claude, work, codex];
    const nextCodex = { ...codex, windows: [{ label: "Weekly", usedPercent: 80, minutes: 10_080 }] };
    const partial = [plan({ status: "notAsked", windows: [] }), nextCodex];
    const merged = answeredPlans(shown, partial, false);
    expect(merged.map((p) => p.id)).toEqual(["claude", "claude:work", "codex"]);
    expect(merged[0]).toBe(claude);
    expect(merged[1]).toBe(work);
    expect(merged[2]).toBe(nextCodex);
  });

  test("adds a row it read that the page did not have", () => {
    const merged = answeredPlans([codex], [plan({ status: "notAsked", windows: [] }), work, codex], false);
    expect(merged.map((p) => p.id)).toEqual(["codex", "claude", "claude:work"]);
  });

  test("keeps the row that carries the switch on a first read without Claude", () => {
    const merged = answeredPlans([], [plan({ status: "notAsked", windows: [] }), codex], false);
    expect(merged.map((p) => p.id)).toEqual(["claude", "codex"]);
  });

  test("the page's own whole read replaces the list", () => {
    const merged = answeredPlans([claude, work, codex], [codex], true);
    expect(merged).toEqual([codex]);
  });
});
