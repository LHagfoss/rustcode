// What Settings › Usage draws, and whether opening it reads again. The whole
// answer is kept in the page cache (`state/pageCache.ts`); this decides what of
// it to show, with Claude's numbers dropped the moment consent is off.

import { planKey, planOf, type UsagePlan } from "../acp/rustcode";
import { type PageAnswer, pageIsFresh } from "./pageCache";

/** The cache key of the page's own answer: every subscription, every login. */
export const USAGE_PLANS_KEY = "settings/usage/plans";

/** The page's last answer: the plans it drew, and whether it asked for Claude. */
export interface UsageAnswer {
  plans: UsagePlan[];
  /** Whether the read asked for Claude's logins, so it is fresh for that switch only. */
  asked: boolean;
}

export interface UsageView {
  /** The plans to draw now, or null when there is nothing yet (the skeleton). */
  plans: UsagePlan[] | null;
  /** When the drawn numbers were read. */
  readAt: number | null;
  /** Whether opening the page should read now. */
  fetch: boolean;
  /** The error page: a failure with nothing to draw. */
  fatal: string | null;
  /** A quiet failure in the foot: the last numbers stay. */
  note: string | null;
}

/**
 * What the page shows and whether it reads. A last answer is shown at once,
 * with Claude's numbers dropped when consent is off; a read is skipped only when
 * it was made for the same consent and is under 30 seconds old, so flipping
 * between pages costs nothing and cannot raise a Keychain prompt.
 * A failed read keeps the shown numbers in the foot's note, and is the page
 * only when there is nothing else to show.
 */
export function usageView(
  answer: PageAnswer<UsageAnswer> | null,
  allowed: boolean,
  error: string | null,
  now = Date.now(),
): UsageView {
  const plans = answer ? (allowed ? answer.data.plans : withoutClaudeData(answer.data.plans)) : null;
  return {
    plans,
    readAt: answer?.readAt ?? null,
    fetch: !answer || answer.data.asked !== allowed || !pageIsFresh(answer, now),
    fatal: plans === null ? error : null,
    note: plans === null ? null : error,
  };
}

/**
 * Claude's rows without their numbers once consent is off, with the one row
 * that carries the switch kept (as `notAsked`, which draws no plan windows):
 * the switch to turn the meter back on must stay reachable. Named accounts
 * add no other row here.
 */
function withoutClaudeData(plans: UsagePlan[]): UsagePlan[] {
  const claude = plans.find((p) => planOf(p) === "claude-code");
  if (!claude) return plans;
  return plans.flatMap((p) => {
    if (planOf(p) !== "claude-code") return [p];
    if (p !== claude) return [];
    return [{ ...claude, status: "notAsked" as const, plan: undefined, error: undefined, windows: [] }];
  });
}

/**
 * The list the page keeps after a read. Its own whole read (`claude` and every
 * account) replaces the list. A partial answer — consent off, or a background
 * read of one account — says nothing about the rows it did not ask (`notAsked`
 * or absent), so they stay as shown instead of being wiped.
 */
export function answeredPlans(shown: UsagePlan[], answer: UsagePlan[], full: boolean): UsagePlan[] {
  if (full) return answer;
  const byKey = new Map(answer.map((p) => [planKey(p), p]));
  const known = new Set(shown.map(planKey));
  const next = shown.map((p) => {
    const asked = byKey.get(planKey(p));
    return asked && asked.status !== "notAsked" ? asked : p;
  });
  for (const asked of answer) {
    if (known.has(planKey(asked))) continue;
    next.push(asked);
  }
  return next;
}
