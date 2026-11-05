import { describe, expect, test } from "bun:test";
import type { UsagePlan } from "../acp/rustcode";
import { PAGE_FRESH_MS, type PageAnswer } from "./pageCache";
import { answeredPlans, type UsageAnswer, usageView } from "./usagePage";

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
