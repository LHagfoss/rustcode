// The hang guards of Settings on other machines: a read that failed is not
// started again by the page, and a Check on a machine that does not answer ends
// with one error per harness instead of waiting on each in turn.

import { describe, expect, test } from "bun:test";
import { checkMachineHarnesses, harnessCheckLine, shouldAutoLoadHarness } from "./settingsScope";
import { LOCAL_HOST } from "./threadKeys";

describe("shouldAutoLoadHarness", () => {
  const idle = { remote: undefined, loading: false, error: null };

  test("a machine not read yet is read once", () => {
    expect(shouldAutoLoadHarness("studio", idle)).toBe(true);
  });

  test("a failed read is not started again by the page; Retry does that", () => {
    expect(shouldAutoLoadHarness("studio", { ...idle, error: "That machine is not connected." })).toBe(false);
  });

  test("nothing starts while a read runs, or for this machine", () => {
    expect(shouldAutoLoadHarness("studio", { ...idle, loading: true })).toBe(false);
    expect(shouldAutoLoadHarness(LOCAL_HOST, idle)).toBe(false);
  });
});

describe("harnessCheckLine", () => {
  test("says where a harness ended", () => {
    expect(harnessCheckLine(undefined)).toContain("not listed");
    expect(harnessCheckLine({ installed: false, auth: "unknown" })).toBe("Not installed on that machine.");
    expect(harnessCheckLine({ installed: true, auth: "needs_login" })).toBe("Needs sign-in on that machine.");
    expect(harnessCheckLine({ installed: true, auth: "ok" })).toBe("Installed and signed in.");
  });
});

describe("checkMachineHarnesses", () => {
  test("a machine that is not connected answers every harness at once, without waiting", async () => {
    const started = Date.now();
    const lines = await checkMachineHarnesses("no-such-machine", ["claude-code", "codex", "gemini"]);
    expect(Object.keys(lines)).toEqual(["claude-code", "codex", "gemini"]);
    for (const line of Object.values(lines)) expect(line).toContain("not connected");
    expect(Date.now() - started).toBeLessThan(1_000);
  });

  test("no harness, no request", async () => {
    expect(await checkMachineHarnesses("no-such-machine", [])).toEqual({});
  });
});
