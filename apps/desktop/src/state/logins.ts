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
