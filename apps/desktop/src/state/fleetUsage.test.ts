// Usage across paired machines (state/fleetUsage.ts): which stores a scope
// reads, how their reports add up, and which of a machine's plans are drawn.

import { describe, expect, test } from "bun:test";
import type { UsagePlan, UsageReport, UsageRow } from "../acp/rustcode";
import {
  groupMergedPlans,
  mergeUsagePlans,
  mergeUsageReports,
  planProvenance,
  scopeHosts,
  sharedPlans,
} from "./fleetUsage";
import { emptyTotals } from "./statistics";

const row = (over: Partial<UsageRow> = {}): UsageRow => ({
  harness: "codex",
  ...emptyTotals(),
  turns: 1,
  ...over,
});

const report = (groupBy: string, rows: UsageRow[], over: Partial<UsageReport> = {}): UsageReport => {
  const totals = emptyTotals();
  for (const one of rows) {
    totals.turns += one.turns;
    totals.input = (totals.input ?? 0) + (one.input ?? 0);
  }
  return {
    groupBy,
    rows,
    totals,
    truncated: false,
    parentsReported: true,
    ...over,
  };
};

describe("scopeHosts", () => {
  test("all reads this computer first, then each connected machine", () => {
    expect(scopeHosts("all", ["vps", "box"])).toEqual(["", "vps", "box"]);
  });
  test("one machine reads only it, and a gone one falls back to this computer", () => {
    expect(scopeHosts("vps", ["vps"])).toEqual(["vps"]);
    expect(scopeHosts("", ["vps"])).toEqual([""]);
    expect(scopeHosts("gone", ["vps"])).toEqual([""]);
  });
});

describe("mergeUsageReports", () => {
  test("this computer alone is its own report", () => {
    const one = report("model", [row({ model: "gpt-5.5" })]);
    expect(mergeUsageReports([{ host: "", report: one }])).toBe(one);
  });

  test("the same model on two machines is one row with both counts", () => {
    const merged = mergeUsageReports([
      { host: "", report: report("model", [row({ model: "gpt-5.5", input: 10, cost: 1 })]) },
      {
        host: "vps",
        report: report("model", [
          row({ model: "gpt-5.5", input: 5, turns: 2 }),
          row({ model: "o4", input: 1 }),
        ]),
      },
    ]);
    expect(merged.rows).toHaveLength(2);
    const shared = merged.rows.find((one) => one.model === "gpt-5.5");
    expect(shared?.input).toBe(15);
    expect(shared?.turns).toBe(3);
    expect(shared?.cost).toBe(1);
    expect(merged.totals.turns).toBe(4);
  });

  test("days add up across machines", () => {
    const merged = mergeUsageReports([
      { host: "", report: report("day", [row({ day: "2026-11-18", turns: 2 })]) },
      {
        host: "vps",
        report: report("day", [row({ day: "2026-11-18", turns: 3 }), row({ day: "2026-11-19" })]),
      },
    ]);
    expect(merged.rows.map((one) => [one.day, one.turns])).toEqual([
      ["2026-11-18", 5],
      ["2026-11-19", 1],
    ]);
  });

  test("a machine's threads keep their machine, parents included", () => {
    const merged = mergeUsageReports([
      { host: "", report: report("thread", [row({ thread: "a" })]) },
      { host: "vps", report: report("thread", [row({ thread: "a" }), row({ thread: "b", parentId: "a" })]) },
    ]);
    expect(merged.rows.map((one) => one.thread)).toEqual(["a", "vps/a", "vps/b"]);
    expect(merged.rows[2]?.parentId).toBe("vps/a");
  });

  test("a capped machine marks the whole read capped", () => {
    const merged = mergeUsageReports([
      { host: "", report: report("model", []) },
      { host: "vps", report: report("model", [], { truncated: true }) },
    ]);
    expect(merged.truncated).toBe(true);
  });
});

describe("sharedPlans", () => {
  test("drops the rows the machine did not read", () => {
    const plans = sharedPlans({
      plans: [
        { id: "claude", name: "Claude", status: "notAsked", windows: [] },
        {
          id: "codex",
          name: "ChatGPT",
          status: "ok",
          windows: [{ label: "5-hour", usedPercent: 40, windowMinutes: 300, resetsAt: 1 }],
        },
      ],
    });
    expect(plans.map((plan) => plan.id)).toEqual(["codex"]);
  });
});

const NOW = 1_800_000_000_000;
const RESET_5H = NOW + 2 * 3_600_000;
const RESET_WK = NOW + 48 * 3_600_000;

const plan = (over: Partial<UsagePlan> = {}): UsagePlan => ({
  id: "claude:paral-2",
  name: "Paral 2",
  via: "Claude Code",
  harness: "claude-code",
  account: "paral-2",
  status: "ok",
  plan: "max",
  windows: [
    { label: "5-hour", usedPercent: 11, minutes: 300, resetsAt: RESET_5H },
    { label: "Weekly", usedPercent: 22, minutes: 10_080, resetsAt: RESET_WK },
  ],
  ...over,
});

describe("mergeUsagePlans", () => {
  test("merges the same subscription by opaque fingerprint even across different names or expired local token", () => {
    const localExpired = plan({
      id: "claude:paral-2",
      name: "Paral 2",
      account: "paral-2",
      fingerprint: "a1b2c3d4e5f60001",
      status: "unavailable",
      reason: "expired",
      plan: undefined,
      windows: [],
    });
    const remoteLive = plan({
      id: "claude",
      name: "Claude",
      account: null,
      fingerprint: "a1b2c3d4e5f60001",
      status: "ok",
      plan: "max",
      windows: [{ label: "5-hour", usedPercent: 18, minutes: 300, resetsAt: RESET_5H }],
    });

    const merged = mergeUsagePlans([
      { host: "", machine: "This computer", plans: [localExpired], readAt: NOW },
      { host: "vps", machine: "Lagos VPS", plans: [remoteLive], readAt: NOW },
    ]);

    expect(merged).toHaveLength(1);
    expect(merged[0]!.plan.name).toBe("Paral 2");
    expect(merged[0]!.plan.status).toBe("ok");
    expect(merged[0]!.plan.windows[0]?.usedPercent).toBe(18);
    expect(merged[0]!.winnerHost).toBe("vps");
    const prov = planProvenance(merged[0]!, NOW);
    expect(prov?.label).toBe("via Lagos VPS");
    expect(prov?.detail).toContain("This computer: sign-in expired");
    expect(prov?.detail).toContain("Lagos VPS: live");
  });

  test("merges same live subscription by window reset signature when fingerprint is absent", () => {
    const local = plan({
      id: "claude:paral-2",
      name: "Paral 2",
      account: "paral-2",
      windows: [
        { label: "5-hour", usedPercent: 11, minutes: 300, resetsAt: RESET_5H },
        { label: "Weekly", usedPercent: 22, minutes: 10_080, resetsAt: RESET_WK },
      ],
    });
    const remote = plan({
      id: "claude",
      name: "Claude",
      account: null,
      windows: [
        // 30s clock jitter, slightly higher consumption within same poll window.
        { label: "5-hour", usedPercent: 14, minutes: 300, resetsAt: RESET_5H + 30_000 },
        { label: "Weekly", usedPercent: 23, minutes: 10_080, resetsAt: RESET_WK + 30_000 },
      ],
    });

    const merged = mergeUsagePlans([
      { host: "", machine: "This computer", plans: [local], readAt: NOW },
      { host: "vps", machine: "Lagos VPS", plans: [remote], readAt: NOW + 1_000 },
    ]);

    expect(merged).toHaveLength(1);
    expect(merged[0]!.plan.name).toBe("Paral 2");
    expect(merged[0]!.plan.windows[0]?.usedPercent).toBe(14);
    expect(planProvenance(merged[0]!, NOW + 2_000)?.label).toBe("This computer · Lagos VPS");
  });

  test("never merges two live subscriptions with different active reset times or different tiers", () => {
    const localCodex = plan({
      id: "codex",
      name: "ChatGPT",
      via: "Codex CLI",
      harness: "codex",
      account: null,
      plan: undefined,
      windows: [
        { label: "5-hour", usedPercent: 0, minutes: 300, resetsAt: RESET_5H },
        { label: "Weekly", usedPercent: 92, minutes: 10_080, resetsAt: RESET_WK },
      ],
    });
    const otherCodex = plan({
      id: "codex",
      name: "ChatGPT",
      via: "Codex CLI",
      harness: "codex",
      account: null,
      plan: undefined,
      windows: [
        { label: "5-hour", usedPercent: 0, minutes: 300, resetsAt: RESET_5H + 180_000 },
        // Different active weekly reset proves a second distinct ChatGPT account.
        { label: "Weekly", usedPercent: 15, minutes: 10_080, resetsAt: RESET_WK + 12 * 3_600_000 },
      ],
    });

    const merged = mergeUsagePlans([
      { host: "", machine: "This computer", plans: [localCodex], readAt: NOW },
      { host: "vps", machine: "Lagos VPS", plans: [otherCodex], readAt: NOW },
    ]);

    expect(merged).toHaveLength(2);
    expect(merged[0]!.key).not.toBe(merged[1]!.key);
    expect(planProvenance(merged[0]!, NOW)?.label).toBe("This computer");
    expect(planProvenance(merged[1]!, NOW)?.label).toBe("Lagos VPS");
  });

  test("ignores rolling 0%-used windows when comparing reset times on the same account", () => {
    const localCodex = plan({
      id: "codex",
      name: "ChatGPT",
      via: "Codex CLI",
      harness: "codex",
      account: null,
      plan: undefined,
      windows: [
        { label: "5-hour", usedPercent: 0, minutes: 300, resetsAt: RESET_5H },
        { label: "Weekly", usedPercent: 92, minutes: 10_080, resetsAt: RESET_WK },
      ],
    });
    const remoteCodex = plan({
      id: "codex",
      name: "ChatGPT",
      via: "Codex CLI",
      harness: "codex",
      account: null,
      plan: undefined,
      windows: [
        // Idle 5-hour window shifted by 10 minutes between polls, but active Weekly matches.
        { label: "5-hour", usedPercent: 0, minutes: 300, resetsAt: RESET_5H + 600_000 },
        { label: "Weekly", usedPercent: 92, minutes: 10_080, resetsAt: RESET_WK },
      ],
    });

    const merged = mergeUsagePlans([
      { host: "", machine: "This computer", plans: [localCodex], readAt: NOW },
      { host: "vps", machine: "Lagos VPS", plans: [remoteCodex], readAt: NOW },
    ]);

    expect(merged).toHaveLength(1);
    expect(planProvenance(merged[0]!, NOW)?.label).toBe("This computer · Lagos VPS");
  });

  test("collapses a remote signed-out duplicate into the live local card and hides remote-only signed-out rows", () => {
    const localCodex = plan({
      id: "codex",
      name: "ChatGPT",
      via: "Codex CLI",
      harness: "codex",
      account: null,
      plan: undefined,
    });
    const remoteCodexSignedOut = plan({
      id: "codex",
      name: "ChatGPT",
      via: "Codex CLI",
      harness: "codex",
      account: null,
      plan: undefined,
      status: "signedOut",
      windows: [],
    });
    const remoteOnlyAntigravitySignedOut = plan({
      id: "antigravity",
      name: "Model quotas",
      via: "Antigravity",
      harness: "antigravity",
      account: null,
      plan: undefined,
      status: "signedOut",
      windows: [],
    });

    const merged = mergeUsagePlans([
      { host: "", machine: "This computer", plans: [localCodex], readAt: NOW },
      {
        host: "vps",
        machine: "Lagos VPS",
        plans: [remoteCodexSignedOut, remoteOnlyAntigravitySignedOut],
        readAt: NOW,
      },
    ]);

    expect(merged).toHaveLength(1);
    expect(merged[0]!.plan.id).toBe("codex");
    // Signed-out machine is not claimed as holding the subscription.
    expect(planProvenance(merged[0]!, NOW)?.label).toBe("This computer");
  });

  test("heals a broken local single-login harness (Antigravity TLS error) with a live remote copy", () => {
    const localBroken = plan({
      id: "antigravity",
      name: "Model quotas",
      via: "Antigravity",
      harness: "antigravity",
      account: null,
      plan: undefined,
      status: "unavailable",
      error: "x509: certificate signed by unknown authority",
      windows: [],
    });
    const remoteLive = plan({
      id: "antigravity",
      name: "Model quotas",
      via: "Antigravity",
      harness: "antigravity",
      account: null,
      plan: undefined,
      status: "ok",
      windows: [{ label: "Gemini Models · Weekly", usedPercent: 21, minutes: 10_080, resetsAt: RESET_WK }],
    });

    const merged = mergeUsagePlans([
      { host: "", machine: "This computer", plans: [localBroken], readAt: NOW },
      { host: "vps", machine: "Lagos VPS", plans: [remoteLive], readAt: NOW },
    ]);

    expect(merged).toHaveLength(1);
    expect(merged[0]!.plan.status).toBe("ok");
    expect(merged[0]!.plan.windows[0]?.usedPercent).toBe(21);
    const prov = planProvenance(merged[0]!, NOW);
    expect(prov?.label).toBe("via Lagos VPS");
    expect(prov?.detail).toContain("This computer: could not read");
  });

  test("keeps machine-only accounts separate and groups them under their harness", () => {
    const localParal = plan({ id: "claude:paral-2", name: "Paral 2", account: "paral-2" });
    const remoteWork = plan({
      id: "claude:work",
      name: "Work",
      account: "work",
      plan: "pro",
      windows: [{ label: "5-hour", usedPercent: 65, minutes: 300, resetsAt: RESET_5H + 3_600_000 }],
    });
    const localCodex = plan({
      id: "codex",
      name: "ChatGPT",
      via: "Codex CLI",
      harness: "codex",
      account: null,
      plan: undefined,
    });

    const merged = mergeUsagePlans([
      { host: "", machine: "This computer", plans: [localParal, localCodex], readAt: NOW },
      { host: "vps", machine: "Lagos VPS", plans: [remoteWork], readAt: NOW },
    ]);
    const grouped = groupMergedPlans(merged);

    expect(grouped.map((g) => g.harness)).toEqual(["claude-code", "codex"]);
    expect(grouped[0]!.plans.map((entry) => entry.plan.name)).toEqual(["Paral 2", "Work"]);
    expect(planProvenance(grouped[0]!.plans[1]!, NOW)?.label).toBe("Lagos VPS");
  });

  test("merges Antigravity's quota groups by label, not one group's weekly window with another's", () => {
    const groups = (): UsagePlan =>
      plan({
        id: "antigravity",
        name: "Model quotas",
        via: "Antigravity",
        harness: "antigravity",
        account: null,
        plan: undefined,
        windows: [
          { label: "Gemini Models · Weekly", usedPercent: 67, minutes: 10_080, resetsAt: RESET_WK },
          { label: "Gemini Models · 5-hour", usedPercent: 39, minutes: 300, resetsAt: RESET_5H },
          {
            label: "Claude and GPT models · Weekly",
            usedPercent: 0,
            minutes: 10_080,
            resetsAt: RESET_WK + 1,
          },
          {
            label: "Gemini 4 Argon · Weekly",
            usedPercent: 100,
            minutes: 10_080,
            resetsAt: RESET_WK + 4 * 3_600_000,
          },
        ],
      });

    const merged = mergeUsagePlans(
      [
        { host: "", machine: "This computer", plans: [groups()], readAt: NOW },
        { host: "vps", machine: "Lagos VPS", plans: [groups()], readAt: NOW },
      ],
      NOW,
    );

    expect(merged).toHaveLength(1);
    expect(planProvenance(merged[0]!, NOW)?.label).toBe("This computer · Lagos VPS");
  });

  test("draws the newer numbers, not the answer that arrived last", () => {
    const local = plan({
      windows: [
        { label: "5-hour", usedPercent: 97, minutes: 300, resetsAt: RESET_5H },
        { label: "Weekly", usedPercent: 40, minutes: 10_080, resetsAt: RESET_WK },
      ],
    });
    const alreadyReset = plan({
      windows: [
        { label: "5-hour", usedPercent: 61, minutes: 300, resetsAt: NOW - 3_600_000 },
        { label: "Weekly", usedPercent: 27, minutes: 10_080, resetsAt: RESET_WK },
      ],
    });
    const lessUsedSameWindow = plan({
      windows: [
        { label: "5-hour", usedPercent: 80, minutes: 300, resetsAt: RESET_5H },
        { label: "Weekly", usedPercent: 38, minutes: 10_080, resetsAt: RESET_WK },
      ],
    });

    for (const remote of [alreadyReset, lessUsedSameWindow]) {
      const merged = mergeUsagePlans(
        [
          { host: "", machine: "This computer", plans: [local], readAt: NOW - 120_000 },
          { host: "vps", machine: "Lagos VPS", plans: [remote], readAt: NOW },
        ],
        NOW,
      );
      expect(merged).toHaveLength(1);
      expect(merged[0]!.winnerHost).toBe("");
      expect(merged[0]!.plan.windows[0]?.usedPercent).toBe(97);
    }
  });
});
