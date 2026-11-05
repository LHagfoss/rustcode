import { describe, expect, test } from "bun:test";
import type { UsagePlan } from "../acp/rustcode";
import type { Harness, HarnessAccount } from "../acp/types";
import {
  accountLabelOf,
  accountNameOf,
  accountSignedOut,
  childAccountFor,
  childAccountOn,
  defaultLoginGone,
  firstSignedInAccount,
  namedAccountOn,
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

  test("an answer that read one account says nothing about the logins it skipped", () => {
    const before = { "claude-code": true, "claude-code#home": true };
    // The app reads one login at a time: the rows of the others are absent, and
    // the default login's row says it was not asked.
    const record = signedOutFrom(
      [plan({ status: "notAsked" }), plan({ id: "claude:work", account: "work", status: "ok" })],
      before,
    );
    expect(record).toEqual(before);
    // A row that is there is still read: that account signed out.
    expect(
      signedOutFrom([plan({ id: "claude:work", account: "work", status: "signedOut" })], before),
    ).toEqual({ ...before, "claude-code#work": true });
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

  test("runs on the named login it was told to use, by id or by name", () => {
    const work = { ...account("work"), name: "Work" };
    const h = claude([work, account("home")]);
    // A named login wins over the parent's, since the agent asked for it.
    expect(childAccountFor(h, "work", { harness: "claude-code", account: "home" }, {})).toEqual({
      account: "work",
    });
    // A display name picks the same login, whatever its case.
    expect(childAccountFor(h, "Work", undefined, {})).toEqual({ account: "work" });
    expect(namedAccountOn(h, "HOME")).toEqual({ kind: "one", account: h.accounts![1]! });
    expect(accountNameOf(h, "work")).toBe("Work");
    expect(accountNameOf(h, "gone")).toBe("gone");
    expect(accountNameOf(h, null)).toBeUndefined();
    // The approval card names both the display name and the id.
    expect(accountLabelOf(h, "work")).toBe("Work (work)");
    expect(accountLabelOf(h, "home")).toBe("home");
    expect(accountLabelOf(h, null)).toBeUndefined();
  });

  test("refuses an account the harness does not list, never a quiet fall back", () => {
    const h = claude([account("work")]);
    expect(childAccountFor(h, "home", undefined, {})).toEqual({
      refused: { kind: "unknown", asked: "home", available: ["work"] },
    });
    expect(childAccountFor(claude(), "work", undefined, {})).toEqual({
      refused: { kind: "unknown", asked: "work", available: [] },
    });
    // Without a name, the inherited rules are exactly as they were.
    expect(childAccountFor(h, undefined, { harness: "claude-code", account: "work" }, {})).toEqual({
      account: "work",
    });
    expect(childAccountFor(h, "  ", { harness: "codex", account: "home" }, {})).toEqual({ account: null });
  });

  test("refuses an ambiguous name rather than picking one login", () => {
    // Two logins whose display name is the same.
    const h = claude([
      { ...account("work"), name: "Paral" },
      { ...account("home"), name: "Paral" },
      { ...account("other"), name: "other" },
    ]);
    expect(childAccountFor(h, "paral", undefined, {})).toEqual({
      refused: { kind: "ambiguous", asked: "paral", matches: ["work", "home"] },
    });
  });

  test("an exact id stays reachable when another login is named the same", () => {
    const clash = claude([
      { ...account("work"), name: "Work" },
      { ...account("other"), name: "work" },
    ]);
    // The exact id is the most precise name and wins over the other account's display name.
    expect(namedAccountOn(clash, "work")).toEqual({ kind: "one", account: clash.accounts![0]! });
    expect(childAccountFor(clash, "work", undefined, {})).toEqual({ account: "work" });
    expect(childAccountFor(clash, "Work", undefined, {})).toEqual({ account: "work" });
  });

  test("refuses ids that only match case-insensitively more than once", () => {
    const h = claude([account("work"), account("Work")]);
    expect(namedAccountOn(h, "WORK")).toEqual({ kind: "ambiguous", ids: ["work", "Work"] });
    expect(childAccountFor(h, "WORK", undefined, {})).toEqual({
      refused: { kind: "ambiguous", asked: "WORK", matches: ["work", "Work"] },
    });
    // The exact spelling still picks its own id.
    expect(childAccountFor(h, "Work", undefined, {})).toEqual({ account: "Work" });
  });

  test("refuses an explicitly signed-out login", () => {
    const h = claude([{ ...account("work"), name: "Work" }, account("home")]);
    // The plan read keys a named login as `harness#account`.
    const signedOut = { "claude-code#work": true };
    expect(accountSignedOut("claude-code", "work", signedOut)).toBe(true);
    expect(accountSignedOut("claude-code", "home", signedOut)).toBe(false);
    expect(childAccountFor(h, "work", undefined, signedOut)).toEqual({
      refused: { kind: "signedOut", account: "work", name: "Work", available: ["work", "home"] },
    });
    // Another login is unaffected, and the default login is not an account.
    expect(childAccountFor(h, "home", undefined, signedOut)).toEqual({ account: "home" });
    expect(childAccountFor(h, "work", undefined, { "claude-code": true })).toEqual({ account: "work" });
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

  test("draws only the accounts an answer read", () => {
    // One account at a time: the page's rows are what came back, and the
    // switch lands in the first Claude group that is there.
    const rows = usageRows([plan({ id: "claude:work", account: "work" })]);
    expect(rows.plans.map((p) => p.id)).toEqual(["claude:work"]);
    expect(rows.switchIn).toBe("claude:work");
    // A row the answer skipped is not reported as signed out, so nothing is dropped.
    expect(rows.plans).toHaveLength(1);
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
