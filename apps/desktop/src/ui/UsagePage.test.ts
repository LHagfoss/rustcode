import { describe, expect, test } from "bun:test";
import { parsePlans } from "../acp/rustcode";

describe("usage plans", () => {
  test("reads each plan and its windows", () => {
    const plans = parsePlans({
      plans: [
        {
          id: "claude",
          name: "Claude",
          via: "Claude Code",
          status: "ok",
          plan: "max",
          windows: [{ label: "5-hour", usedPercent: 18, windowMinutes: 300, resetsAt: 1000 }],
        },
        { id: "codex", name: "ChatGPT", status: "signedOut", windows: [] },
        {
          id: "claude:work",
          name: "Work",
          harness: "claude-code",
          account: "work",
          status: "unavailable",
          reason: "expired",
          error: "Open Claude Code to renew it, then Refresh.",
          windows: [],
        },
        // A reason this app does not know is no reason at all, never a crash.
        { id: "claude:home", name: "Home", status: "unavailable", reason: "brand-new", windows: [] },
        { id: "claude:office", name: "Office", status: "unavailable", reason: "forbidden", windows: [] },
        { status: "ok" },
      ],
    });
    expect(plans.map((p) => p.id)).toEqual([
      "claude",
      "codex",
      "claude:work",
      "claude:home",
      "claude:office",
    ]);
    expect(plans[0]!.windows[0]).toEqual({
      label: "5-hour",
      usedPercent: 18,
      minutes: 300,
      resetsAt: 1_000_000,
    });
    expect(plans[1]!.status).toBe("signedOut");
    expect(plans[2]).toMatchObject({ status: "unavailable", reason: "expired", account: "work" });
    expect(plans[3]!.reason).toBeUndefined();
    expect(plans[4]!.reason).toBe("forbidden");
  });
});
