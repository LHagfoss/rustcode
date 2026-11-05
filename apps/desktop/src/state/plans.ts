// What a plans read asks for, and which logins it stops asking about.
//
// `_rustcode/usage/plans` reads Claude Code's sign-in, which on macOS is in the
// Keychain: macOS may ask before an unseen item is read, so the app reads one
// login at a time in the background — the default login, and the account of
// the thread on screen. A login whose read was denied (or which is signed out)
// is left alone until the user looks explicitly (Settings › Usage, Refresh,
// Check in Settings › Agents). The rules are here, away from the store, so they
// are tested without an engine, a Keychain or a React tree.

import { planOf, type UsagePlan } from "../acp/rustcode";

/**
 * The Claude logins the background read leaves alone. The default login and a
 * named account are kept apart, so one being unreadable never stops the other.
 */
export interface PlanPauses {
  /** The default login (`claude`). */
  claude: boolean;
  /** Named accounts, by id. */
  accounts: ReadonlySet<string>;
}

export const NO_PAUSES: PlanPauses = { claude: false, accounts: new Set() };

/** One `_rustcode/usage/plans` call: the Claude logins it reads. */
export interface PlansAsk {
  /** The default login. */
  claude: boolean;
  /** One named account's own sign-in. */
  account?: string;
}

/** What the app knows when it decides what to ask for. */
export interface PlansAskInput {
  /** Whether the user let Claude's sign-in be read at all (Settings › Usage's switch). */
  allowed: boolean;
  /** The harness of the thread on screen, or of the one that will start. */
  harness: string | undefined;
  /** The named account that thread runs on, if it runs on one. */
  account?: string | null;
  pauses: PlanPauses;
  /** The user asked to look now, so a pause does not hold it back. */
  explicit?: boolean;
}

/**
 * What a refresh asks for: the default login when consent allows it and the
 * last read did not stop it, and the account of the thread on screen when that
 * one was not stopped either. Only a Claude Code thread has such an account; a
 * paused default login does not keep a named account away, or the reverse.
 */
export function plansAskFor({ allowed, harness, account, pauses, explicit }: PlansAskInput): PlansAsk {
  if (!allowed) return { claude: false };
  const claude = explicit || !pauses.claude;
  if (harness !== "claude-code" || !account) return { claude };
  if (!explicit && pauses.accounts.has(account)) return { claude };
  return { claude, account };
}

/**
 * The pauses an answer adds: a Claude login whose read was denied, or which is
 * signed out, is not read again in the background, so a Deny cannot prompt
 * every three minutes. An answer that did not read a login says nothing about
 * it, so what an earlier read found stays; only an explicit look clears it.
 */
export function pausesAfter(answers: UsagePlan[], before: PlanPauses): PlanPauses {
  let claude = before.claude;
  let accounts = before.accounts;
  for (const plan of answers) {
    if (planOf(plan) !== "claude-code") continue;
    if (plan.status !== "unavailable" && plan.status !== "signedOut") continue;
    if (plan.account) {
      if (accounts.has(plan.account)) continue;
      accounts = new Set([...accounts, plan.account]);
    } else {
      claude = true;
    }
  }
  if (claude === before.claude && accounts === before.accounts) return before;
  return { claude, accounts };
}

/** Two asks can share a request: one account at most travels with the default login. */
const mergeable = (a: PlansAsk, b: PlansAsk) => !(a.account && b.account && a.account !== b.account);

/**
 * Adds an ask to what is waiting for the request in flight, coalesced so
 * nothing piles up: an ask a waiting request already carries changes nothing,
 * and the default login and an account fold into one call. Two different
 * accounts stay two calls, since a request reads one account at most.
 */
export function queueAsks(queued: PlansAsk[], ask: PlansAsk): PlansAsk[] {
  if (!ask.claude && !ask.account) return queued;
  const at = queued.findIndex((q) => mergeable(q, ask));
  if (at < 0) return [...queued, ask];
  const before = queued[at]!;
  const claude = before.claude || ask.claude;
  const account = before.account ?? ask.account;
  if (claude === before.claude && account === before.account) return queued;
  const next = [...queued];
  next[at] = account ? { claude, account } : { claude };
  return next;
}
