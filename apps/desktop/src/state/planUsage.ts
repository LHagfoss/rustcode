// Plan usage: the composer meter's numbers and the shared plan cache, the requests that fill them
// and the poll that keeps them fresh. The pure rules for what to ask live in `state/plans.ts`;
// this file owns the timers and the requests. Split out of state/store.ts; the state it writes
// (`planUsage`, `planCache`, `signedOutLogins`) still lives in the app store.

import {
  harnessPlanKey,
  nextPlanRefresh,
  type PlanUsage,
  parsePlans,
  parsePlanUsage,
  planHarness,
  planKey,
  type UsagePlan,
  type UsageWindow,
} from "../acp/rustcode";
import { Method } from "../acp/types";
import { signedOutFrom } from "./logins";
import { NO_PAUSES, type PlanPauses, type PlansAsk, pausesAfter, plansAskFor, queueAsks } from "./plans";
import {
  type AppState,
  accountOn,
  type CachedPlan,
  connectionFor,
  connectionForThread,
  harnessOf,
  PLAN_CACHE_MAX_AGE_MS,
  STORAGE,
  store,
  useApp,
} from "./store";
import { splitThreadKey } from "./threadKeys";

// Read through `useApp` at call time: store.ts imports this module, so `useApp` is not defined yet
// while this one loads.
const get = () => useApp.getState();
const set = (patch: Partial<AppState> | ((s: AppState) => Partial<AppState>)) => useApp.setState(patch);

const planUsageInFlight = new Set<string>();

/** How often the app asks for every plan while its window is visible. */
export const PLAN_CACHE_POLL_MS = 180_000;

/**
 * How much before the poll interval an entry counts as stale. A read stamps
 * `fetchedAt` when it finishes, a moment after the timer that asked for it
 * fired, so comparing against the whole interval would always miss the next
 * tick and read every other one — every six minutes instead of three.
 */
export const PLAN_STALE_MARGIN_MS = 15_000;

/** True when a harness's numbers are missing or old enough for the next tick to read them again. */
export function planIsStale(plan: CachedPlan | undefined, now = Date.now()): boolean {
  return !plan?.fetchedAt || now - plan.fetchedAt >= PLAN_CACHE_POLL_MS - PLAN_STALE_MARGIN_MS;
}

/** True when anything in the cache wants a refresh: an entry past the poll, or none at all. */
export function planCacheStale(cache: Record<string, CachedPlan>, now = Date.now()): boolean {
  const entries = Object.values(cache);
  return !entries.length || entries.some((p) => planIsStale(p, now));
}

/** The persisted cache without entries from more than a day ago. */
export function prunePlans(cache: Record<string, CachedPlan>, now = Date.now()): Record<string, CachedPlan> {
  const kept = Object.entries(cache).filter(([, p]) => now - (p.fetchedAt ?? 0) < PLAN_CACHE_MAX_AGE_MS);
  return kept.length === Object.keys(cache).length ? cache : Object.fromEntries(kept);
}

/** The cache without the harnesses a plans answer proved signed out or not installed. */
export function forgetPlans(
  cache: Record<string, CachedPlan>,
  harnesses: string[],
): Record<string, CachedPlan> {
  const gone = harnesses.filter((h) => cache[h]);
  if (!gone.length) return cache;
  const next = { ...cache };
  for (const h of gone) delete next[h];
  return next;
}

/**
 * A window whose reset passed since these numbers were read as reset: nothing
 * used, and the stale reset time dropped, until the next read says otherwise.
 */
export function windowNow(w: UsageWindow, now = Date.now()): UsageWindow {
  return w.resetsAt && w.resetsAt <= now ? { ...w, usedPercent: 0, resetsAt: undefined } : w;
}

/** The plan with every passed reset folded in; undefined when it has no windows. */
export function planNow(plan: PlanUsage | null | undefined, now = Date.now()): PlanUsage | undefined {
  if (!plan?.windows.length) return undefined;
  const windows = plan.windows.map((w) => windowNow(w, now));
  return windows.some((w, i) => w !== plan.windows[i]) ? { ...plan, windows } : plan;
}

/**
 * Folds fresh numbers into the cache, one entry per harness: the newer of the
 * two wins, so a slow plans answer never overwrites a thread's newer limits,
 * and an answer without windows (a harness with no plan) never erases one.
 */
export function mergePlans(
  cache: Record<string, CachedPlan>,
  incoming: Record<string, CachedPlan>,
): Record<string, CachedPlan> {
  let next = cache;
  for (const [harness, plan] of Object.entries(incoming)) {
    if (!plan.windows.length) continue;
    const known = next[harness];
    if (known && (known.fetchedAt ?? 0) >= (plan.fetchedAt ?? 0)) continue;
    if (next === cache) next = { ...cache };
    next[harness] = plan;
  }
  return next;
}

/** The plans answer as cache entries, for each signed-in harness that reported windows. */
export function plansToCache(plans: UsagePlan[], fetchedAt = Date.now()): Record<string, CachedPlan> {
  const out: Record<string, CachedPlan> = {};
  for (const p of plans) {
    if (p.status !== "ok" || !p.windows.length) continue;
    const harness = p.harness ?? planHarness(p.id);
    out[planKey(p)] = {
      source: harness,
      windows: p.windows.map((w) => ({
        usedPercent: w.usedPercent,
        minutes: w.minutes,
        resetsAt: w.resetsAt,
        label: w.label,
      })),
      fetchedAt,
      name: p.name,
      plan: p.plan,
      harness,
      account: p.account ?? null,
      ...(p.asOf !== undefined ? { asOf: p.asOf } : {}),
    };
  }
  return out;
}

/**
 * The one plan in the cache, for a thread whose harness the engine did not
 * report: with a single subscription signed in, it is the one in use. With
 * several, guessing would show another harness's numbers, so none is used.
 */
export function onlyCachedPlan(cache: Record<string, CachedPlan>): CachedPlan | undefined {
  const plans = Object.values(cache).filter((p) => p.windows.length);
  return plans.length === 1 ? plans[0] : undefined;
}

/**
 * The plan the composer's meter shows. A thread's own limits are for the account
 * it runs on, so they win whenever it has any; the harness's cache (marked
 * `cached`) fills in where the engine answers none, as it does for Claude Code.
 */
export function shownPlan(
  own: PlanUsage | null | undefined,
  cached: CachedPlan | undefined,
  now = Date.now(),
): { plan: PlanUsage | undefined; cached: boolean } {
  const live = planNow(own, now);
  if (live) return { plan: live, cached: false };
  const known = planNow(cached, now);
  return { plan: known, cached: !!known };
}

/** Writes a cache into the store and app storage; no-op when it is the same map. */
function setPlanCache(next: Record<string, CachedPlan>) {
  if (next === get().planCache) return;
  set({ planCache: next });
  store(STORAGE.planCache, JSON.stringify(next));
  schedulePlanReset(next);
}

/** Writes merged numbers into the store and app storage; no-op when nothing is newer. */
function commitPlans(incoming: Record<string, CachedPlan>) {
  setPlanCache(mergePlans(get().planCache, incoming));
}

/**
 * Asks the engine for the plan's usage windows. One request per thread at a
 * time; a failed one keeps the last good numbers (marked stale), and only an
 * answer without windows (the harness has no plan usage) clears them.
 */
export async function refreshPlanUsage(id: string): Promise<void> {
  const conn = connectionForThread(id);
  if (!conn || planUsageInFlight.has(id)) return;
  planUsageInFlight.add(id);
  try {
    const usage = parsePlanUsage(
      await conn.request(Method.usageLimits, { sessionId: splitThreadKey(id).id }, 10_000),
    );
    const fetchedAt = Date.now();
    set((s) => ({ planUsage: { ...s.planUsage, [id]: usage && { ...usage, fetchedAt } } }));
    // The same numbers serve every thread on that harness; a thread that has
    // none of its own (Claude Code's limits answer null) reads it from the cache.
    // A native thread's numbers belong to the provider it runs on, which the
    // harness id does not name (`rustcode`): they stay on the thread, so no
    // other native thread reads them as its own.
    const harness = harnessOf(get(), id);
    if (usage && harness && harness !== "rustcode") {
      // The account's own row, so it never overwrites another login's numbers.
      const key = harnessPlanKey(harness, accountOn(get(), id));
      commitPlans({
        [key]: { ...usage, fetchedAt, source: harness, harness },
      });
    }
  } catch {
    set((s) => {
      const last = s.planUsage[id];
      return last ? { planUsage: { ...s.planUsage, [id]: { ...last, stale: true } } } : s;
    });
  } finally {
    planUsageInFlight.delete(id);
  }
}

let plansInFlight: PlansAsk | null = null;
/** Asks that arrived while one was in flight: run after it, coalesced (state/plans.ts). */
let plansQueued: PlansAsk[] = [];
/**
 * The Claude logins the background read leaves alone: the default login, and
 * each account whose Keychain read was denied (or which is signed out), so a
 * Deny never prompts every three minutes. Settings' own look clears them. An
 * expired sign-in is not here: Claude Code renews it, and the background read
 * must notice that without the user opening Settings.
 */
let planPauses: PlanPauses = NO_PAUSES;

/** The harness whose plans come from named logins (`acp/rustcode.ts` `harnessPlanKey`). */
export const CLAUDE_CODE = "claude-code";

// Start Claude reads only after an explicit look or use of a Claude account.
let claudePlansStarted = false;

/** Use of a Claude account (opening, picking or switching to one): its plans may be read from now on. */
export function markClaudePlansStarted(): void {
  claudePlansStarted = true;
}

/**
 * Folds a plans answer into the cache: a signed-in harness's windows go in, one
 * that is signed out or gone loses its entry, and a Claude login that could not
 * be read stops the background poll asking for it again (a Deny in the Keychain
 * must not prompt every three minutes). An answer may hold only some rows, so
 * nothing it did not read is touched. `asked` is Settings' own call, which is
 * what unpauses them.
 */
export function commitPlansAnswer(answers: UsagePlan[], asked = false): void {
  if (asked) {
    claudePlansStarted = true;
    planPauses = NO_PAUSES;
  }
  const gone = answers.filter((p) => p.status === "signedOut" || p.status === "notInstalled").map(planKey);
  const kept = forgetPlans(get().planCache, gone);
  const learned = plansToCache(answers);
  setPlanCache(mergePlans(kept, learned));
  // Which logins are signed out, for the places that must not offer one: the
  // picker's account chips and a new thread's account (state/logins.ts).
  set({ signedOutLogins: signedOutFrom(answers, get().signedOutLogins) });
  // A login whose read was denied, or that is signed out, pauses its own row
  // only: the default login's failure must not stop a named account's meter,
  // and an account without a sign-in must not stop the default login's.
  planPauses = pausesAfter(answers, planPauses);
}

/**
 * What a refresh asks the engine for: the default login and the account of the
 * thread on screen, after first use and while it was not paused.
 */
function currentPlansAsk(explicit: boolean): PlansAsk {
  const s = get();
  const threadId = s.selectedId ?? undefined;
  if (explicit) claudePlansStarted = true;
  return plansAskFor({
    allowed: claudePlansStarted,
    harness: harnessOf(s, threadId),
    account: accountOn(s, threadId),
    pauses: planPauses,
    explicit,
  });
}

/**
 * Asks for every subscription's plan (`_rustcode/usage/plans`, no thread
 * needed) and folds the answer into the cache. Claude is asked for after first use: the default login, and the account of the thread on screen so its
 * meter has numbers without a visit to Settings › Usage, one Keychain item at a
 * time. One request at a time: anything asked for while one is in flight runs
 * after it, coalesced, so a hover during the poll is not dropped. A failed one
 * keeps the last numbers for the next poll. An explicit check (`again`) bypasses
 * the engine's minute-long cache, so a sign-in renewed in Claude Code shows at
 * once instead of after that minute.
 */
export async function refreshPlanCache(again = false): Promise<void> {
  if (!connectionFor("") || get().status !== "ready") return;
  const ask = currentPlansAsk(again);
  // An explicit check asks again even after an answer paused a login.
  if (again) planPauses = NO_PAUSES;
  if (plansInFlight) {
    plansQueued = queueAsks(plansQueued, ask);
    return;
  }
  let next: PlansAsk | undefined = ask;
  for (let conn = connectionFor(""); next && conn; conn = connectionFor("")) {
    plansInFlight = next;
    try {
      const result = await conn.request(
        Method.usagePlans,
        {
          claude: next.claude,
          ...(next.account ? { claudeAccount: next.account } : {}),
          // An explicit look reads the Keychain and Anthropic again rather
          // than answer from the engine's minute of cache (state/plans.ts).
          ...(next.force ? { force: true } : {}),
        },
        20_000,
      );
      commitPlansAnswer(parsePlans(result));
    } catch {
      // Cached numbers stay; the next poll tries again.
    } finally {
      plansInFlight = null;
    }
    next = plansQueued.shift();
  }
}

/**
 * What a background read asks for, for a caller that sends its own request
 * (Settings › Usage's own interval): the default login and the account of the
 * thread on screen, each only with consent and while the last read did not
 * pause it (`state/plans.ts`). Settings' explicit look asks for every login
 * instead, which is what `_rustcode/usage/plans`'s `claudeAccounts` does.
 */
export function backgroundPlansAsk(): PlansAsk {
  return currentPlansAsk(false);
}

/** When the named logins were last asked for, so that it happens once per poll. */
let planLoginsAskedAt = 0;

/**
 * Asks in the background for the numbers a caller that can only report the
 * cache needs newer ones of (`list_harnesses`): the app's own ask — the default
 * login and the account of the thread on screen (`currentPlansAsk`) — and every
 * named login that was read before and has gone stale, which the poll alone
 * never reads.
 *
 * A login nothing was read for is never asked for: reading an unseen Keychain
 * item can prompt macOS, and the user never asked for that. Neither is one a
 * failed read paused (`failurePauses` in `state/plans.ts`). All of it happens
 * at most once per poll interval however often it is called, and the requests
 * queue behind the one in flight, so one Keychain item is read at a time. The
 * caller's answer stays cache-only: this never awaits.
 */
export function refreshReadPlanLogins(now = Date.now()): void {
  if (!connectionFor("") || get().status !== "ready") return;
  if (now - planLoginsAskedAt < PLAN_CACHE_POLL_MS) return;
  planLoginsAskedAt = now;
  const cache = get().planCache;
  const prefix = `${CLAUDE_CODE}#`;
  const stale = Object.keys(cache)
    .filter((key) => key.startsWith(prefix))
    .map((key) => key.slice(prefix.length))
    .filter((id) => planIsStale(cache[harnessPlanKey(CLAUDE_CODE, id)], now))
    .filter((id) => !planPauses.accounts.has(id));
  for (const account of stale) plansQueued = queueAsks(plansQueued, { claude: false, account });
  // One request at a time: with one in flight its loop drains these asks, and
  // otherwise the app's own ask goes first and they follow it.
  if (!plansInFlight) void refreshPlanCache();
}

let planTimer: ReturnType<typeof setInterval> | null = null;
let planResetTimer: ReturnType<typeof setTimeout> | null = null;
let planResetDue = 0;
let plansOnVisible: (() => void) | null = null;

/**
 * Asks once more a few seconds after the soonest cached window resets, when
 * that would otherwise be missed until the poll after next: a new window shows
 * as it starts. Every poll re-arms it, and an earlier deadline already armed is
 * kept, so a poll never pushes the reset back.
 */
function schedulePlanReset(cache: Record<string, CachedPlan>) {
  const now = Date.now();
  const windows = Object.values(cache).flatMap((p) => p.windows);
  const delay = nextPlanRefresh({ source: "plan", windows }, now);
  if (delay >= PLAN_CACHE_POLL_MS) return;
  const due = now + delay;
  if (planResetTimer && planResetDue <= due) return;
  if (planResetTimer) clearTimeout(planResetTimer);
  planResetDue = due;
  planResetTimer = setTimeout(() => {
    planResetTimer = null;
    planResetDue = 0;
    if (typeof document !== "undefined" && document.hidden) return;
    void refreshPlanCache();
    refreshSelectedPlanUsage(true);
  }, delay);
}

/**
 * Whether the thread on screen's own limits want another read: never read, a
 * read that failed (marked stale), or one as old as the poll. The footer meter
 * shows a thread's own windows before the harness's cache (`shownPlan`), so a
 * poll that only refreshed the cache left a Codex thread's numbers where the
 * last turn put them.
 */
export function ownPlanUsageDue(own: PlanUsage | null | undefined, now = Date.now()): boolean {
  return own === undefined || own === null || own.stale === true || planIsStale(own, now);
}

/**
 * Re-reads the limits of the thread on screen when they are due (`force`: at
 * once). `refreshPlanUsage` already keeps one request per thread in flight, and
 * it never touches the Keychain: Claude Code's limits answer null, so the plans
 * read (`refreshPlanCache`) stays the only thing that does.
 */
function refreshSelectedPlanUsage(force = false, now = Date.now()): void {
  const s = get();
  const id = s.selectedId;
  if (!id || s.status !== "ready" || !s.threads[id]?.loaded) return;
  if (force || ownPlanUsageDue(s.planUsage[id], now)) void refreshPlanUsage(id);
}

/**
 * One poll of everything the composer footer and the usage limiter draw: the
 * shared plan cache when anything in it is old, and the selected thread's own
 * limits when they are. A hidden window asks for nothing. Runs on the timer and
 * when the window comes back.
 */
export function pollPlans(now = Date.now()): void {
  if (typeof document !== "undefined" && document.hidden) return;
  refreshSelectedPlanUsage(false, now);
  if (planCacheStale(get().planCache, now)) void refreshPlanCache();
}

/**
 * One poller for the app, not one per thread: every three minutes while the
 * window is visible, and at once when it comes back after being hidden for
 * longer than that. A hidden window asks for nothing, and a cache another read
 * just filled (Settings › Usage reads the same subscriptions) is left alone.
 */
export function startPlanPolling() {
  if (planTimer) return;
  planTimer = setInterval(() => pollPlans(), PLAN_CACHE_POLL_MS);
  if (typeof document === "undefined") return;
  // Back after a while: catch up now rather than at the next tick.
  plansOnVisible = () => pollPlans();
  document.addEventListener("visibilitychange", plansOnVisible);
}

export function stopPlanPolling() {
  if (planTimer) clearInterval(planTimer);
  planTimer = null;
  if (planResetTimer) clearTimeout(planResetTimer);
  planResetTimer = null;
  planResetDue = 0;
  planPauses = NO_PAUSES;
  claudePlansStarted = false;
  if (plansOnVisible) document.removeEventListener("visibilitychange", plansOnVisible);
  plansOnVisible = null;
  plansInFlight = null;
  plansQueued = [];
  planLoginsAskedAt = 0;
}
