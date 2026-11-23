// Jobs (issues 3892, 3937): prompts the scheduler daemon runs once, on a schedule, manually, or
// from a webhook, each run in a fresh thread or one chosen thread (docs/daemon.md › Jobs). Reached
// from the sidebar's Jobs row. Jobs live across the entire paired machine network: every connected
// computer's scheduler is read and managed directly over ACP, and a hub's members remain reachable
// too. Each job is a card with its agent, last result, schedule and next run; the card opens one
// dialog that edits the job beside its runs. Jobs paused by failures lead. Review is a timeline by
// day of failed runs and open threads, a repeated failure folded into one row.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { HarnessSnapshot } from "../acp/types";
import { catalogModelTitle } from "../lib/modelNames";
import {
  fetchFleetLoops,
  type HostLoopsState,
  loopKey,
  loopMachineLabel,
  remoteLoopError,
} from "../state/fleetLoops";
import { lastAccountFor } from "../state/harnessActions";
import { newThreadAccountOn } from "../state/logins";
import {
  deleteLoop,
  groupReview,
  type Loop,
  type LoopHubMember,
  loadReviewed,
  loopState,
  rememberLoops,
  reviewItems,
  saveReviewed,
  startScheduler,
  useLoopThreads,
} from "../state/loops";
import { machineName, selfName, useLocalIdentity, useMachines } from "../state/machines";
import { askConfirm, errorText, loadMachineHarnesses, toast, useApp } from "../state/store";
import { LOCAL_HOST } from "../state/threadKeys";
import { statusOf } from "../state/threadStatus";
import { CheckCheck, Plus, Repeat } from "./icons";
import { JobCard, listening } from "./JobCard";
import { JobReview } from "./JobReview";
import { showJob } from "./JobRunBar";
import { LoopDialog } from "./LoopDialog";
import { ViewHeader } from "./MainHeader";
import { Callout, cx, Pill, Segments, Select, Spinner } from "./primitives";
import { Loading } from "./settingsKit";

/** Poll quickly while a run is active; rest at 30s when idle. */
const IDLE_REFRESH_MS = 30_000;
const RUNNING_REFRESH_MS = 5_000;
const ALL_MACHINES = "all";

type Tab = "jobs" | "review";

export { showJob };

function jobAgent(
  loop: Loop,
  harnesses: HarnessSnapshot | null,
  signedOut: Parameters<typeof newThreadAccountOn>[2],
): string {
  if (loop.threadId) return "Existing thread";
  const harness = harnesses?.harnesses.find(
    (item) => item.id === (loop.harness || harnesses.default.harness),
  );
  const effectiveAccountId =
    loop.account ||
    (!loop.machine && !loop.host && harness
      ? newThreadAccountOn(harness, lastAccountFor(harness.id), signedOut)
      : null) ||
    undefined;
  const account = harness?.accounts?.find((item) => item.id === effectiveAccountId);
  return [
    harness?.name ?? loop.harness,
    account?.name ?? effectiveAccountId,
    catalogModelTitle(loop.model, harness),
    loop.effort && `${loop.effort} effort`,
  ]
    .filter(Boolean)
    .join(" · ");
}

export function Loops() {
  const enginePhase = useApp((s) => s.engine.phase);
  const connectedHosts = useApp((s) => s.connectedHosts);
  const sessions = useApp((s) => s.sessions);
  const harnesses = useApp((s) => s.harnesses);
  const remoteHarnesses = useApp((s) => s.remoteHarnesses);
  const signedOut = useApp((s) => s.signedOutLogins);
  const statusMarks = useApp((s) => s.statusMarks);
  const machines = useMachines((s) => s.machines);
  const local = useLocalIdentity();
  const localName = local.displayName ?? selfName();

  const [byHost, setByHost] = useState<Record<string, HostLoopsState> | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [scopeHost, setScopeHost] = useState<string>(ALL_MACHINES);
  const [tab, setTab] = useState<Tab>("jobs");
  const [editing, setEditing] = useState<Loop | null>(null);
  const [duplicating, setDuplicating] = useState(false);
  const [dialogOpen, setDialogOpen] = useState(false);
  const [startingHost, setStartingHost] = useState<string | null>(null);
  const [reviewed, setReviewed] = useState(loadReviewed);
  const [now, setNow] = useState(() => Date.now());

  const connectedKey = connectedHosts.join(",");
  const reload = useCallback(async () => {
    if (enginePhase !== "ready") return;
    const hosts = connectedKey ? connectedKey.split(",") : [];
    const result = await fetchFleetLoops(hosts);
    if (result.localError && !result.byHost[LOCAL_HOST]) {
      setError(result.localError);
      return;
    }
    setError(null);
    setByHost(result.byHost);
    const every = Object.values(result.byHost).flatMap((entry) => entry.loops);
    rememberLoops(every);
  }, [enginePhase, connectedKey]);

  useEffect(() => {
    void reload();
  }, [reload]);

  const allLoops = useMemo(
    () => (byHost ? Object.values(byHost).flatMap((entry) => entry.loops) : []),
    [byHost],
  );
  const hasRunning = allLoops.some((loop) => loopState(loop) === "running");

  useEffect(() => {
    const interval = hasRunning ? RUNNING_REFRESH_MS : IDLE_REFRESH_MS;
    const timer = setInterval(() => {
      setNow(Date.now());
      void reload();
    }, interval);
    return () => clearInterval(timer);
  }, [reload, hasRunning]);

  // Load harnesses for connected hosts that have jobs so cards show real model/account names.
  useEffect(() => {
    for (const host of connectedHosts) {
      if (byHost?.[host]?.loops.length && !remoteHarnesses[host]) {
        void loadMachineHarnesses(host, { report: false });
      }
    }
  }, [connectedHosts, byHost, remoteHarnesses]);

  const hasPeers = machines.length > 0;
  const effectiveScope =
    hasPeers &&
    (scopeHost === ALL_MACHINES || scopeHost === LOCAL_HOST || machines.some((m) => m.id === scopeHost))
      ? scopeHost
      : hasPeers
        ? ALL_MACHINES
        : LOCAL_HOST;

  const loops = useMemo(
    () =>
      effectiveScope === ALL_MACHINES ? allLoops : allLoops.filter((loop) => loop.host === effectiveScope),
    [allLoops, effectiveScope],
  );

  const hostLabel = useCallback(
    (host: string) => {
      if (!host) return localName;
      const machine = machines.find((entry) => entry.id === host);
      return machine ? machineName(machine) : host;
    },
    [machines, localName],
  );

  const act = useCallback(
    (what: string, run: () => Promise<void>) => {
      void run().then(
        () => {
          toast(what, "success");
          setNow(Date.now());
          void reload();
        },
        (e) => toast(errorText(e), "error"),
      );
    },
    [reload],
  );

  const openJob = useCallback((loop: Loop, duplicate = false) => {
    setEditing(loop);
    setDuplicating(duplicate);
    setDialogOpen(true);
  }, []);

  /** Opened from one of a job's runs: that job's dialog, matched by key, id or name. */
  const reveal = useLoopThreads((s) => s.reveal);
  const listRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!reveal || !byHost) return;
    const match = allLoops.find(
      (loop) => loopKey(loop) === reveal || loop.id === reveal || loop.name === reveal,
    );
    if (!match) return;
    setTab("jobs");
    if (effectiveScope !== ALL_MACHINES && effectiveScope !== match.host) setScopeHost(ALL_MACHINES);
    useLoopThreads.setState({ reveal: null });
    openJob(match);
    requestAnimationFrame(() =>
      listRef.current
        ?.querySelector(`[data-loop="${CSS.escape(loopKey(match))}"]`)
        ?.scrollIntoView({ block: "nearest" }),
    );
  }, [reveal, byHost, allLoops, effectiveScope, openJob]);

  const attention: Loop[] = [];
  const upcoming: Loop[] = [];
  const inactive: Loop[] = [];
  for (const loop of loops) {
    if (loopState(loop) === "failing") attention.push(loop);
    else if (
      loopState(loop) === "running" ||
      (!loop.paused && loop.nextDueAt !== undefined) ||
      listening(loop)
    )
      upcoming.push(loop);
    else inactive.push(loop);
  }
  const byName = (a: Loop, b: Loop) => a.name.localeCompare(b.name);
  attention.sort(byName);
  upcoming.sort((a, b) => (a.nextDueAt ?? Infinity) - (b.nextDueAt ?? Infinity) || byName(a, b));
  inactive.sort((a, b) => Number(b.paused) - Number(a.paused) || byName(a, b));
  const groups = [
    { title: "Needs attention", loops: attention },
    { title: "Active", loops: upcoming },
    { title: "Paused & inactive", loops: inactive },
  ].filter((group) => group.loops.length > 0);

  // biome-ignore lint/correctness/useExhaustiveDependencies: statusMarks is what statusOf reads
  const open = useMemo(() => {
    const listed = new Set(sessions.map((session) => session.id));
    const state = useApp.getState();
    return (id: string) => listed.has(id) && statusOf(state, id) !== "done";
  }, [sessions, statusMarks]);
  const review = useMemo(
    () => groupReview(reviewItems(loops, open, reviewed, now)),
    [loops, open, reviewed, now],
  );

  const markReviewed = (runIds: readonly string[]) => {
    const next = new Set(reviewed);
    for (const id of runIds) next.add(id);
    setReviewed(next);
    saveReviewed(next);
  };

  const confirmDelete = (loop: Loop, onDeleted?: () => void) => {
    askConfirm({
      title: `Delete “${loop.name}”?`,
      message: "It stops at once, and its run history goes with it. The threads it started stay.",
      action: "Delete",
      destructive: true,
      run: async () => {
        await deleteLoop(loop.id, loop.host);
        toast("Job deleted", "success");
        setNow(Date.now());
        onDeleted?.();
        void reload();
      },
    });
  };

  const startDaemon = async (host: string) => {
    setStartingHost(host);
    try {
      await startScheduler(host);
      toast(host ? `The scheduler is running on ${hostLabel(host)}` : "The scheduler is running", "success");
      await reload();
    } catch (e) {
      toast(remoteLoopError(e, hostLabel(host)), "error");
    } finally {
      setStartingHost(null);
    }
  };

  const openNew = () => {
    setEditing(null);
    setDuplicating(false);
    setDialogOpen(true);
  };

  const live =
    editing && !duplicating ? allLoops.find((loop) => loopKey(loop) === loopKey(editing)) : undefined;

  const daemonRunningByHost = useMemo(() => {
    const map: Record<string, boolean> = {};
    if (byHost) {
      for (const [host, state] of Object.entries(byHost)) {
        if (!state.unsupported && !state.error) map[host] = state.daemonRunning;
      }
    }
    return map;
  }, [byHost]);

  const remoteHubMembers = useMemo(() => {
    const map: Record<string, LoopHubMember[]> = {};
    if (byHost) {
      for (const [host, state] of Object.entries(byHost)) {
        if (host && state.hubMembers?.length) map[host] = state.hubMembers;
      }
    }
    return map;
  }, [byHost]);

  const unsupportedHosts = useMemo(() => {
    const set = new Set<string>();
    if (byHost) {
      for (const [host, state] of Object.entries(byHost)) {
        if (state.unsupported) set.add(host);
      }
    }
    return set;
  }, [byHost]);

  const scopeMachine =
    effectiveScope !== ALL_MACHINES && effectiveScope !== LOCAL_HOST
      ? machines.find((m) => m.id === effectiveScope)
      : undefined;
  const scopeOffline = Boolean(scopeMachine && scopeMachine.state !== "connected");
  const scopeState = effectiveScope === ALL_MACHINES ? undefined : byHost?.[effectiveScope];

  // Connected machines in the active scope whose scheduler is stopped (excluding older unsupported hosts).
  const stoppedHosts = useMemo(() => {
    if (!byHost) return [];
    const list = effectiveScope === ALL_MACHINES ? Object.values(byHost) : scopeState ? [scopeState] : [];
    return list.filter((entry) => !entry.daemonRunning && !entry.unsupported && !entry.error);
  }, [byHost, effectiveScope, scopeState]);

  const anyDaemonRunning = Boolean(byHost && Object.values(byHost).some((entry) => entry.daemonRunning));
  const showMachineTag = hasPeers && effectiveScope === ALL_MACHINES;
  const defaultDialogHost =
    effectiveScope !== ALL_MACHINES && !scopeOffline && !unsupportedHosts.has(effectiveScope)
      ? effectiveScope
      : LOCAL_HOST;

  const filterOptions = useMemo(() => {
    if (!hasPeers) return [];
    return [
      { value: ALL_MACHINES, label: "All machines" },
      { value: LOCAL_HOST, label: `${localName} (this computer)` },
      ...machines.map((machine) => ({
        value: machine.id,
        label: machine.state === "connected" ? machineName(machine) : `${machineName(machine)} (offline)`,
      })),
    ];
  }, [hasPeers, localName, machines]);

  const showToolbar = Boolean(byHost && (hasPeers || (anyDaemonRunning && loops.length > 0)));
  const canCreate = Boolean(byHost && (anyDaemonRunning || hasPeers));

  return (
    <div className="flex min-h-0 flex-1 flex-col bg-bg">
      <ViewHeader title="Jobs" />
      <div ref={listRef} className="min-h-0 flex-1 overflow-y-auto">
        <div className="mx-auto flex w-full max-w-5xl flex-col gap-3 px-6 pt-2 pb-8">
          {showToolbar && (
            <div className="flex flex-wrap items-center gap-2 pb-1">
              {(loops.length > 0 || tab === "review") && (
                <Segments
                  label="Show"
                  value={tab}
                  options={[
                    { value: "jobs", label: "Jobs" },
                    { value: "review", label: review.length ? `Review ${review.length}` : "Review" },
                  ]}
                  onChange={setTab}
                />
              )}
              {hasPeers && (
                <Select
                  label="Machine"
                  value={effectiveScope}
                  options={filterOptions}
                  onChange={setScopeHost}
                />
              )}
              <span className="flex-1" />
              {tab === "review" && review.length > 0 && (
                <Pill quiet onClick={() => markReviewed(review.flatMap((group) => group.runIds))}>
                  <CheckCheck size={13} />
                  Mark all reviewed
                </Pill>
              )}
              {canCreate && (
                <Pill prominent onClick={openNew}>
                  <Plus size={13} />
                  New job
                </Pill>
              )}
            </div>
          )}
          {!byHost ? (
            <Loading error={error} onRetry={() => void reload()} />
          ) : scopeOffline && scopeMachine ? (
            <Callout tone="warning">
              {machineName(scopeMachine)} is offline. Connect to it to view or edit its jobs.
            </Callout>
          ) : scopeState?.unsupported && scopeMachine ? (
            <Callout tone="warning">
              Update RustCode on {machineName(scopeMachine)} to view and manage its jobs from here.
            </Callout>
          ) : (
            <>
              {stoppedHosts.map((entry) => {
                const name = entry.host ? hostLabel(entry.host) : null;
                return (
                  <Callout
                    key={entry.host || "local"}
                    tone="warning"
                    action={
                      <Pill
                        size="sm"
                        disabled={startingHost === entry.host}
                        onClick={() => void startDaemon(entry.host)}
                      >
                        {startingHost === entry.host ? <Spinner size={12} /> : null}
                        Start scheduler
                      </Pill>
                    }
                  >
                    {name ? (
                      <>
                        The scheduler on <span className="font-medium">{name}</span> is stopped, so its jobs
                        cannot be listed or run.
                      </>
                    ) : (
                      <>
                        The scheduler is stopped, so jobs cannot be listed or run. It runs in the background,
                        apart from the app, as <span className="font-mono">rc daemon start</span> does.
                      </>
                    )}
                  </Callout>
                );
              })}
              {loops.length === 0 ? (
                (anyDaemonRunning || hasPeers) && (
                  <div className="flex flex-col items-center gap-3 py-16 text-center">
                    <span className="inline-flex size-10 items-center justify-center rounded-full bg-fill text-fg-muted">
                      <Repeat size={18} />
                    </span>
                    <h2 className="font-semibold text-title3 text-fg">
                      {effectiveScope === ALL_MACHINES
                        ? "No jobs yet"
                        : `No jobs on ${hostLabel(effectiveScope)}`}
                    </h2>
                    <p className="max-w-sm text-callout text-fg-muted">
                      A job runs a prompt once, on a schedule, manually, or from a webhook—like “every 4
                      hours, check CI on main”—each run in a fresh thread that stays out of your thread list.
                    </p>
                    <Pill prominent onClick={openNew}>
                      New job
                    </Pill>
                  </div>
                )
              ) : tab === "review" ? (
                <JobReview groups={review} now={now} showMachine={showMachineTag} onReviewed={markReviewed} />
              ) : (
                groups.map((group) => (
                  <section key={group.title} aria-label={group.title} className="flex flex-col gap-2 pb-3">
                    {groups.length > 1 && (
                      <h2
                        className={cx(
                          "px-1 font-medium text-caption",
                          group.loops === attention ? "text-danger" : "text-fg-muted",
                        )}
                      >
                        {group.title} · {group.loops.length}
                      </h2>
                    )}
                    <ul className="grid grid-cols-[repeat(auto-fill,minmax(min(100%,300px),1fr))] gap-3">
                      {group.loops.map((loop) => {
                        const loopHarnesses = loop.host
                          ? (remoteHarnesses[loop.host] ?? harnesses)
                          : harnesses;
                        return (
                          <JobCard
                            key={loopKey(loop)}
                            loop={loop}
                            now={now}
                            agent={jobAgent(loop, loopHarnesses, signedOut)}
                            machineLabel={
                              showMachineTag || loop.machine
                                ? loopMachineLabel(loop, machines, localName)
                                : undefined
                            }
                            onOpen={() => openJob(loop)}
                            onDuplicate={() => openJob(loop, true)}
                            onDelete={() => confirmDelete(loop)}
                            act={act}
                          />
                        );
                      })}
                    </ul>
                  </section>
                ))
              )}
            </>
          )}
        </div>
      </div>
      <LoopDialog
        open={dialogOpen}
        loop={editing}
        duplicate={duplicating}
        defaultHost={defaultDialogHost}
        daemonRunningByHost={daemonRunningByHost}
        remoteHubMembers={remoteHubMembers}
        unsupportedHosts={unsupportedHosts}
        runs={live?.runs}
        running={live ? loopState(live) === "running" : false}
        onOpenChange={setDialogOpen}
        onDelete={
          editing && !duplicating
            ? () =>
                confirmDelete(editing, () => {
                  setDialogOpen(false);
                })
            : undefined
        }
        onSaved={() => {
          toast(editing && !duplicating ? "Job saved" : "Job created", "success");
          void reload();
        }}
      />
    </div>
  );
}
