// What a plans read asks for, and which logins it stops asking about.
//
// `_rustcode/usage/plans` reads Claude Code's sign-in, which on macOS is in the
// Keychain: macOS may ask before an unseen item is read, so the app reads one
// login at a time in the background — the default login, and the account of
// the thread on screen. A login whose read was denied is left alone until the
// user looks explicitly (Settings › Usage, Refresh, Check in Settings › Harnesses),
// since asking again would only prompt again; so is one Anthropic forbids
// (403), since renewing the sign-in does not change that. A login whose sign-in
// expired, or which Anthropic refused (401), is *not* paused: the user renews
// it in Claude Code, and the next poll has to pick that up on its own. A
// signed-out login is not asked for again in the background either: the user
// signs in again in Claude Code, and Settings › Usage re-reads it. The rules are
// here, away from the store, so they are tested without an engine, a Keychain
// or a React tree.

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
  /**
   * Read the logins again instead of answering from the engine's minute of
   * cache. An explicit look sets it, so that a sign-in renewed in Claude Code
   * is seen at once; a background poll leaves it off, or every poll would read
   * the Keychain.
   */
  force?: boolean;
}

/** What the app knows when it decides what to ask for. */
export interface PlansAskInput {
  /** Whether a first Claude read has been requested in this app session. */
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
 * What a refresh asks for: the default login after the first request and when the
 * last read did not stop it, and the account of the thread on screen when that
 * one was not stopped either. Only a Claude Code thread has such an account; a
 * paused default login does not keep a named account away, or the reverse.
 */
export function plansAskFor({ allowed, harness, account, pauses, explicit }: PlansAskInput): PlansAsk {
  if (!allowed) return { claude: false };
  const claude = explicit || !pauses.claude;
  const ask: PlansAsk = explicit ? { claude, force: true } : { claude };
  if (harness !== "claude-code" || !account) return ask;
  if (!explicit && pauses.accounts.has(account)) return ask;
  return { ...ask, account };
}

/**
 * The pauses an answer adds. A login macOS would not hand over (a denied
 * prompt, a locked Keychain) is left alone, so a Deny cannot prompt every three
 * minutes, and so is one Anthropic forbids (renewing does not fix a 403); a
 * signed-out login is not read again in the background either. An expired or
 * refused sign-in is retried at the next poll: opening Claude Code renews it,
 * and the app must pick that up without a visit to Settings › Usage. An engine
 * that names no reason keeps the old rule (pause) for every failed read, so a
 * Deny can never become a prompt loop against an older engine. An answer that
 * did not read a login says nothing about it, so what an earlier read found
 * stays; only an explicit look clears it.
 */
export function pausesAfter(answers: UsagePlan[], before: PlanPauses): PlanPauses {
  let claude = before.claude;
  let accounts = before.accounts;
  for (const plan of answers) {
    if (planOf(plan) !== "claude-code") continue;
    const failed =
      plan.status === "signedOut" || (plan.status === "unavailable" && failurePauses(plan.reason));
    if (!failed) continue;
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

/**
 * Whether a failed read is one the background must not repeat on its own: a
 * denied Keychain prompt (asking would prompt again) and a forbidden sign-in
 * (renewing will not change it). A missing reason is an engine that predates
 * the reasons, which always paused.
 */
function failurePauses(reason: UsagePlan["reason"]): boolean {
  return reason === undefined || reason === "denied" || reason === "forbidden";
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
  const force = before.force || ask.force;
  const forcedNow = !!force && !before.force;
  if (claude === before.claude && account === before.account && !forcedNow) return queued;
  const next = [...queued];
  const merged: PlansAsk = { claude };
  if (account) merged.account = account;
  // An explicit look folded into a background one still forces the read.
  if (force) merged.force = true;
  next[at] = merged;
  return next;
}
