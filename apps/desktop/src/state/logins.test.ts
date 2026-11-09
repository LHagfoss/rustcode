import { describe, expect, test } from "bun:test";
import type { UsagePlan } from "../acp/rustcode";
import type { Harness, HarnessAccount, HarnessSnapshot } from "../acp/types";
import {
  accountLabelOf,
  accountNameOf,
  accountSignedOut,
  accountToSignIn,
  childAccountFor,
  childAccountOn,
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
  known: true,
  enabled: true,
  command: ["claude"],
  installed: true,
  auth: "unknown",
  capabilities: {
    steer: "none",
    steering: {
      live: false,
      attachments: false,
      while_awaiting_user: true,
      interrupt_restart: true,
      queue: true,
    },
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
    // Named accounts carry the thread all the same: a login nobody could read
    // is not a reason to offer the harness's own sign-in again.
    const rows = usageRows([denied, plan({ id: "claude:work", account: "work" })]);
    expect(rows.plans.map((p) => p.id)).toEqual(["claude:work"]);
    expect(newThreadAccountOn(claude([account("work")]), null)).toBe("work");
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

describe("the harness's own login", () => {
  test("is not offered once a named account exists", () => {
    // Whatever was stored, a null choice is not used while named accounts can carry the thread.
    expect(newThreadAccountOn(claude([account("work")]), null)).toBe("work");
    expect(newThreadAccountOn(claude([account("work")]), "gone")).toBe("work");
  });

  test("is the only choice when the harness has no named accounts", () => {
    expect(newThreadAccountOn(claude(), null)).toBeNull();
    expect(newThreadAccountOn(undefined, null)).toBeNull();
  });

  test("a new thread starts on the chosen account while it is there", () => {
    const h = claude([account("work"), account("home")]);
    expect(newThreadAccountOn(h, "home")).toBe("home");
    expect(newThreadAccountOn(h, "gone")).toBe("work");
  });

  test("a new thread falls back to the first signed-in account", () => {
    const h = claude([account("work", "unknown"), account("home")]);
    expect(newThreadAccountOn(h, null)).toBe("home");
    // A look found no sign-in anywhere yet: the first named account still carries the thread.
    expect(newThreadAccountOn(claude([account("work", "needs_login")]), null)).toBe("work");
  });

  test("firstSignedInAccount prefers a login whose credentials were found", () => {
    expect(firstSignedInAccount([account("a", "unknown"), account("b")])).toBe("b");
    expect(firstSignedInAccount([account("a", "needs_login")])).toBe("a");
    expect(firstSignedInAccount([])).toBeNull();
    expect(firstSignedInAccount(undefined)).toBeNull();
  });

  test("a login the plan read found signed out is the last resort", () => {
    const h = claude([account("work"), account("home")]);
    const signedOut = { "claude-code#work": true };
    // Not looked at beats known signed out, which beats a harness login there is not.
    expect(firstSignedInAccount([account("a", "unknown"), account("b", "ok")])).toBe("b");
    expect(
      firstSignedInAccount([account("a", "unknown"), account("b")], "claude-code", {
        "claude-code#b": true,
      }),
    ).toBe("a");
    expect(firstSignedInAccount(h.accounts, "claude-code", signedOut)).toBe("home");
    // A stored choice that is signed out falls to the best other login.
    expect(newThreadAccountOn(h, "work", signedOut)).toBe("home");
    // With only a signed-out login left it is still better than nothing.
    expect(firstSignedInAccount([account("a")], "claude-code", { "claude-code#a": true })).toBe("a");
  });

  test("does not force a stored bad login on a new thread", () => {
    const h = claude([{ ...account("work"), auth: "needs_login" }, account("home", "unknown")]);
    // `needs_login` is last, so the not-looked-at login carries the new thread.
    expect(newThreadAccountOn(h, "work")).toBe("home");
    expect(newThreadAccountOn(h, "home")).toBe("home");
  });
});

describe("a thread started by another thread", () => {
  test("inherits the parent's account on the same harness", () => {
    const h = claude([account("work"), account("home")]);
    expect(childAccountOn(h, { harness: "claude-code", account: "home" })).toBe("home");
  });

  test("moves off the parent's own login once named accounts exist", () => {
    const h = claude([account("work")]);
    expect(childAccountOn(h, { harness: "claude-code", account: null })).toBe("work");
  });

  test("does not inherit a login the plan read found signed out", () => {
    const h = claude([account("work"), account("home")]);
    expect(childAccountOn(h, { harness: "claude-code", account: "work" }, { "claude-code#work": true })).toBe(
      "home",
    );
  });

  test("never lends an account to another harness", () => {
    // Another harness's account is not one of this harness's; without named
    // accounts here, its own login stands.
    const other = claude([account("work")]);
    expect(childAccountOn(other, { harness: "codex", account: "home" })).toBe("work");
    expect(childAccountOn(claude(), { harness: "codex", account: "home" })).toBeNull();
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
    expect(childAccountFor(h, "  ", { harness: "codex", account: "home" }, {})).toEqual({
      account: "work",
    });
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
    // Another login is unaffected, and the harness's own sign-in is not an account.
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
  });

  test("keeps the signed-out default login when it is the only Claude login", () => {
    const rows = usageRows([plan({ status: "signedOut" })]);
    expect(rows.plans.map((p) => p.id)).toEqual(["claude"]);
  });

  test("drops it even when it is signed in, once named accounts are there", () => {
    const rows = usageRows([plan({ status: "ok" }), plan({ id: "claude:work", account: "work" })]);
    expect(rows.plans.map((p) => p.id)).toEqual(["claude:work"]);
  });

  test("drops it whether or not it was read, while named accounts are there", () => {
    const rows = usageRows([plan({ status: "notAsked" }), plan({ id: "claude:work", account: "work" })]);
    expect(rows.plans.map((p) => p.id)).toEqual(["claude:work"]);
  });

  test("keeps the own-login row while the thread on screen runs on it", () => {
    const rows = usageRows([plan({ status: "ok" }), plan({ id: "claude:work", account: "work" })], true);
    expect(rows.plans.map((p) => p.id)).toEqual(["claude", "claude:work"]);
  });

  test("draws only the accounts an answer read", () => {
    // One account at a time: the page's rows are what came back.
    const rows = usageRows([plan({ id: "claude:work", account: "work" })]);
    expect(rows.plans.map((p) => p.id)).toEqual(["claude:work"]);
    // A row the answer skipped is not reported as signed out, so nothing is dropped.
    expect(rows.plans).toHaveLength(1);
  });
});

describe("adding a discovered login", () => {
  const snapshot = (accounts: HarnessAccount[]): HarnessSnapshot => ({
    harnesses: [claude(accounts)],
    default: { harness: "rustcode", model: null, effort: null },
    onboarded: true,
  });

  test("signs in only when the check found no sign-in", () => {
    const dir = "~/.claude-personal";
    expect(
      accountToSignIn(
        snapshot([{ ...account("personal"), config_dir: dir, auth: "needs_login" }]),
        "claude-code",
        dir,
      )?.id,
    ).toBe("personal");
    // `accounts/add` answers `unknown`; only a real look says `needs_login`.
    expect(
      accountToSignIn(
        snapshot([{ ...account("personal"), config_dir: dir, auth: "unknown" }]),
        "claude-code",
        dir,
      ),
    ).toBeUndefined();
    expect(
      accountToSignIn(
        snapshot([{ ...account("personal"), config_dir: dir, auth: "ok" }]),
        "claude-code",
        dir,
      ),
    ).toBeUndefined();
    // A failed look, or a different directory, signs in nothing.
    expect(accountToSignIn(null, "claude-code", dir)).toBeUndefined();
    expect(accountToSignIn(snapshot([]), "claude-code", dir)).toBeUndefined();
  });
});
