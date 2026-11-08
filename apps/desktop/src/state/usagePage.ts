// What Settings › Usage draws, whether opening it reads again, and its own
// refresh loop. The whole answer is kept in the page cache
// (`state/pageCache.ts`); this decides what of it to show, with Claude's
// numbers dropped the moment consent is off, and when it reads on its own.

import { planKey, planOf, type UsagePlan } from "../acp/rustcode";
import { type PageAnswer, pageIsFresh } from "./pageCache";

/** The cache key of the page's own answer: every subscription, every login. */
export const USAGE_PLANS_KEY = "settings/usage/plans";

/** The page's last answer: the plans it drew, and whether it was the whole look. */
export interface UsageAnswer {
  plans: UsagePlan[];
  /**
   * Whether the read asked for every Claude login, which is the page's explicit
   * look (opening it, Refresh) with consent on. A background read answers part
   * of that, so the next open reads the logins again, and a read made with the
   * switch off is not fresh for it either.
   */
  asked: boolean;
}

export interface UsageView {
  /** The plans to draw now, or null when there is nothing yet (the skeleton). */
  plans: UsagePlan[] | null;
  /** When the drawn numbers were read. */
  readAt: number | null;
  /** Whether opening the page should read now. */
  fetch: boolean;
  /** Whether that read must bypass the engine's cache, because a retry can fix it. */
  force: boolean;
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
 * While a Claude login's sign-in expired or was refused, the page reads again
 * even when that answer is fresh, and forces the read past the engine's minute
 * of cache: the user may have just renewed it in Claude Code.
 */
export function usageView(
  answer: PageAnswer<UsageAnswer> | null,
  allowed: boolean,
  error: string | null,
  now = Date.now(),
): UsageView {
  const plans = answer ? (allowed ? answer.data.plans : withoutClaudeData(answer.data.plans)) : null;
  const force = plans?.some(retryable) ?? false;
  return {
    plans,
    readAt: answer?.readAt ?? null,
    fetch: force || !answer || answer.data.asked !== allowed || !pageIsFresh(answer, now),
    force,
    fatal: plans === null ? error : null,
    note: plans === null ? null : error,
  };
}

/**
 * A failed Claude read that a retry can fix: Claude Code renews a lapsed access
 * token when it runs, and the user signs in again after a refusal. Only a row
 * with nothing recorded behind it gets here; numbers RustCode recorded for the
 * login are drawn "as of" instead. A denied Keychain prompt is left to an
 * explicit Refresh (asking again would prompt again), a forbidden sign-in to
 * the user (renewing does not change it), and a transient failure to the next
 * poll.
 */
function retryable(plan: UsagePlan): boolean {
  return (
    planOf(plan) === "claude-code" &&
    plan.status === "unavailable" &&
    (plan.reason === "expired" || plan.reason === "refused")
  );
}

/**
 * Whether Settings › Usage reads on its own now. Its interval (`everyMs` in
 * [`watchUsageRefresh`], the app's one plan-poll length) reads on every tick
 * while the window is visible, unless the app's plan cache was read within
 * that interval: the app's own poll (`state/store.ts` `startPlanPolling`)
 * reads the same logins, so reading them again here would read one Keychain
 * item twice per cycle. Coming back to the window reads only when what is drawn
 * is older than that interval, so switching away and back costs nothing. A read
 * already in flight is never doubled.
 */
export function usageAutoRead(input: {
  /** When the drawn numbers were read, or null with nothing drawn yet. */
  readAt: number | null;
  now?: number;
  /** `document.visibilityState === "visible"`. */
  visible: boolean;
  /** A read is in flight. */
  reading: boolean;
  /** Nothing in the plan cache wants a read (`!planCacheStale` in `state/store.ts`). */
  cacheFresh: boolean;
  /** The interval fired, or the window came back. */
  trigger: "tick" | "resumed";
  /** The interval's length; the app's `PLAN_CACHE_POLL_MS`. */
  everyMs: number;
}): boolean {
  const { readAt, now = Date.now(), visible, reading, cacheFresh, trigger, everyMs } = input;
  if (!visible || reading) return false;
  if (trigger === "tick") return !cacheFresh;
  return readAt === null || now - readAt >= everyMs;
}

/** The page's longest wait after failed reads: fifteen minutes. */
export const PLAN_REFRESH_MAX_BACKOFF_MS = 900_000;

/**
 * How long the page waits before reading again: its interval, doubled for each
 * read that failed in a row, up to fifteen minutes, and back to the interval as
 * soon as one answers. A page whose engine went away then asks every fifteen
 * minutes instead of every three, and shows the numbers at once when it comes
 * back.
 */
export function refreshBackoff(everyMs: number, failures: number): number {
  if (failures <= 0) return everyMs;
  return Math.min(everyMs * 2 ** failures, PLAN_REFRESH_MAX_BACKOFF_MS);
}

/**
 * Wires the page's own refresh: a timer that reads while the window is visible,
 * a read when the window comes back after being hidden longer than the
 * interval, and a longer wait after a read that failed ([`refreshBackoff`]).
 * Returns the stop function. A tick of a hidden window does nothing, so coming
 * back is caught by the staleness check rather than by the timer restarting.
 */
export function watchUsageRefresh(loop: {
  /**
   * Reads now; true when it answered, false when it failed, which backs the
   * timer off.
   */
  read: () => Promise<boolean> | boolean;
  /** Whether a read is in flight. */
  reading: () => boolean;
  /** `document.visibilityState === "visible"`. */
  visible: () => boolean;
  /** Nothing in the plan cache wants a read (`!planCacheStale` in `state/store.ts`). */
  cacheFresh: () => boolean;
  /** When the drawn numbers were read, or null with nothing drawn yet. */
  readAt: () => number | null;
  /** The interval's length in milliseconds (state/store.ts `PLAN_CACHE_POLL_MS`). */
  everyMs: number;
  /** Where `visibilitychange` is heard; the document, unless a test wires its own. */
  onChange?: (handler: () => void) => () => void;
}): () => void {
  let failures = 0;
  let timer: ReturnType<typeof setTimeout> | null = null;
  let stopped = false;
  const arm = () => {
    if (stopped) return;
    if (timer) clearTimeout(timer);
    timer = setTimeout(() => void run("tick"), refreshBackoff(loop.everyMs, failures));
  };
  const run = async (trigger: "tick" | "resumed") => {
    const read = usageAutoRead({
      readAt: loop.readAt(),
      visible: loop.visible(),
      reading: loop.reading(),
      cacheFresh: loop.cacheFresh(),
      trigger,
      everyMs: loop.everyMs,
    });
    if (!read) {
      arm();
      return;
    }
    let ok = false;
    try {
      ok = await loop.read();
    } catch {
      ok = false;
    }
    failures = ok ? 0 : failures + 1;
    arm();
  };
  arm();
  const off = (loop.onChange ?? listenOnDocument)(() => void run("resumed"));
  return () => {
    stopped = true;
    if (timer) clearTimeout(timer);
    timer = null;
    off();
  };
}

/** The default `visibilitychange` source: the page's document, when there is one. */
function listenOnDocument(handler: () => void): () => void {
  if (typeof document === "undefined") return () => {};
  document.addEventListener("visibilitychange", handler);
  return () => document.removeEventListener("visibilitychange", handler);
}

/**
 * The words for a plan row that has no numbers: the reason first, then its error.
 */
export function failureText(plan: UsagePlan): { title: string; subtitle?: string } {
  switch (plan.reason) {
    case "expired":
      // The access token Claude Code keeps on disk lives for hours and is
      // renewed only when Claude Code runs; the sign-in itself is still good.
      // Nothing here refreshes it, so a run on that login is what renews it.
      return {
        title: "Claude Code needs to refresh this sign-in",
        subtitle: plan.account
          ? "Run a Claude Code thread on this account to refresh its usage."
          : "Run a Claude Code thread to refresh its usage.",
      };
    case "refused":
      return {
        title: "Claude Code did not accept the sign-in",
        subtitle: "Sign in again in Claude Code, then press Refresh.",
      };
    case "forbidden":
      return {
        title: "Anthropic does not allow reading this plan's usage",
        subtitle: "Signing in again does not change that. Press Refresh to try once more.",
      };
    case "denied":
      return {
        title: "Could not read the sign-in",
        subtitle: "The system did not allow it. Press Refresh to be asked again.",
      };
    case "transient":
      return {
        title: "Could not read the usage",
        subtitle: "The next read will try again.",
      };
    default:
      return { title: "Could not read the usage" };
  }
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
    return [
      {
        ...claude,
        status: "notAsked" as const,
        plan: undefined,
        error: undefined,
        reason: undefined,
        windows: [],
      },
    ];
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

/**
 * What a press on the composer's usage meter opens: Settings' Usage page, and a press while that
 * page is already open closes it, the way a press on its trigger does elsewhere. The meter's hover
 * card is the glance; the page is where the press goes (a press that toggled the card left the
 * page reachable only from the card's own link).
 */
export function usagePressTarget(usageOpen: boolean): "usage" | null {
  return usageOpen ? null : "usage";
}
