// Settings › Usage: every subscription this machine is signed in to, with its
// plan windows (`_rustcode/usage/plans`), whatever thread is open. Claude's
// numbers come from Claude Code's sign-in in the macOS Keychain: while the app
// is open it is read every few minutes, so the composer's meter has the plan
// (macOS may ask once to allow it); the switch below turns that off.

import { RefreshCw } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
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
  CLAUDE_USAGE_DEFAULT,
  claudeUsageAllowed,
  commitPlansAnswer,
  request,
  setClaudeUsageAllowed,
  useApp,
} from "../state/store";
import { relativeAge } from "../state/thread";
import { answeredPlans, failureText, USAGE_PLANS_KEY, type UsageAnswer, usageView } from "../state/usagePage";
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
  const load = useCallback(async (withClaude: boolean, force: boolean) => {
    const token = beginPageRead(USAGE_PLANS_KEY);
    setLoading(true);
    try {
      // Showing this page (or Refresh) is the explicit consent that also
      // reads each account's own sign-in at once; the background reads one
      // account at a time, the one the thread on screen runs on. Refresh forces
      // the read past the engine's minute of cache, so a sign-in renewed in
      // Claude Code shows now rather than after that minute.
      const plans = parsePlans(
        await request(
          Method.usagePlans,
          { claude: withClaude, claudeAccounts: withClaude, ...(force ? { force: true } : {}) },
          20_000,
        ),
      );
      // A whole read replaces the list; one with Claude off replaces only the
      // rows it asked, so turning the switch back on has numbers to draw while
      // it reads the logins again.
      const shown = cachedPage<UsageAnswer>(USAGE_PLANS_KEY)?.data.plans ?? [];
      const next = commitPage(USAGE_PLANS_KEY, token, {
        plans: answeredPlans(shown, plans, withClaude),
        asked: withClaude,
      });
      if (!next) return;
      // The same answer fills the cache the composer's meter reads, and asking
      // here is what lets the background poll read Claude again after a pause.
      commitPlansAnswer(plans, withClaude);
      // An answer equal to what is drawn keeps its list and rows, so the bars do
      // not re-animate; only the foot's read time changes.
      setAnswer((current) =>
        current && sameValue(current.data, next.data) ? { data: current.data, readAt: next.readAt } : next,
      );
      setError(null);
    } catch (e) {
      if (!isLatestPageRead(USAGE_PLANS_KEY, token)) return;
      setError(e instanceof Error ? e.message : String(e));
    } finally {
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
  const allowClaude = (on: boolean) => {
    setClaudeUsageAllowed(on);
    setClaude(on);
  };
  const view = usageView(answer, claude, error);
  if (view.plans === null) {
    return view.fatal ? <p className="text-callout text-warning">{view.fatal}</p> : <PlansSkeleton />;
  }
  // The harness's own login row is dropped whenever named accounts are there; the
  // composer's switch moves to the first Claude group left (state/logins.ts).
  const rows = usageRows(view.plans);
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
                  ? `Sign in with ${p.via ?? p.name} on this Mac to see this plan here.`
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
              subtitle="RustCode reads Claude Code's sign-in every few minutes while the app is open, so the composer shows your plan. macOS may ask once to allow it in the Keychain. Only reads; never signs in or out."
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
            `Read ${Date.now() - view.readAt < 60_000 ? "just now" : relativeAge(view.readAt)}. ${
              soonestReset(view.plans) ?? ""
            }`
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
        <section key={card} className="mb-8">
          <div className="mb-2.5 flex min-h-7 items-center px-3 sm:px-4">
            <Skeleton className="h-3 w-24" />
          </div>
          <div className="flex flex-col gap-4 rounded-xl border border-border-strong bg-surface-2/40 px-4 py-4">
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
