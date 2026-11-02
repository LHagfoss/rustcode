import { describe, expect, test } from "bun:test";
import { parsePlans } from "./UsagePage";

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
        { status: "ok" },
      ],
    });
    expect(plans.map((p) => p.id)).toEqual(["claude", "codex"]);
    expect(plans[0]!.windows[0]).toEqual({
      label: "5-hour",
      usedPercent: 18,
      minutes: 300,
      resetsAt: 1_000_000,
    });
    expect(plans[1]!.status).toBe("signedOut");
  });
});
