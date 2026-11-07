import { describe, expect, test } from "bun:test";
import { BUILTIN_CAPABILITIES, type Harness, type HarnessSnapshot } from "../acp/types";
import {
  accountLoginCommand,
  accountSignIn,
  capabilityRows,
  commandLine,
  controlDisabledReason,
  defaultEffort,
  harnessStatus,
  normalizeDefault,
  orderHarnesses,
  preferredDefault,
  restoreChoice,
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

  test("a harness with named accounts is as ready as its best login", () => {
    const account = (id: string, auth: "ok" | "needs_login" | "unknown") => ({
      id,
      name: id,
      config_dir: `/tmp/${id}`,
      auth,
    });
    const signedIn: Harness = {
      ...base,
      auth: "needs_login",
      accounts: [account("work", "needs_login"), account("home", "ok")],
    };
    expect(harnessStatus(signedIn)).toEqual({ tone: "ok", label: "Ready" });
    expect(unavailableReason(signedIn)).toBeUndefined();

    const signedOut: Harness = {
      ...base,
      auth: "ok",
      accounts: [account("work", "needs_login"), account("home", "needs_login")],
    };
    expect(harnessStatus(signedOut)).toEqual({ tone: "warn", label: "Sign in an account" });
    expect(unavailableReason(signedOut)).toBe("Sign in an account");

    // Not looked at yet is not a reason to block or to claim it is signed out.
    const unchecked: Harness = {
      ...base,
      auth: "needs_login",
      accounts: [account("work", "unknown")],
    };
    expect(unavailableReason(unchecked)).toBeUndefined();
    expect(harnessStatus(unchecked).tone).toBe("ok");
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

  test("a named login is signed in with its own harness command and variable", () => {
    const codex: Harness = {
      ...base,
      account_env: "CODEX_HOME",
      account_login_command: ["codex", "login"],
    };
    const claude: Harness = {
      ...base,
      id: "claude-code",
      name: "Claude Code",
      account_env: "CLAUDE_CONFIG_DIR",
      account_login_command: ["claude", "auth", "login"],
    };
    expect(accountSignIn(codex)).toEqual({ env: "CODEX_HOME", command: ["codex", "login"] });
    expect(accountSignIn(claude)).toEqual({
      env: "CLAUDE_CONFIG_DIR",
      command: ["claude", "auth", "login"],
    });
    // The copyable line names the account's directory, and nothing when it is
    // the harness's own login.
    expect(accountLoginCommand(codex, { config_dir: "~/.codex-paral", own_dir: false })).toBe(
      "mkdir -p ~/.codex-paral && CODEX_HOME=~/.codex-paral codex login",
    );
    expect(accountLoginCommand(claude, { config_dir: "~/.claude-paral-2", own_dir: false })).toBe(
      "mkdir -p ~/.claude-paral-2 && CLAUDE_CONFIG_DIR=~/.claude-paral-2 claude auth login",
    );
    expect(accountLoginCommand(claude, { config_dir: "~/.claude", own_dir: true })).toBe("claude auth login");
    // A harness with no named logins has nothing to sign in.
    expect(accountSignIn(base)).toBeUndefined();
    expect(accountLoginCommand(base, { config_dir: "/tmp/x", own_dir: false })).toBe("");
  });

  test("a harness's remembered choice is restored, and a stale one falls back quietly", () => {
    const claude: Harness = {
      ...base,
      id: "claude-code",
      name: "Claude Code",
      models: [
        { id: "opus", name: "Opus", efforts: ["low", "high"], default_effort: "low" },
        { id: "sonnet", name: "Sonnet", efforts: ["low", "medium"], default_effort: "low" },
      ],
      last: { model: "opus", effort: "high" },
    };
    const snap = snapshot([base, claude]);
    expect(restoreChoice(snap, "claude-code")).toEqual({
      harness: "claude-code",
      model: "opus",
      effort: "high",
    });
    // Nothing remembered: the harness picks (no model, no effort).
    expect(restoreChoice(snap, "codex")).toEqual({ harness: "codex", model: null, effort: null });
    // The app's own record, for an engine that keeps none.
    expect(restoreChoice(snap, "codex", { codex: { model: "gpt-6-luna", effort: "low" } })).toEqual({
      harness: "codex",
      model: "gpt-6-luna",
      effort: "low",
    });
    // A model the harness no longer lists, or an effort it does not offer, is dropped.
    const stale = snapshot([{ ...claude, last: { model: "gone", effort: "max" } }]);
    expect(restoreChoice(stale, "claude-code")).toEqual({
      harness: "claude-code",
      model: null,
      effort: null,
    });
    const badEffort = snapshot([{ ...claude, last: { model: "sonnet", effort: "high" } }]);
    expect(restoreChoice(badEffort, "claude-code")).toEqual({
      harness: "claude-code",
      model: "sonnet",
      effort: null,
    });
    // The default applies to its own harness.
    const withDefault = {
      ...snapshot([base]),
      default: { harness: "codex", model: "gpt-6-luna", effort: "low" },
    };
    expect(restoreChoice(withDefault, "codex")).toEqual({
      harness: "codex",
      model: "gpt-6-luna",
      effort: "low",
    });
  });

  test("the default effort is the model's, else the provider default, else the first", () => {
    expect(defaultEffort(base.models[0], ["low", "medium"])).toBe("medium");
    expect(defaultEffort(undefined, ["low", "medium", "high", "default"])).toBe("default");
    expect(defaultEffort(undefined, ["low", "high"])).toBe("low");
    expect(defaultEffort(undefined, [])).toBeNull();
  });
});

describe("the desktop app's preferred harness", () => {
  const rustcode: Harness = { ...base, id: "rustcode", name: "RustCode", builtin: true };
  const claude: Harness = { ...base, id: "claude-code", name: "Claude Code", kind: "acp" };

  test("Claude Code first, RustCode last", () => {
    expect(orderHarnesses([rustcode, base, claude]).map((h) => h.id)).toEqual([
      "claude-code",
      "codex",
      "rustcode",
    ]);
  });

  test("a first run starts on Claude Code, else another agent, else RustCode", () => {
    expect(preferredDefault(snapshot([rustcode, base, claude])).harness).toBe("claude-code");
    expect(preferredDefault(snapshot([rustcode, base, { ...claude, installed: false }])).harness).toBe(
      "codex",
    );
    expect(preferredDefault(snapshot([rustcode, { ...base, auth: "needs_login" }])).harness).toBe("rustcode");
    // A harness whose own login is missing but whose account is signed in can run.
    const withAccount = {
      ...claude,
      auth: "needs_login" as const,
      accounts: [{ id: "work", name: "Work", config_dir: "/tmp/work", auth: "ok" as const }],
    };
    expect(preferredDefault(snapshot([rustcode, withAccount])).harness).toBe("claude-code");
  });
});
