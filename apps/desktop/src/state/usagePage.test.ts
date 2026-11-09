import { afterEach, describe, expect, jest, test } from "bun:test";
import type { UsagePlan } from "../acp/rustcode";
import { PAGE_FRESH_MS, type PageAnswer } from "./pageCache";
import {
  answeredPlans,
  failureText,
  groupPlans,
  oneReadAtATime,
  PLAN_REFRESH_MAX_BACKOFF_MS,
  refreshBackoff,
  type UsageAnswer,
  usageAutoRead,
  usagePressTarget,
  usageView,
  watchUsageRefresh,
} from "./usagePage";

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

test("groups interleaved accounts under their harness, including unavailable plans", () => {
  const denied = plan({
    id: "claude:denied",
    account: "denied",
    status: "unavailable",
    reason: "denied",
    windows: [],
  });
  expect(groupPlans([claude, codex, work, denied])).toEqual([
    { harness: "claude-code", plans: [claude, work, denied] },
    { harness: "codex", plans: [codex] },
  ]);
});

describe("what Settings › Usage shows", () => {
  test("first open: nothing yet, so the skeleton and a read", () => {
    expect(usageView(null, null, NOW)).toEqual({
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
    const view = usageView(cached, null, NOW);
    expect(view.plans).toBe(cached.data.plans);
    expect(view.readAt).toBe(cached.readAt);
    expect(view.fetch).toBe(false);
    expect(view.fatal).toBeNull();
  });

  test("reopen later: the same list at once, and a quiet read behind it", () => {
    const cached = answer([claude, codex], true, NOW - PAGE_FRESH_MS);
    const view = usageView(cached, null, NOW);
    expect(view.plans).toBe(cached.data.plans);
    expect(view.fetch).toBe(true);
    expect(view.readAt).toBe(cached.readAt);
  });

  test("a failed read with numbers keeps them and says so quietly", () => {
    const cached = answer([claude, codex]);
    const view = usageView(cached, "Keychain denied", NOW);
    expect(view.plans).toBe(cached.data.plans);
    expect(view.fatal).toBeNull();
    expect(view.note).toBe("Keychain denied");
  });

  test("a failed read with nothing to show is the error page", () => {
    const view = usageView(null, "Keychain denied", NOW);
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
      const view = usageView(cached, null, NOW);
      // A Deny must not turn every page open into another prompt, and a 403 is
      // not fixed by asking again.
      expect(view.fetch).toBe(false);
      expect(view.force).toBe(false);
    }
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
      const view = usageView(cached, null, NOW);
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
      const view = usageView(cached, null, NOW);
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
    expect(usageView(cached, null, NOW)).toMatchObject({ fetch: false, force: false });
  });

  test("a named account's expired row is retried as well", () => {
    const cached = answer(
      [claude, plan({ ...work, status: "unavailable", reason: "expired", error: "no", windows: [] })],
      true,
    );
    expect(usageView(cached, null, NOW)).toMatchObject({ fetch: true, force: true });
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
    expect(failureText(failed("denied")).title).toMatch(/sign-in/);
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

describe("a press on the composer's usage meter", () => {
  test("opens Settings' Usage page", () => {
    expect(usagePressTarget(false)).toBe("usage");
  });

  test("closes the page it is already on", () => {
    expect(usagePressTarget(true)).toBe(null);
  });
});

/** The app's one plan-poll length (state/store.ts `PLAN_CACHE_POLL_MS`). */
const EVERY = 180_000;

describe("the page keeps itself current", () => {
  /** Lets the loop's `await` and its re-arm run before the test looks at it. */
  const settle = async () => {
    await Promise.resolve();
    await Promise.resolve();
  };
  const advance = async (ms: number) => {
    jest.advanceTimersByTime(ms);
    await settle();
  };

  test("a tick reads while the window is visible, unless a read is in flight", () => {
    const tick = (over: Partial<Parameters<typeof usageAutoRead>[0]> = {}) =>
      usageAutoRead({
        readAt: NOW,
        now: NOW,
        visible: true,
        reading: false,
        trigger: "tick",
        everyMs: EVERY,
        ...over,
      });
    expect(tick()).toBe(true);
    // A hidden window asks for nothing.
    expect(tick({ visible: false })).toBe(false);
    // Neither does one that is already reading.
    expect(tick({ reading: true })).toBe(false);
  });

  test("a tick reads even when the app's plan cache is fresh, so the page never freezes", async () => {
    // The rows on screen come from the page's own read: skipping the tick
    // because another loop kept the cache warm left the page drawing old
    // numbers. (The engine shares a read already under way, so the overlap
    // costs one request.)
    expect(
      usageAutoRead({
        readAt: NOW,
        now: NOW,
        visible: true,
        reading: false,
        trigger: "tick",
        everyMs: EVERY,
      }),
    ).toBe(true);
    jest.useFakeTimers();
    let reads = 0;
    const stop = watchUsageRefresh({
      read: () => {
        reads += 1;
        return true;
      },
      reading: () => false,
      visible: () => true,
      // Just read, by this page and by the app's poll.
      readAt: () => Date.now(),
      everyMs: EVERY,
      onChange: () => () => {},
    });
    await advance(EVERY);
    expect(reads).toBe(1);
    await advance(EVERY);
    expect(reads).toBe(2);
    stop();
  });

  test("coming back reads only when what is drawn is older than the interval", () => {
    const back = (readAt: number | null) =>
      usageAutoRead({
        readAt,
        now: NOW,
        visible: true,
        reading: false,
        trigger: "resumed",
        everyMs: EVERY,
      });
    expect(back(NOW - EVERY + 1)).toBe(false);
    expect(back(NOW - EVERY)).toBe(true);
    // Nothing drawn yet: the first answer is always wanted.
    expect(back(null)).toBe(true);
  });

  test("a failed read doubles the wait, up to fifteen minutes", () => {
    expect(refreshBackoff(EVERY, 0)).toBe(EVERY);
    expect(refreshBackoff(EVERY, 1)).toBe(EVERY * 2);
    expect(refreshBackoff(EVERY, 2)).toBe(EVERY * 4);
    expect(refreshBackoff(EVERY, 3)).toBe(PLAN_REFRESH_MAX_BACKOFF_MS);
    expect(refreshBackoff(EVERY, 40)).toBe(PLAN_REFRESH_MAX_BACKOFF_MS);
    expect(PLAN_REFRESH_MAX_BACKOFF_MS).toBe(15 * 60_000);
  });

  test("the loop ticks every interval, catches up on coming back when stale, and stops", async () => {
    jest.useFakeTimers();
    let visible = true;
    let reading = false;
    let readAt = Date.now();
    let reads = 0;
    let changes: (() => void)[] = [];
    const stop = watchUsageRefresh({
      read: () => {
        reads += 1;
        readAt = Date.now();
        return true;
      },
      reading: () => reading,
      visible: () => visible,
      readAt: () => readAt,
      everyMs: EVERY,
      onChange: (handler) => {
        changes.push(handler);
        return () => {
          changes = changes.filter((h) => h !== handler);
        };
      },
    });
    const resume = async () => {
      for (const handler of changes) handler();
      await settle();
    };

    await advance(EVERY);
    expect(reads).toBe(1);
    await advance(EVERY);
    expect(reads).toBe(2);

    // A read in flight is not doubled, and the next tick reads once it is done.
    reading = true;
    await advance(EVERY);
    expect(reads).toBe(2);
    reading = false;
    await advance(EVERY);
    expect(reads).toBe(3);

    // Hidden: no read, and coming back within the interval costs nothing.
    visible = false;
    await advance(EVERY / 2);
    expect(reads).toBe(3);
    visible = true;
    readAt = Date.now();
    await resume();
    expect(reads).toBe(3);

    // Hidden for longer than the interval: coming back catches up at once.
    visible = false;
    await advance(EVERY * 2);
    expect(reads).toBe(3);
    visible = true;
    await resume();
    expect(reads).toBe(4);

    // Stopped, the loop reads nothing more and hears no visibility change.
    stop();
    expect(changes).toHaveLength(0);
    await advance(EVERY * 2);
    expect(reads).toBe(4);
  });

  test("the loop waits longer after a read that failed, and back to the interval after one that works", async () => {
    jest.useFakeTimers();
    const STEP = 60_000;
    let failing = true;
    let reads = 0;
    const stop = watchUsageRefresh({
      read: () => {
        reads += 1;
        return !failing;
      },
      reading: () => false,
      visible: () => true,
      readAt: () => Date.now(),
      everyMs: STEP,
      onChange: () => () => {},
    });

    // One failed read: the next one is two steps away, not one.
    await advance(STEP);
    expect(reads).toBe(1);
    await advance(STEP);
    expect(reads).toBe(1);
    await advance(STEP);
    expect(reads).toBe(2);
    // Two in a row: four steps.
    await advance(STEP * 3);
    expect(reads).toBe(2);
    await advance(STEP);
    expect(reads).toBe(3);
    // Three: eight steps.
    await advance(STEP * 7);
    expect(reads).toBe(3);
    await advance(STEP);
    expect(reads).toBe(4);
    // Four: sixteen steps asked for, fifteen allowed.
    expect(refreshBackoff(STEP, 4)).toBe(PLAN_REFRESH_MAX_BACKOFF_MS);
    failing = false;
    await advance(STEP * 14);
    expect(reads).toBe(4);
    await advance(STEP);
    expect(reads).toBe(5);
    // A read that answers puts it back on the interval.
    await advance(STEP);
    expect(reads).toBe(6);
    await advance(STEP);
    expect(reads).toBe(7);
    stop();
  });

  test("a read that throws counts as a failure and never stops the loop", async () => {
    jest.useFakeTimers();
    let reads = 0;
    const stop = watchUsageRefresh({
      read: () => {
        reads += 1;
        throw new Error("engine went away");
      },
      reading: () => false,
      visible: () => true,
      readAt: () => Date.now(),
      everyMs: EVERY,
      onChange: () => () => {},
    });
    await advance(EVERY);
    expect(reads).toBe(1);
    // Backed off to two intervals.
    await advance(EVERY);
    expect(reads).toBe(1);
    await advance(EVERY);
    expect(reads).toBe(2);
    stop();
  });

  afterEach(() => {
    jest.useRealTimers();
  });
});

describe("oneReadAtATime", () => {
  /** A read the test finishes by hand, recording what it was asked. */
  function held() {
    const asked: string[] = [];
    const finish: Array<(ok: boolean) => void> = [];
    const gate = oneReadAtATime<string>(
      (what) =>
        new Promise<boolean>((resolve) => {
          asked.push(what);
          finish.push(resolve);
        }),
    );
    return { asked, finish, gate };
  }
  const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

  test("an explicit look during a background read runs right after it", async () => {
    const { asked, finish, gate } = held();
    const tick = gate.run("tick", false);
    expect(gate.busy()).toBe(true);
    // Claude's switch goes on while the tick reads: that look is kept, not dropped.
    expect(await gate.run("switch on", true)).toBe(true);
    expect(asked).toEqual(["tick"]);
    finish[0]!(true);
    expect(await tick).toBe(true);
    await settle();
    expect(asked).toEqual(["tick", "switch on"]);
    finish[1]!(true);
    await settle();
    expect(gate.busy()).toBe(false);
  });

  test("a background read during a read is dropped, and only the newest look is kept", async () => {
    const { asked, finish, gate } = held();
    void gate.run("open", true);
    void gate.run("tick", false);
    void gate.run("switch on", true);
    void gate.run("switch off", true);
    finish[0]!(false);
    await settle();
    expect(asked).toEqual(["open", "switch off"]);
    finish[1]!(true);
    await settle();
    expect(asked).toEqual(["open", "switch off"]);
    expect(gate.busy()).toBe(false);
  });

  test("a read that throws is a failure and frees the gate", async () => {
    const gate = oneReadAtATime<string>(async () => {
      throw new Error("engine went away");
    });
    expect(await gate.run("tick", false)).toBe(false);
    expect(gate.busy()).toBe(false);
  });
});
