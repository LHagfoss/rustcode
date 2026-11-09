// Which logins a thread runs on, and which the app offers.
//
// With named accounts, the harness's own login is never offered: a thread runs
// on a named account or, for a thread that already started on the harness's
// own sign-in before accounts existed, on that sign-in ("Original login").
// The engine's harness `auth` cannot say for Claude Code: it has no cheap
// sign-in command (`auth_probe: None` in `rustcode/engine/src/harness/mod.rs`),
// so its `auth` is always `unknown`, signed in or not. That is why a named
// account, once one exists, is always the choice for a new thread.

import { harnessPlanKey, planKey, planOf, type UsagePlan } from "../acp/rustcode";
import type { Harness, HarnessAccount, HarnessSnapshot } from "../acp/types";

/**
 * The logins the last plan read found signed out, by cache key: the harness
 * for its default login, the harness and the account id for a named one
 * (`claude-code`, `claude-code#work`).
 */
export type SignedOutLogins = Record<string, boolean>;

/**
 * The record a plans answer leaves. A login it read as signed out is marked;
 * one it could read (`ok`, or signed in but unreadable) is cleared. `notAsked`
 * (the meter is off, or the answer was for something else) says nothing, and
 * keeps what an earlier answer found.
 */
export function signedOutFrom(plans: UsagePlan[], before: SignedOutLogins = {}): SignedOutLogins {
  const next = { ...before };
  for (const plan of plans) {
    if (plan.status === "notAsked") continue;
    const key = planKey(plan);
    if (plan.status === "signedOut") next[key] = true;
    else delete next[key];
  }
  return next;
}

/**
 * How good a login is for a thread that must pick one: a sign-in found first,
 * then one not looked at yet (usually signed in), then one known to be signed
 * out, whether its own `auth` says so or the plan read did.
 */
function accountRank(
  account: HarnessAccount,
  harness: string | undefined,
  signedOut: SignedOutLogins,
): number {
  if (harness && accountSignedOut(harness, account.id, signedOut)) return 2;
  if (account.auth === "ok") return 0;
  if (account.auth === "unknown") return 1;
  return 2;
}

/**
 * The named account to run on, best first: `ok`, then `unknown` (only the
 * macOS Keychain could tell), then a login known to be signed out. A known
 * signed-out login is the last resort, never the first pick: the harness's own
 * sign-in is no longer offered once there is a choice. The list's order
 * decides between equals.
 */
export function firstSignedInAccount(
  accounts: HarnessAccount[] | undefined,
  harness?: string,
  signedOut: SignedOutLogins = {},
): string | null {
  const list = accounts ?? [];
  let best: HarnessAccount | undefined;
  let bestRank = Number.POSITIVE_INFINITY;
  for (const account of list) {
    const rank = accountRank(account, harness, signedOut);
    if (rank < bestRank) {
      best = account;
      bestRank = rank;
    }
  }
  return best?.id ?? null;
}

/**
 * The account a new thread on `harness` starts on: the chosen account while it
 * can run (signed in, or not looked at), else no account at all when the
 * harness has none (it runs on its own sign-in and no account UI is shown),
 * else the best named account. `null` is never chosen while named accounts
 * exist, and a login known to be signed out is not chosen over a better one.
 */
export function newThreadAccountOn(
  harness: Harness | undefined,
  chosen: string | null,
  signedOut: SignedOutLogins = {},
): string | null {
  const accounts = harness?.accounts ?? [];
  const found = accounts.find((account) => account.id === chosen);
  if (found && accountRank(found, harness?.id, signedOut) < 2) return found.id;
  return firstSignedInAccount(accounts, harness?.id, signedOut);
}

/**
 * The account a thread started by another thread (`start_session`, `fan_out`)
 * runs on: the parent's own account when the child runs on the same harness,
 * else what a new thread would start on.
 */
export function childAccountOn(
  harness: Harness | undefined,
  parent: { harness?: string; account?: string | null } | undefined,
  signedOut: SignedOutLogins = {},
): string | null {
  const inherited = parent && parent.harness === harness?.id ? (parent.account ?? null) : null;
  return newThreadAccountOn(harness, inherited, signedOut);
}

/** What an account id or display name picked, or why it picked nothing. */
export type NamedAccountMatch =
  | { kind: "one"; account: HarnessAccount }
  | { kind: "ambiguous"; ids: string[] }
  | { kind: "none" };

/**
 * The named login of `harness` an id or a display name picks. An exact id is
 * the most precise name, so it wins even when another login's display name is
 * the same, and stays reachable. Failing that, a case-insensitive id match is
 * used, then a display name; more than one of either is ambiguous, never a
 * guess.
 */
export function namedAccountOn(harness: Harness | undefined, asked: string): NamedAccountMatch {
  const wanted = asked.trim();
  if (!wanted) return { kind: "none" };
  const accounts = harness?.accounts ?? [];
  const exact = accounts.filter((account) => account.id === wanted);
  if (exact.length === 1) return { kind: "one", account: exact[0]! };
  if (exact.length > 1) return { kind: "ambiguous", ids: exact.map((account) => account.id) };
  const lower = wanted.toLowerCase();
  const byId = accounts.filter((account) => account.id.toLowerCase() === lower);
  if (byId.length === 1) return { kind: "one", account: byId[0]! };
  if (byId.length > 1) return { kind: "ambiguous", ids: byId.map((account) => account.id) };
  const byName = accounts.filter((account) => account.name.toLowerCase() === lower);
  if (byName.length === 1) return { kind: "one", account: byName[0]! };
  if (byName.length > 1) return { kind: "ambiguous", ids: byName.map((account) => account.id) };
  return { kind: "none" };
}

/** A login's display name, or its id when the harness no longer lists it. */
export function accountNameOf(
  harness: Harness | undefined,
  id: string | null | undefined,
): string | undefined {
  if (!id) return undefined;
  return harness?.accounts?.find((account) => account.id === id)?.name ?? id;
}

/** A login as an approval card names it: both its display name and its id. */
export function accountLabelOf(
  harness: Harness | undefined,
  id: string | null | undefined,
): string | undefined {
  if (!id) return undefined;
  const name = harness?.accounts?.find((account) => account.id === id)?.name;
  return name && name !== id ? `${name} (${id})` : (name ?? id);
}

/** Whether the plan read found this named login signed out (`harness#account`). */
export const accountSignedOut = (harness: string, account: string, signedOut: SignedOutLogins): boolean =>
  !!signedOut[harnessPlanKey(harness, account)];

/** Why a delegated thread's named login cannot be used. */
export type ChildAccountRefusal =
  | { kind: "unknown"; asked: string; available: string[] }
  | { kind: "ambiguous"; asked: string; matches: string[] }
  | { kind: "signedOut"; account: string; name: string; available: string[] };

/**
 * The account a delegated thread runs on: the named one it was told to use,
 * checked against that harness's accounts, else the inherited default. An
 * unknown, ambiguous or signed-out login is refused, so the child never
 * quietly lands on another login than the one the agent named.
 */
export type ChildAccount = { account: string | null } | { refused: ChildAccountRefusal };

export function childAccountFor(
  harness: Harness | undefined,
  asked: string | undefined,
  parent: { harness?: string; account?: string | null } | undefined,
  signedOut: SignedOutLogins,
): ChildAccount {
  if (!asked?.trim()) return { account: childAccountOn(harness, parent, signedOut) };
  const available = (harness?.accounts ?? []).map((account) => account.id);
  const match = namedAccountOn(harness, asked);
  if (match.kind === "none") {
    return { refused: { kind: "unknown", asked: asked.trim(), available } };
  }
  if (match.kind === "ambiguous") {
    return { refused: { kind: "ambiguous", asked: asked.trim(), matches: match.ids } };
  }
  const account = match.account;
  // A login the plan read found signed out cannot run a turn, so naming it is refused now
  // rather than starting a thread that fails on its first message.
  if (harness && accountSignedOut(harness.id, account.id, signedOut)) {
    return {
      refused: { kind: "signedOut", account: account.id, name: account.name, available },
    };
  }
  return { account: account.id };
}

/** Settings › Usage's visible account rows. */
export interface UsageRows {
  /** The groups to draw, in order. */
  plans: UsagePlan[];
}

/**
 * The groups Settings › Usage draws. The engine always lists the Claude login
 * with no account (`rustcode/engine/src/acp/usage.rs`); with named accounts the
 * app and the engine read those instead, so its row is dropped.
 */
export function usageRows(plans: UsagePlan[], keepOwnLogin = false): UsageRows {
  const claude = (plan: UsagePlan) => planOf(plan) === "claude-code";
  const isDefault = (plan: UsagePlan) => claude(plan) && !plan.account;
  const named = plans.some((plan) => claude(plan) && !!plan.account);
  // The thread on screen may still run on the harness's own sign-in ("Original
  // login"): its row stays so its plan is not lost.
  const drop = named && !keepOwnLogin;
  const shown = plans.filter((plan) => !(drop && isDefault(plan)));
  return { plans: shown };
}

/**
 * The account an add still has to sign in, from the check that followed it:
 * only a look that found no sign-in asks for one. `accounts/add` answers
 * without probing, so its row alone reads `unknown`.
 */
export function accountToSignIn(
  snapshot: HarnessSnapshot | null,
  harness: string,
  configDir: string,
): HarnessAccount | undefined {
  const found = snapshot?.harnesses
    .find((h) => h.id === harness)
    ?.accounts?.find((account) => account.config_dir === configDir);
  return found?.auth === "needs_login" ? found : undefined;
}
