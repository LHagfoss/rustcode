import { describe, expect, test } from "bun:test";
import type { UsagePlan } from "../acp/rustcode";
import type { Harness, HarnessAccount } from "../acp/types";
import {
  childAccountOn,
  defaultLoginGone,
  firstSignedInAccount,
  newThreadAccountOn,
  signedOutFrom,
  usageRows,
} from "./logins";

const plan = (over: Partial<UsagePlan>): UsagePlan => ({
  id: "claude",
  name: "Claude",
  via: "Claude Code",
  harness: "claude-code",
  account: null,
  status: "ok",
  windows: [],
  ...over,
});

const account = (id: string, auth: HarnessAccount["auth"] = "ok"): HarnessAccount => ({
  id,
  name: id,
  config_dir: `/tmp/${id}`,
  auth,
});

const claude = (accounts: HarnessAccount[] = []): Harness => ({
  id: "claude-code",
  name: "Claude Code",
  kind: "acp",
  builtin: false,
  enabled: true,
  command: ["claude"],
  installed: true,
  auth: "unknown",
  capabilities: {
    steer: "none",
    cancel: true,
    resume: true,
    rewind: false,
    images: true,
    models: "dynamic",
    effort: false,
    permission_modes: [],
    subagents: false,
    slash_commands: true,
    smart_ui: false,
    rustcode_extensions: false,
  },
  models: [],
  accounts,
});

describe("signed out logins", () => {
  test("marks a login the answer read as signed out, and clears it when it reads again", () => {
    const once = signedOutFrom([plan({ status: "signedOut" })]);
    expect(once).toEqual({ "claude-code": true });
    expect(signedOutFrom([plan({ status: "ok" })], once)).toEqual({});
    // Signed in but unreadable is not signed out.
    expect(signedOutFrom([plan({ status: "unavailable" })], once)).toEqual({});
  });

  test("notAsked says nothing and keeps what an earlier answer found", () => {
    const before = { "claude-code": true };
    expect(signedOutFrom([plan({ status: "notAsked" })], before)).toEqual(before);
  });

  test("a login it could not read is not a sign-out", () => {
    // A denied Keychain prompt, a locked Keychain: signed in as far as anyone knows.
    const denied = plan({ status: "unavailable", error: "macOS did not allow reading it." });
    expect(signedOutFrom([denied], { "claude-code": true })).toEqual({});
    // So its group is drawn (with the reason), its switch stays in it, and
    // "Default login" is still offered.
    const rows = usageRows([denied, plan({ id: "claude:work", account: "work" })]);
    expect(rows.plans.map((p) => p.id)).toEqual(["claude", "claude:work"]);
    expect(rows.switchIn).toBe("claude");
    expect(newThreadAccountOn(claude([account("work")]), null, signedOutFrom([denied]))).toBeNull();
  });

  test("keeps each login apart", () => {
    const record = signedOutFrom([
      plan({ id: "claude:work", account: "work", status: "ok" }),
      plan({ status: "signedOut" }),
    ]);
    expect(record).toEqual({ "claude-code": true });
  });
});

describe("the default login", () => {
  test("is gone only when it is signed out and a named account exists", () => {
    expect(defaultLoginGone(claude([account("work")]), { "claude-code": true })).toBe(true);
    expect(defaultLoginGone(claude([account("work")]), {})).toBe(false);
    expect(defaultLoginGone(claude(), { "claude-code": true })).toBe(false);
    expect(defaultLoginGone(undefined, { "claude-code": true })).toBe(false);
  });

  test("a new thread starts on the chosen account while it is there", () => {
    const h = claude([account("work"), account("home")]);
    expect(newThreadAccountOn(h, "home", { "claude-code": true })).toBe("home");
    expect(newThreadAccountOn(h, "gone", { "claude-code": true })).toBe("work");
  });

  test("a new thread falls back to the first signed-in account once the default is gone", () => {
    const h = claude([account("work", "unknown"), account("home")]);
    expect(newThreadAccountOn(h, null, { "claude-code": true })).toBe("home");
    // No sign-in found anywhere: the first named account still beats a login known to fail.
    expect(newThreadAccountOn(claude([account("work", "unknown")]), null, { "claude-code": true })).toBe(
      "work",
    );
  });

  test("a new thread keeps the default login while it can run", () => {
    expect(newThreadAccountOn(claude([account("work")]), null, {})).toBeNull();
  });

  test("firstSignedInAccount prefers a login whose credentials were found", () => {
    expect(firstSignedInAccount([account("a", "unknown"), account("b")])).toBe("b");
    expect(firstSignedInAccount([])).toBeNull();
    expect(firstSignedInAccount(undefined)).toBeNull();
  });
});

describe("a thread started by another thread", () => {
  test("inherits the parent's account on the same harness", () => {
    const h = claude([account("work"), account("home")]);
    expect(childAccountOn(h, { harness: "claude-code", account: "home" }, {})).toBe("home");
  });

  test("uses a named account when the parent is on the default login, which is signed out", () => {
    const h = claude([account("work")]);
    expect(childAccountOn(h, { harness: "claude-code", account: null }, { "claude-code": true })).toBe(
      "work",
    );
  });

  test("never lends an account to another harness", () => {
    const h = claude([account("work")]);
    // Another harness's account is not one of this harness's, so the default login stands.
    expect(childAccountOn(h, { harness: "codex", account: "home" }, {})).toBeNull();
  });
});

describe("Settings › Usage groups", () => {
  test("drops the signed-out default login when named accounts are there", () => {
    const rows = usageRows([
      plan({ status: "signedOut" }),
      plan({ id: "claude:work", name: "Work", account: "work" }),
      plan({ id: "codex", name: "ChatGPT", harness: "codex", via: "Codex CLI" }),
    ]);
    expect(rows.plans.map((p) => p.id)).toEqual(["claude:work", "codex"]);
    // The switch that lived in the dropped group moves to the first Claude group left.
    expect(rows.switchIn).toBe("claude:work");
  });

  test("keeps the signed-out default login when it is the only Claude login", () => {
    const rows = usageRows([plan({ status: "signedOut" })]);
    expect(rows.plans.map((p) => p.id)).toEqual(["claude"]);
    expect(rows.switchIn).toBe("claude");
  });

  test("keeps it when it is signed in, and draws the switch where it always was", () => {
    const rows = usageRows([plan({ status: "ok" }), plan({ id: "claude:work", account: "work" })]);
    expect(rows.plans.map((p) => p.id)).toEqual(["claude", "claude:work"]);
    expect(rows.switchIn).toBe("claude");
  });

  test("drops nothing when the meter is off and the default login was not read", () => {
    const rows = usageRows([plan({ status: "notAsked" }), plan({ id: "claude:work", account: "work" })]);
    expect(rows.plans.map((p) => p.id)).toEqual(["claude", "claude:work"]);
  });

  test("draws the switch once, in the first remaining Claude group", () => {
    const rows = usageRows([
      plan({ id: "codex", name: "ChatGPT", harness: "codex" }),
      plan({ status: "signedOut" }),
      plan({ id: "claude:work", account: "work" }),
      plan({ id: "claude:home", account: "home" }),
    ]);
    expect(rows.switchIn).toBe("claude:work");
    expect(rows.plans.filter((p) => p.id === rows.switchIn)).toHaveLength(1);
  });
});
