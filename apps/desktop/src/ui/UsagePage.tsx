// Settings › Usage: every subscription this machine is signed in to, with its
// plan windows (`_rustcode/usage/plans`), whatever thread is open. Claude's
// numbers come from Claude Code's sign-in in the macOS Keychain: while the app
// is open it is read every few minutes, so the composer's meter has the plan
// (macOS may ask once to allow it); the switch below turns that off.

import { RefreshCw } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { parsePlans, planHarness, resetLabel, type UsagePlan } from "../acp/rustcode";
import { Method } from "../acp/types";
import {
  CLAUDE_USAGE_DEFAULT,
  claudeUsageAllowed,
  commitPlansAnswer,
  request,
  setClaudeUsageAllowed,
  useApp,
} from "../state/store";
import { relativeAge } from "../state/thread";
import { PlanWindow } from "./ComposerUsage";
import { Skeleton, Spinner, Switch } from "./primitives";
import { Action, Group, Row } from "./settingsKit";

export function UsagePage() {
  const status = useApp((s) => s.status);
  const [claude, setClaude] = useState(claudeUsageAllowed);
  const [plans, setPlans] = useState<UsagePlan[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [readAt, setReadAt] = useState<number | null>(null);
  const load = useCallback(async (withClaude: boolean) => {
    setLoading(true);
    try {
      const answer = parsePlans(await request(Method.usagePlans, { claude: withClaude }, 20_000));
      setPlans(answer);
      // The same answer fills the cache the composer's meter reads, and asking
      // here is what lets the background poll read Claude again after a pause.
      commitPlansAnswer(answer, withClaude);
      setReadAt(Date.now());
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, []);
  useEffect(() => {
    if (status === "ready") void load(claude);
  }, [status, claude, load]);
  const allowClaude = (on: boolean) => {
    setClaudeUsageAllowed(on);
    setClaude(on);
  };
  if (!plans) {
    return error ? <p className="text-callout text-warning">{error}</p> : <PlansSkeleton />;
  }
  return (
    <>
      {plans.map((p) => (
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
                      : "Could not read the usage"
              }
              subtitle={
                p.error ??
                (p.status === "signedOut"
                  ? `Sign in with ${p.via ?? p.name} on this Mac to see this plan here.`
                  : p.status === "notInstalled"
                    ? `Install ${p.via ?? p.name} and sign in to see this plan here.`
                    : undefined)
              }
            />
          )}
          {planHarness(p.id) === "claude-code" && (
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
          {error ? (
            <span className="text-warning">Could not refresh: {error}</span>
          ) : readAt ? (
            `Read ${Date.now() - readAt < 60_000 ? "just now" : relativeAge(readAt)}. ${soonestReset(plans) ?? ""}`
          ) : null}
        </span>
        <Action onClick={() => void load(claude)} disabled={loading}>
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
