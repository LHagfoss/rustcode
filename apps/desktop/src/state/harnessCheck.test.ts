import { describe, expect, test } from "bun:test";
import { BUILTIN_CAPABILITIES, type Harness } from "../acp/types";
import { currentResult, failedResult, readyResult, resultTone, settledEarly } from "./harnessCheck";

const base: Harness = {
  id: "claude-code",
  name: "Claude Code",
  kind: "acp",
  builtin: false,
  known: true,
  enabled: true,
  command: ["claude-agent-acp"],
  installed: true,
  auth: "unknown",
  capabilities: BUILTIN_CAPABILITIES,
  models: [],
};
const account = (id: string, auth: "ok" | "needs_login" | "unknown") => ({
  id,
  name: id,
  config_dir: `/tmp/${id}`,
  auth,
});

describe("what a Check settles before it lists models", () => {
  test("a harness that is not installed ends as Not installed", () => {
    expect(settledEarly({ ...base, installed: false })).toEqual({
      state: "not_installed",
      text: "Not installed",
    });
  });

  test("a custom command that is not found names the program", () => {
    expect(settledEarly({ ...base, known: false, installed: false, command: ["dsh", "acp"] })?.text).toBe(
      "Not installed: dsh was not found on PATH",
    );
  });

  test("a signed-out harness ends as Needs sign-in without being started", () => {
    expect(settledEarly({ ...base, auth: "needs_login" })).toMatchObject({ state: "needs_login" });
    expect(
      settledEarly({ ...base, accounts: [account("a", "needs_login"), account("b", "needs_login")] }),
    ).toMatchObject({
      state: "needs_login",
      text: "Needs sign-in: sign in an account",
    });
  });

  test("one signed-in account is enough to go on, and so is not knowing", () => {
    expect(
      settledEarly({
        ...base,
        auth: "needs_login",
        accounts: [account("a", "ok"), account("b", "needs_login")],
      }),
    ).toBeNull();
    expect(settledEarly({ ...base, accounts: [account("a", "unknown")] })).toBeNull();
    expect(settledEarly(base)).toBeNull();
  });
});

describe("where a Check ends", () => {
  test("listing models is Ready, with the version and the count", () => {
    expect(readyResult(13, "2.1.2")).toEqual({ state: "ready", text: "Ready · 2.1.2 · 13 models" });
    expect(readyResult(1).text).toBe("Ready · 1 model");
    expect(readyResult(0).text).toBe("Ready · no model list");
  });

  test("a failure keeps its text, and never ends empty", () => {
    expect(failedResult(new Error("spawn dsh ENOENT"))).toEqual({ state: "error", text: "spawn dsh ENOENT" });
    expect(failedResult("handshake timed out").text).toBe("handshake timed out");
    expect(failedResult(new Error("  ")).state).toBe("error");
    expect(failedResult(new Error("  ")).text).not.toBe("");
  });

  test("a card shows the result unless the harness changed since", () => {
    const ready = { result: readyResult(3) };
    expect(currentResult(base, ready)).toEqual(ready.result);
    expect(currentResult({ ...base, installed: false }, ready)?.state).toBe("not_installed");
    expect(currentResult({ ...base, auth: "needs_login" }, ready)?.state).toBe("needs_login");
    const error = { result: failedResult("boom") };
    expect(currentResult(base, error)).toEqual(error.result);
    expect(currentResult(base, { running: "models" })).toBeNull();
    expect(currentResult(base, undefined)).toBeNull();
  });

  test("tones", () => {
    expect(resultTone(readyResult(2))).toBe("ok");
    expect(resultTone(failedResult("x"))).toBe("warn");
    expect(resultTone({ state: "needs_login", text: "" })).toBe("warn");
    expect(resultTone({ state: "not_installed", text: "" })).toBe("off");
  });
});
