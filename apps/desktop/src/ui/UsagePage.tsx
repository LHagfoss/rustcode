// Settings › Usage: every subscription across this computer and its paired
// machines, merged by subscription under "All machines" (or inspected per
// machine). Local logins come from `_rustcode/usage/plans`, and each connected
// machine's own subscriptions come from `_rustcode/usage/shared` (numbers plus
// an opaque account fingerprint, never credentials). The page reads again by
// itself every three minutes while its window is visible (and at once when it
// comes back after longer than that), so the numbers and the read time move on
// their own; Refresh reads now, past the engine's minute of cache, across every
// connected machine. Opening the page and Refresh are explicit looks: every
// login, in one read. The page's own timer is a background read like the app's
// poll — the default login and the account of the thread on screen, never a
// login nothing was read for or one a failed read paused — and it waits longer
// after a read that failed.

import { useCallback, useEffect, useMemo, useState } from "react";
import { parsePlans, resetLabel, type UsagePlan } from "../acp/rustcode";
import { Method } from "../acp/types";
import {
  groupMergedPlans,
  type HostPlans,
  type MachineScope,
  type MergedUsagePlan,
  mergeUsagePlans,
  planProvenance,
  sharedPlans,
} from "../state/fleetUsage";
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
  commitPlansAnswer,
  connectionFor,
  harnessOf,
  PLAN_CACHE_POLL_MS,
  request,
  useApp,
} from "../state/store";
import { relativeAge } from "../state/thread";
import { LOCAL_HOST } from "../state/threadKeys";
import {
  answeredPlans,
  failureText,
  oneReadAtATime,
  USAGE_PLANS_KEY,
  type UsageAnswer,
  usageView,
  watchUsageRefresh,
} from "../state/usagePage";
import { HarnessIcon } from "./HarnessIcon";
import { Skeleton, Spinner, Tooltip } from "./primitives";
import { Action, Chip, Choice, Group, Row, SectionTitle, SettingsHeaderActions } from "./settingsKit";
import { PlanWindow } from "./ThreadUsage";

/** One page read, optionally bypassing the cache as an explicit look. */
interface PageRead {
  force: boolean;
  asked: boolean;
}

interface RemoteRead {
  plans: UsagePlan[];
  readAt: number;
}

/**
 * Each paired machine's last answer, kept in memory like `state/pageCache.ts`'s,
 * so reopening the page draws its rows at once and reads again behind them
 * instead of showing a skeleton (or no Antigravity card) until it answers.
 */
const remoteReads: Record<string, RemoteRead> = {};

/**
 * How soon to ask again when an answer's numbers are the last good read while
 * a new one runs on the engine (`refreshing`): Antigravity answers at once
 * from them and reads `agy` behind them, which takes a few seconds.
 */
const REFRESHING_REREAD_MS = 4_000;

const isRefreshing = (plans: readonly UsagePlan[]) => plans.some((plan) => plan.refreshing);

export function UsagePage() {
  const status = useApp((s) => s.engine.phase);
  const connectedHosts = useApp((s) => s.connectedHosts);
  const machineNames = useApp((s) => s.machineNames);
  const [scope, setScope] = useState<MachineScope>("all");
  const [answer, setAnswer] = useState<PageAnswer<UsageAnswer> | null>(
    () => cachedPage<UsageAnswer>(USAGE_PLANS_KEY) ?? null,
  );
  const [byHost, setByHost] = useState<Record<string, RemoteRead>>(() => ({ ...remoteReads }));
  const [remoteLoading, setRemoteLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  // `force` is Refresh: the machine reads its Antigravity CLI again now (its
  // other logins stay on its minute of cache, `_rustcode/usage/shared`).
  const readMachines = useCallback(async (hosts: readonly string[], force = false) => {
    if (!hosts.length) return;
    // Rows already on screen refresh quietly: only Refresh, or a machine with
    // nothing drawn yet, spins the button.
    const spin = force || hosts.some((host) => !remoteReads[host]);
    if (spin) setRemoteLoading(true);
    try {
      await Promise.all(
        hosts.map(async (host) => {
          const conn = connectionFor(host);
          if (!conn) return;
          try {
            const result = await conn.request(Method.usageShared, force ? { force: true } : {}, 20_000);
            const plans = sharedPlans(result);
            const read = { plans, readAt: Date.now() };
            remoteReads[host] = read;
            setByHost((was) => ({ ...was, [host]: read }));
          } catch {
            // A machine whose engine predates `_rustcode/usage/shared` or drops mid-read stays silent.
          }
        }),
      );
    } finally {
      if (spin) setRemoteLoading(false);
    }
  }, []);

  // One read at a time: the interval and a press of Refresh never double a
  // Keychain read or an Anthropic request. An explicit look that comes while a
  // read is in flight
  // runs once it ends instead of being dropped (`oneReadAtATime`).
  const readPage = async ({ force, asked }: PageRead): Promise<boolean> => {
    const token = beginPageRead(USAGE_PLANS_KEY);
    setLoading(true);
    try {
      // Showing this page (or Refresh) is the explicit look that also reads
      // each account's own sign-in at once (`claudeAccounts`); Refresh forces
      // the read past the engine's minute of cache, so a sign-in renewed in
      // Claude Code shows now rather than after that minute.
      //
      // The page's own interval passes `asked: false`, and then it asks like
      // every other background read (`backgroundPlansAsk`, `state/plans.ts`):
      // the default login and the account of the thread on screen, never one a failed read paused, so a Deny cannot prompt
      // every three minutes and a named login the user never asked about is
      // never read from the Keychain.
      const ask = asked ? { claude: true, claudeAccounts: true } : backgroundPlansAsk();
      const plans = parsePlans(
        await request(
          Method.usagePlans,
          { ...ask, antigravity: true, ...(force ? { force: true } : {}) },
          20_000,
        ),
      );
      // A whole read (every login) replaces the list; the interval's partial read
      // keeps the rows it did not ask, so a named login's numbers stay on
      // screen instead of vanishing.
      const shownPage = cachedPage<UsageAnswer>(USAGE_PLANS_KEY);
      const shown = shownPage?.data.plans ?? [];
      const whole = asked;
      // A background read answers part of the page: the default
      // login and one account, not every named login, so the foot keeps the
      // time of the last whole read instead of saying "just now" over rows it
      // did not ask.
      const partial = !asked;
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
      // Antigravity's quota groups apply to different sets of models.
      commitPlansAnswer(
        plans.filter((plan) => plan.harness !== "antigravity"),
        whole,
      );
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
      if (isLatestPageRead(USAGE_PLANS_KEY, token)) setLoading(false);
    }
  };
  // Made once: `readPage` uses only state setters and module state, which do
  // not change between renders.
  const [reads] = useState(() => oneReadAtATime<PageRead>(readPage));
  const load = useCallback(
    (force: boolean, asked = true): Promise<boolean> => reads.run({ force, asked }, asked),
    [reads],
  );
  useEffect(() => {
    if (status !== "ready") {
      // The engine went away, and with it its answers: draw nothing from it.
      setAnswer(cachedPage<UsageAnswer>(USAGE_PLANS_KEY) ?? null);
      setError(null);
      setLoading(false);
      return;
    }
    const cached = cachedPage<UsageAnswer>(USAGE_PLANS_KEY) ?? null;
    const view = usageView(cached, null);
    if (view.fetch) void load(view.force);
  }, [status, load]);
  // Read connected machines on open and on the same poll interval while visible.
  useEffect(() => {
    if (!connectedHosts.length) return;
    void readMachines(connectedHosts);
    const timer = setInterval(() => {
      if (document.visibilityState === "visible") void readMachines(connectedHosts);
    }, PLAN_CACHE_POLL_MS);
    return () => clearInterval(timer);
  }, [connectedHosts, readMachines]);
  // Numbers answered while a new read runs behind them (`refreshing`): ask
  // again shortly, so the new ones replace them without a press of Refresh.
  const localRefreshing = !!answer && isRefreshing(answer.data.plans);
  // biome-ignore lint/correctness/useExhaustiveDependencies: each answer that still says refreshing asks once more
  useEffect(() => {
    if (status !== "ready" || !localRefreshing) return;
    const timer = setTimeout(() => void load(false, false), REFRESHING_REREAD_MS);
    return () => clearTimeout(timer);
  }, [status, localRefreshing, answer, load]);
  const remoteRefreshing = connectedHosts.filter((host) => {
    const read = byHost[host];
    return read && isRefreshing(read.plans);
  });
  const remoteRefreshingKey = remoteRefreshing.join("\n");
  // biome-ignore lint/correctness/useExhaustiveDependencies: keyed by the hosts and their answers
  useEffect(() => {
    if (!remoteRefreshing.length) return;
    const timer = setTimeout(() => void readMachines(remoteRefreshing), REFRESHING_REREAD_MS);
    return () => clearTimeout(timer);
  }, [remoteRefreshingKey, byHost, readMachines]);
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
      read: () => load(false, false),
      reading: () => reads.busy(),
      visible: () => document.visibilityState === "visible",
      readAt: () => cachedPage<UsageAnswer>(USAGE_PLANS_KEY)?.readAt ?? null,
      everyMs: PLAN_CACHE_POLL_MS,
    });
  }, [status, load, reads.busy]);
  // A thread still on the harness's own login ("Original login") keeps its row:
  // dropping it would leave Settings › Usage with nothing for that thread.
  const keepOwnLogin = useApp((s) => {
    const thread = s.selectedId ?? undefined;
    return harnessOf(s, thread) === "claude-code" && !accountOn(s, thread);
  });
  const view = usageView(answer, error);
  const effectiveScope: MachineScope =
    scope === "all" || scope === LOCAL_HOST || connectedHosts.includes(scope) ? scope : "all";
  const hasConnected = connectedHosts.length > 0;

  const localRows = useMemo(
    () => (view.plans ? usageRows(view.plans, keepOwnLogin).plans : null),
    [view.plans, keepOwnLogin],
  );

  const displayed = useMemo((): {
    entries: MergedUsagePlan[];
    readAt: number | null;
    showProvenance: boolean;
  } | null => {
    if (effectiveScope !== "all" && effectiveScope !== LOCAL_HOST) {
      const remote = byHost[effectiveScope];
      if (!remote) return null;
      const name = machineNames[effectiveScope] ?? effectiveScope;
      return {
        entries: remote.plans.map((plan, idx) => ({
          key: `${effectiveScope}:${plan.id}:${idx}`,
          plan,
          winnerHost: effectiveScope,
          winnerMachine: name,
          sources: [{ host: effectiveScope, machine: name, status: plan.status, readAt: remote.readAt }],
        })),
        readAt: remote.readAt,
        showProvenance: false,
      };
    }
    if (!localRows) return null;
    if (effectiveScope === LOCAL_HOST || !hasConnected) {
      return {
        entries: localRows.map((plan, idx) => ({
          key: `local:${plan.id}:${idx}`,
          plan,
          winnerHost: LOCAL_HOST,
          winnerMachine: "This computer",
          sources: [
            {
              host: LOCAL_HOST,
              machine: "This computer",
              status: plan.status,
              readAt: view.readAt ?? undefined,
            },
          ],
        })),
        readAt: view.readAt,
        showProvenance: false,
      };
    }
    const hosts: HostPlans[] = [
      {
        host: LOCAL_HOST,
        machine: "This computer",
        plans: localRows,
        readAt: view.readAt ?? undefined,
      },
    ];
    let latestRead = view.readAt ?? 0;
    for (const host of connectedHosts) {
      const remote = byHost[host];
      if (!remote) continue;
      latestRead = Math.max(latestRead, remote.readAt);
      hosts.push({
        host,
        machine: machineNames[host] ?? host,
        plans: remote.plans,
        readAt: remote.readAt,
      });
    }
    return {
      entries: mergeUsagePlans(hosts),
      readAt: latestRead || view.readAt,
      showProvenance: true,
    };
  }, [effectiveScope, localRows, hasConnected, connectedHosts, byHost, machineNames, view.readAt]);

  const refreshAll = () => {
    if (effectiveScope === "all" || effectiveScope === LOCAL_HOST) void load(true);
    if (effectiveScope === "all") void readMachines(connectedHosts, true);
    else if (effectiveScope !== LOCAL_HOST) void readMachines([effectiveScope], true);
  };

  if (view.plans === null && (effectiveScope === "all" || effectiveScope === LOCAL_HOST)) {
    return view.fatal ? <p className="text-callout text-warning">{view.fatal}</p> : <PlansSkeleton />;
  }
  if (!displayed) {
    return <PlansSkeleton />;
  }

  const busy = loading || remoteLoading;
  const displayedPlans = displayed.entries.map((entry) => entry.plan);
  const includesLocal = effectiveScope === "all" || effectiveScope === LOCAL_HOST;

  return (
    <>
      <SettingsHeaderActions>
        <Action onClick={refreshAll} disabled={busy}>
          {busy ? <Spinner size={12} /> : null}
          Refresh
        </Action>
      </SettingsHeaderActions>
      {hasConnected && (
        <div className="mb-4 flex flex-wrap items-center justify-between gap-2 px-3 sm:px-4">
          <Choice<MachineScope>
            label="Machines"
            value={effectiveScope}
            options={[
              { value: "all", label: "All machines" },
              { value: LOCAL_HOST, label: "This computer" },
              ...connectedHosts.map((host) => ({ value: host, label: machineNames[host] ?? host })),
            ]}
            onChange={setScope}
          />
        </div>
      )}
      {displayed.entries.length > 0 ? (
        <PlanSections entries={displayed.entries} showProvenance={displayed.showProvenance} />
      ) : (
        <Group>
          <Row
            title="No subscriptions reported"
            subtitle={
              effectiveScope === "all" || effectiveScope === LOCAL_HOST
                ? "Sign in to a harness on this computer to see its plan usage here."
                : `No subscriptions were reported on ${machineNames[effectiveScope] ?? effectiveScope}.`
            }
          />
        </Group>
      )}
      <div className="flex items-center gap-2 px-3 text-caption text-fg-muted sm:px-4">
        <span className="flex-1">
          {includesLocal && view.note ? (
            <span className="text-warning">Could not refresh: {view.note}</span>
          ) : displayed.readAt ? (
            `${Date.now() - displayed.readAt < 60_000 ? "Read just now" : `Read ${relativeAge(displayed.readAt)}`} · this page refreshes every ${
              PLAN_CACHE_POLL_MS / 60_000
            } minutes. ${soonestReset(displayedPlans) ?? ""}`
          ) : null}
        </span>
      </div>
      {includesLocal && (
        <p className="px-4 text-caption text-fg-muted">
          Your system may ask once to allow RustCode to read Claude Code’s sign-in for usage.
        </p>
      )}
    </>
  );
}

/** Each subscription's card, grouped by harness, with an optional machine provenance chip. */
function PlanSections({ entries, showProvenance }: { entries: MergedUsagePlan[]; showProvenance: boolean }) {
  return (
    <>
      {groupMergedPlans(entries).map(({ harness, plans }) => {
        const name = plans[0]?.plan.via ?? harness;
        return (
          <section key={harness}>
            <SectionTitle
              title={
                <>
                  <HarnessIcon id={harness} name={name} size={16} />
                  {name}
                </>
              }
            />
            {plans.map((entry) => {
              const p = entry.plan;
              const where =
                entry.winnerHost === LOCAL_HOST ? "on this computer" : `on ${entry.winnerMachine}`;
              const provenance = showProvenance ? planProvenance(entry) : null;
              return (
                <Group
                  key={entry.key}
                  minor
                  title={
                    <span>
                      {p.name}
                      {p.plan && (
                        <span className="ml-1.5 font-normal text-fg-faint capitalize">{p.plan}</span>
                      )}
                    </span>
                  }
                  action={
                    provenance ? (
                      <Tooltip label={provenance.detail}>
                        <span>
                          <Chip>{provenance.label}</Chip>
                        </span>
                      </Tooltip>
                    ) : undefined
                  }
                >
                  {p.status === "ok" && p.windows.length > 0 ? (
                    <div className="flex flex-col gap-4 px-4 py-4">
                      {p.asOf !== undefined && (
                        <p className="text-caption text-fg-muted">
                          as of {relativeAge(p.asOf)} · the last numbers RustCode read for this login; run a
                          Claude Code thread on it {where} to refresh them.
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
                          ? `Sign in with ${p.via ?? p.name} ${where} to see this plan here.`
                          : p.status === "notInstalled"
                            ? `Install ${p.via ?? p.name} ${where} and sign in to see this plan here.`
                            : undefined)
                      }
                    />
                  )}
                </Group>
              );
            })}
          </section>
        );
      })}
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
