import { describe, expect, test } from "bun:test";
import { BUILTIN_CAPABILITIES, type Harness, type HarnessSnapshot } from "../acp/types";
import {
  capabilityRows,
  commandLine,
  controlDisabledReason,
  normalizeDefault,
  unavailableReason,
  usableHarnesses,
  validateDefault,
} from "./harness";

const base: Harness = {
  id: "codex",
  name: "Codex",
  kind: "codex",
  builtin: false,
  enabled: true,
  command: ["codex", "app-server"],
  installed: true,
  auth: "ok",
  capabilities: { ...BUILTIN_CAPABILITIES, steer: "next_turn", effort: true },
  models: [{ id: "gpt-6-luna", name: "gpt-6-luna", efforts: ["low", "medium"], default_effort: "medium" }],
};

const snapshot = (harnesses: Harness[]): HarnessSnapshot => ({
  harnesses,
  default: { harness: "rustcode", model: null, effort: null },
  onboarded: false,
});

describe("harness logic", () => {
  test("unavailable reasons", () => {
    expect(unavailableReason({ ...base, installed: false, install_hint: "brew install codex" })).toMatch(
      /brew install codex/,
    );
    expect(unavailableReason({ ...base, enabled: false })).toMatch(/Turned off/);
    expect(unavailableReason({ ...base, auth: "needs_login" })).toBe("Sign in first");
    expect(unavailableReason(base)).toBeUndefined();
  });

  test("only usable harnesses can run a thread", () => {
    const list = usableHarnesses(snapshot([base, { ...base, id: "x", installed: false }]));
    expect(list.map((h) => h.id)).toEqual(["codex"]);
  });

  test("validates a default choice", () => {
    const s = snapshot([base]);
    expect(validateDefault(s, { harness: "codex", model: "gpt-6-luna", effort: "low" })).toBeUndefined();
    expect(validateDefault(s, { harness: "codex", model: "nope", effort: null })).toMatch(/does not list/);
    expect(validateDefault(s, { harness: "codex", model: "gpt-6-luna", effort: "xhigh" })).toMatch(
      /no xhigh/,
    );
    expect(validateDefault(s, { harness: "missing", model: null, effort: null })).toBe("Choose a harness");
    const noEffort = snapshot([
      { ...base, capabilities: { ...base.capabilities, effort: false }, models: [] },
    ]);
    expect(validateDefault(noEffort, { harness: "codex", model: null, effort: "low" })).toMatch(/effort/);
  });

  test("normalizes a default after the harness changes", () => {
    const s = snapshot([base]);
    expect(normalizeDefault(s, { harness: "codex", model: null, effort: null })).toEqual({
      harness: "codex",
      model: "gpt-6-luna",
      effort: "medium",
    });
    expect(normalizeDefault(s, { harness: "codex", model: "gpt-6-luna", effort: "low" })).toEqual({
      harness: "codex",
      model: "gpt-6-luna",
      effort: "low",
    });
    const noEffort = snapshot([{ ...base, capabilities: { ...base.capabilities, effort: false } }]);
    expect(
      normalizeDefault(noEffort, { harness: "codex", model: "gpt-6-luna", effort: "low" }).effort,
    ).toBeNull();
  });

  test("greys out controls by capability, never by id", () => {
    expect(controlDisabledReason("steer", base.capabilities, "Codex")).toMatch(/queues/);
    expect(controlDisabledReason("effort", base.capabilities)).toBeUndefined();
    expect(capabilityRows(base.capabilities).find((r) => r.key === "steer")?.detail).toBe(
      "Read at the next turn",
    );
  });

  test("quotes a login command for copying", () => {
    expect(commandLine(["claude", "/login"])).toBe("claude /login");
    expect(commandLine(["sh", "-c", "a b"])).toBe("sh -c 'a b'");
  });
});
