// Which logins a thread may run on, and which the app offers.
//
// The engine's harness `auth` cannot say for Claude Code: it has no cheap
// sign-in command (`auth_probe: None` in `rustcode/engine/src/harness/mod.rs`),
// so its `auth` is always `unknown`, signed in or not. The plan read
// (`_rustcode/usage/plans`) is what reports a login as signed out: the default
// login and the account of the thread on screen every few minutes while the
// composer's meter is on, and each named account's own row when Settings › Usage
// is shown or refreshed. An answer often holds one login's row only, so a row
// that is absent says nothing about that login.

import { harnessPlanKey, planKey, planOf, type UsagePlan } from "../acp/rustcode";
import type { Harness, HarnessAccount } from "../acp/types";

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
 * Whether a harness's default login is signed out while named accounts can
 * carry threads: the one case where it is offered nowhere. With no named
 * account, the default login is all there is and stays as it is today.
 */
export function defaultLoginGone(harness: Harness | undefined, signedOut: SignedOutLogins): boolean {
  return !!harness && (harness.accounts?.length ?? 0) > 0 && !!signedOut[harnessPlanKey(harness.id)];
}

/**
 * The named account to run on when the default login cannot. A login whose
 * credentials are only in the macOS Keychain reads `unknown` until Settings ›
 * Agents probes it, so `unknown` is not "signed out": when none reports `ok`,
 * the first named account stands in rather than a default login known to fail.
 */
export function firstSignedInAccount(accounts: HarnessAccount[] | undefined): string | null {
  const list = accounts ?? [];
  return (list.find((account) => account.auth === "ok") ?? list[0])?.id ?? null;
}

/**
 * The account a new thread on `harness` starts on when nobody chose one: the
 * chosen account while it is still there, else the first signed-in named one
 * once the default login is known to be signed out, else the default login.
 */
export function newThreadAccountOn(
  harness: Harness | undefined,
  chosen: string | null,
  signedOut: SignedOutLogins,
): string | null {
  const accounts = harness?.accounts ?? [];
  if (chosen && accounts.some((account) => account.id === chosen)) return chosen;
  if (!defaultLoginGone(harness, signedOut)) return null;
  return firstSignedInAccount(accounts);
}

/**
 * The account a thread started by another thread (`start_session`, `fan_out`)
 * runs on: the parent's own account when the child runs on the same harness,
 * else what a new thread would start on. Without this, a delegated Claude
 * thread lands on a default login that is signed out and cannot run.
 */
export function childAccountOn(
  harness: Harness | undefined,
  parent: { harness?: string; account?: string | null } | undefined,
  signedOut: SignedOutLogins,
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

/** Settings › Usage's groups, and which one carries the composer's switch. */
export interface UsageRows {
  /** The groups to draw, in order. */
  plans: UsagePlan[];
  /** The id of the group whose row holds "Show Claude usage in the composer"; none without a Claude group. */
  switchIn?: string;
}

/**
 * The groups Settings › Usage draws. The engine always lists the default
 * Claude login (`rustcode/engine/src/acp/usage.rs`), signed out or not; with
 * named accounts there, a signed-out "Claude · Not signed in" group is one the
 * user can do nothing with, so it is dropped. The switch that lived in its
 * last row moves to the first Claude group left, once only.
 */
export function usageRows(plans: UsagePlan[]): UsageRows {
  const claude = (plan: UsagePlan) => planOf(plan) === "claude-code";
  const isDefault = (plan: UsagePlan) => claude(plan) && !plan.account;
  const named = plans.some((plan) => claude(plan) && !!plan.account);
  const drop = (plan: UsagePlan) => named && isDefault(plan) && plan.status === "signedOut";
  const shown = plans.filter((plan) => !drop(plan));
  const own = shown.find(isDefault);
  return { plans: shown, switchIn: (own ?? shown.find(claude))?.id };
}
