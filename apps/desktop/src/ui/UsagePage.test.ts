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
          error: "Run a Claude Code thread on this account to refresh its usage.",
          windows: [],
        },
        // Numbers RustCode recorded before the login's token lapsed say when
        // they were read, in seconds on the wire and ms in the app.
        {
          id: "claude:personal",
          name: "Personal 1",
          harness: "claude-code",
          account: "personal",
          status: "ok",
          plan: "pro",
          asOf: 1_762_500_000,
          windows: [{ label: "5-hour", usedPercent: 41.5, windowMinutes: 300 }],
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
      "claude:personal",
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
    expect(plans[3]).toMatchObject({ status: "ok", asOf: 1_762_500_000_000 });
    expect(plans[3]!.windows[0]!.usedPercent).toBe(41.5);
    expect(plans[4]!.reason).toBeUndefined();
    expect(plans[5]!.reason).toBe("forbidden");
    // A row read now carries no as-of, so it is drawn as this minute's.
    expect(plans[0]!.asOf).toBeUndefined();
  });
});
