// Settings › Usage: every subscription this machine is signed in to, with its
// plan windows (`_rustcode/usage/plans`), whatever thread is open. The page
// reads again by itself every three minutes while its window is visible (and
// at once when it comes back after longer than that), so the numbers and the
// read time move on their own; Refresh reads now, past the engine's minute of
// cache. Opening the page and Refresh are explicit looks: every login, in one
// read. The page's own timer is a background read like the app's poll — the
// default login and the account of the thread on screen, never a login nothing
// was read for or one a failed read paused — and it waits longer after a read
// that failed. Claude's numbers come from Claude Code's sign-in in the macOS
// Keychain: they are read only while the switch below allows it.

import { RefreshCw } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { parsePlans, resetLabel, type UsagePlan } from "../acp/rustcode";
import { Method } from "../acp/types";
import { usageRows } from "../state/logins";
import {
  beginPageRead,
  cachedPage,
  commitPage,
  isLatestPageRead,
  type PageAnswer,
  sameValue,
} from "../state/pageCache";
import {
  accountOn,
  backgroundPlansAsk,
  CLAUDE_USAGE_DEFAULT,
  claudeUsageAllowed,
  commitPlansAnswer,
  harnessOf,
  PLAN_CACHE_POLL_MS,
  request,
  setClaudeUsageAllowed,
  useApp,
} from "../state/store";
import { relativeAge } from "../state/thread";
import {
  answeredPlans,
  failureText,
  USAGE_PLANS_KEY,
  type UsageAnswer,
  usageView,
  watchUsageRefresh,
} from "../state/usagePage";
import { PlanWindow } from "./ComposerUsage";
import { Skeleton, Spinner, Switch } from "./primitives";
import { Action, Group, Row } from "./settingsKit";

export function UsagePage() {
  const status = useApp((s) => s.status);
  const [claude, setClaude] = useState(claudeUsageAllowed);
  const [answer, setAnswer] = useState<PageAnswer<UsageAnswer> | null>(
    () => cachedPage<UsageAnswer>(USAGE_PLANS_KEY) ?? null,
  );
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  // One read at a time: the interval and a press of Refresh never double a
  // Keychain read or an Anthropic request.
  const reading = useRef(false);
  const load = useCallback(async (withClaude: boolean, force: boolean, asked = true): Promise<boolean> => {
    if (reading.current) return true;
    reading.current = true;
    const token = beginPageRead(USAGE_PLANS_KEY);
    setLoading(true);
    try {
      // Showing this page (or Refresh) is the explicit consent that also reads
      // each account's own sign-in at once (`claudeAccounts`); Refresh forces
      // the read past the engine's minute of cache, so a sign-in renewed in
      // Claude Code shows now rather than after that minute.
      //
      // The page's own interval passes `asked: false`, and then it asks like
      // every other background read (`backgroundPlansAsk`, `state/plans.ts`):
      // the default login and the account of the thread on screen, only with
      // consent and never one a failed read paused, so a Deny cannot prompt
      // every three minutes and a named login the user never asked about is
      // never read from the Keychain.
      const ask = asked ? { claude: withClaude, claudeAccounts: withClaude } : backgroundPlansAsk();
      const plans = parsePlans(
        await request(Method.usagePlans, { ...ask, ...(force ? { force: true } : {}) }, 20_000),
      );
      // A whole read (every login) replaces the list; a partial one — Claude
      // off, or the interval's read of the default login and one account —
      // keeps the rows it did not ask, so a named login's numbers stay on
      // screen instead of vanishing, and turning the switch back on has
      // numbers to draw while the logins are read again.
      const shownPage = cachedPage<UsageAnswer>(USAGE_PLANS_KEY);
      const shown = shownPage?.data.plans ?? [];
      const whole = asked && withClaude;
      // A background read with Claude on answers part of the page: the default
      // login and one account, not every named login, so the foot keeps the
      // time of the last whole read instead of saying "just now" over rows it
      // did not ask. With Claude off a read answers every row there is.
      const partial = withClaude && !asked;
      const next = commitPage(
        USAGE_PLANS_KEY,
        token,
        { plans: answeredPlans(shown, plans, whole), asked: whole },
        partial ? shownPage?.readAt : undefined,
      );
      if (!next) return true;
      // The same answer fills the cache the composer's meter reads, and asking
      // here is what lets the background poll read Claude again after a pause.
      // A background read never clears a pause (`asked`), or a Deny would stop
      // holding.
      commitPlansAnswer(plans, whole);
      // An answer equal to what is drawn keeps its list and rows, so the bars do
      // not re-animate; only the foot's read time changes.
      setAnswer((current) =>
        current && sameValue(current.data, next.data) ? { data: current.data, readAt: next.readAt } : next,
      );
      setError(null);
      return true;
    } catch (e) {
      if (isLatestPageRead(USAGE_PLANS_KEY, token)) setError(e instanceof Error ? e.message : String(e));
      return false;
    } finally {
      reading.current = false;
      if (isLatestPageRead(USAGE_PLANS_KEY, token)) setLoading(false);
    }
  }, []);
  useEffect(() => {
    if (status !== "ready") {
      // The engine went away, and with it its answers: draw nothing from it.
      setAnswer(cachedPage<UsageAnswer>(USAGE_PLANS_KEY) ?? null);
      setError(null);
      setLoading(false);
      return;
    }
    const cached = cachedPage<UsageAnswer>(USAGE_PLANS_KEY) ?? null;
    const view = usageView(cached, claude, null);
    if (view.fetch) void load(claude, view.force);
  }, [status, claude, load]);
  // The page keeps itself current: a background read every three minutes while
  // its window is visible, one on coming back after being hidden longer than
  // that, and a longer wait after one that failed. The numbers, the bars and
  // the foot's read time then move without a press; a press of Refresh reads now
  // instead, past the engine's minute of cache. The tick reads even when the
  // app's own poll just filled the cache: the rows on screen come from this
  // read, and the engine shares a read that is already under way.
  useEffect(() => {
    if (status !== "ready") return;
    return watchUsageRefresh({
      read: () => load(claude, false, false),
      reading: () => reading.current,
      visible: () => document.visibilityState === "visible",
      readAt: () => cachedPage<UsageAnswer>(USAGE_PLANS_KEY)?.readAt ?? null,
      everyMs: PLAN_CACHE_POLL_MS,
    });
  }, [status, claude, load]);
  const allowClaude = (on: boolean) => {
    setClaudeUsageAllowed(on);
    setClaude(on);
  };
  // A thread still on the harness's own login ("Original login") keeps its row:
  // dropping it would leave Settings › Usage with nothing for that thread.
  const keepOwnLogin = useApp((s) => {
    const thread = s.selectedId ?? undefined;
    return harnessOf(s, thread) === "claude-code" && !accountOn(s, thread);
  });
  const view = usageView(answer, claude, error);
  if (view.plans === null) {
    return view.fatal ? <p className="text-callout text-warning">{view.fatal}</p> : <PlansSkeleton />;
  }
  // The harness's own login row is dropped while named accounts are there, unless
  // the thread on screen still runs on it; the composer's switch moves to the
  // first Claude group left (state/logins.ts).
  const rows = usageRows(view.plans, keepOwnLogin);
  return (
    <>
      {rows.plans.map((p) => (
        <Group
          key={p.id}
          title={
            <span>
              {p.name}
              {p.plan && <span className="ml-1.5 font-normal text-fg-faint capitalize">{p.plan}</span>}
            </span>
          }
          action={p.via && <span className="text-caption text-fg-faint">from {p.via}</span>}
        >
          {p.status === "ok" && p.windows.length > 0 ? (
            <div className="flex flex-col gap-4 px-4 py-4">
              {p.asOf !== undefined && (
                <p className="text-caption text-fg-faint">
                  as of {relativeAge(p.asOf)} · the last numbers RustCode read for this login; run a Claude
                  Code thread on it to refresh them.
                </p>
              )}
              {p.windows.map((w) => (
                <PlanWindow key={w.label} window={w} title={w.label} />
              ))}
            </div>
          ) : p.status === "notAsked" ? null : (
            <Row
              title={
                p.status === "signedOut"
                  ? `Not signed in to ${p.via ?? p.name}`
                  : p.status === "notInstalled"
                    ? `${p.via ?? p.name} not found`
                    : p.status === "ok"
                      ? "No plan windows reported"
                      : failureText(p).title
              }
              subtitle={
                p.error ??
                failureText(p).subtitle ??
                (p.status === "signedOut"
                  ? `Sign in with ${p.via ?? p.name} on this computer to see this plan here.`
                  : p.status === "notInstalled"
                    ? `Install ${p.via ?? p.name} and sign in to see this plan here.`
                    : undefined)
              }
            />
          )}
          {p.id === rows.switchIn && (
            <Row
              title="Show Claude usage in the composer"
              reset={
                claude
                  ? () => {
                      setClaudeUsageAllowed(CLAUDE_USAGE_DEFAULT);
                      setClaude(CLAUDE_USAGE_DEFAULT);
                    }
                  : undefined
              }
              subtitle="RustCode reads Claude Code's sign-in every few minutes while the app is open, so the composer and this page show your plan. Your system may ask once to allow it. Only reads; never signs in or out."
            >
              <Switch label="Show Claude usage in the composer" checked={claude} onChange={allowClaude} />
            </Row>
          )}
        </Group>
      ))}
      <div className="flex items-center gap-2 px-3 text-caption text-fg-faint sm:px-4">
        <span className="flex-1">
          {view.note ? (
            <span className="text-warning">Could not refresh: {view.note}</span>
          ) : view.readAt ? (
            `${Date.now() - view.readAt < 60_000 ? "Read just now" : `Read ${relativeAge(view.readAt)}`} · this page refreshes every ${
              PLAN_CACHE_POLL_MS / 60_000
            } minutes. ${soonestReset(view.plans) ?? ""}`
          ) : null}
        </span>
        <Action onClick={() => void load(claude, true)} disabled={loading}>
          {loading ? <Spinner size={11} /> : <RefreshCw size={12} />} Refresh
        </Action>
      </div>
    </>
  );
}

function soonestReset(plans: UsagePlan[]): string | undefined {
  const next = plans
    .flatMap((p) => p.windows)
    .filter((w) => w.resetsAt)
    .sort((a, b) => a.resetsAt! - b.resetsAt!)[0];
  return next ? `The soonest window ${resetLabel(next)}.` : undefined;
}

/**
 * Where the plans will be: two group cards, each a title and the windows'
 * label, bar and numbers, at the size the real rows take.
 */
function PlansSkeleton() {
  return (
    <div role="status" aria-label="Loading">
      {[0, 1].map((card) => (
        <section key={card} className="mb-6">
          <div className="mb-2 flex min-h-7 items-center px-4">
            <Skeleton className="h-3 w-24" />
          </div>
          <div className="flex flex-col gap-4 rounded-xl bg-surface px-4 py-4">
            {[0, 1].map((row) => (
              <div key={row} className="flex flex-col gap-1.5 px-0.5">
                <div className="flex items-baseline justify-between gap-3">
                  <Skeleton className="h-3 w-16" />
                  <Skeleton className="h-2.5 w-24" />
                </div>
                <Skeleton className="h-[6px] rounded-full" />
                <div className="flex justify-between">
                  <Skeleton className="h-2.5 w-14" />
                  <Skeleton className="h-2.5 w-12" />
                </div>
              </div>
            ))}
          </div>
        </section>
      ))}
    </div>
  );
}
