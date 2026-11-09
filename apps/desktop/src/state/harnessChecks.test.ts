import { beforeEach, describe, expect, test } from "bun:test";
import { MockTransport } from "../acp/mock";
import { runHarnessCheck, useHarnessChecks } from "./harnessChecks";
import {
  addCustomHarness,
  connect,
  discardCustomHarness,
  enableCustomHarness,
  removeHarnessAccount,
  resetForTests,
  useApp,
} from "./store";

const until = async (check: () => boolean, ms = 2000) => {
  const start = Date.now();
  while (!check()) {
    if (Date.now() - start > ms) throw new Error("timed out waiting");
    await new Promise((r) => setTimeout(r, 2));
  }
};

const find = (id: string) => useApp.getState().harnesses?.harnesses.find((h) => h.id === id);

async function ready() {
  await connect(new MockTransport({ delayMs: 0, scenario: "onboarding" }));
  await until(() => useApp.getState().harnesses !== null);
}

beforeEach(() => {
  resetForTests();
  useHarnessChecks.setState({ byHarness: {} });
});

describe("Check", () => {
  test("a harness that runs ends Ready with its models", async () => {
    await ready();
    const result = await runHarnessCheck("codex");
    expect(result.state).toBe("ready");
    expect(result.text).toMatch(/^Ready/);
    expect(find("codex")?.models.length).toBeGreaterThan(0);
    expect(useHarnessChecks.getState().byHarness.codex).toEqual({ result });
  });

  test("a signed-out harness ends Needs sign-in instead of an error", async () => {
    await ready();
    // Its one signed-in account goes, which leaves the harness's own sign-in: missing.
    await removeHarnessAccount("claude-code", "paral-2");
    expect(await runHarnessCheck("claude-code")).toEqual({ state: "needs_login", text: "Needs sign-in" });
  });

  test("pressing Check twice runs one check", async () => {
    await ready();
    expect(runHarnessCheck("codex")).toBe(runHarnessCheck("codex"));
    await until(() => !("running" in (useHarnessChecks.getState().byHarness.codex ?? {})));
  });

  test("an unknown harness ends in an error with text, not in limbo", async () => {
    await ready();
    const result = await runHarnessCheck("nope");
    expect(result.state).toBe("error");
    expect(result.text).not.toBe("");
    expect("running" in (useHarnessChecks.getState().byHarness.nope ?? {})).toBe(false);
  });
});

describe("a custom ACP harness", () => {
  const add = (command: string[], env: Record<string, string> = {}) =>
    addCustomHarness({ id: "dsh", name: "Dsh", command, env });

  test("is added off, verified by a Check, then turned on", async () => {
    await ready();
    await add(["dsh", "--profile", "acp"], { DSH_PROFILE: "work" });
    expect(find("dsh")).toMatchObject({
      known: false,
      enabled: false,
      command: ["dsh", "--profile", "acp"],
      env_names: ["DSH_PROFILE"],
    });
    expect((await runHarnessCheck("dsh")).state).toBe("ready");
    await enableCustomHarness("dsh");
    expect(find("dsh")?.enabled).toBe(true);
    expect(find("dsh")?.models.length).toBeGreaterThan(0);
  });

  test("a command that is not found ends Not installed, naming the program", async () => {
    await ready();
    await add(["missing"]);
    const result = await runHarnessCheck("dsh");
    expect(result).toEqual({ state: "not_installed", text: "Not installed: missing was not found on PATH" });
  });

  test("a command that never answers ACP ends in the error text and can be taken out again", async () => {
    await ready();
    await add(["broken"]);
    const result = await runHarnessCheck("dsh");
    expect(result.state).toBe("error");
    expect(result.text).toMatch(/handshake/);
    await discardCustomHarness("dsh");
    expect(find("dsh")).toBeUndefined();
  });
});
