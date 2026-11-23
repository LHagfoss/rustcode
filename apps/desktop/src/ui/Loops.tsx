// Jobs (issues 3892, 3937): prompts the scheduler daemon runs once, on a schedule, manually, or
// from a webhook, each run in a fresh thread or one chosen thread (docs/daemon.md › Jobs). Reached
// from the sidebar's Jobs row. Each job is a card with its agent, last result, schedule and next run;
// the card opens one dialog that edits the job beside its runs. Jobs paused by failures lead. Review
// is a timeline by day of failed runs and open threads, a repeated failure folded into one row.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { type HarnessSnapshot, Method } from "../acp/types";
import { catalogModelTitle } from "../lib/modelNames";
import { lastAccountFor } from "../state/harnessActions";
import { newThreadAccountOn } from "../state/logins";
import {
  deleteLoop,
  groupReview,
  type Loop,
  loadReviewed,
  loopState,
  parseLoops,
  rememberLoops,
  reviewItems,
  saveReviewed,
  startScheduler,
  useLoopThreads,
} from "../state/loops";
import { askConfirm, errorText, toast, useApp } from "../state/store";
import { statusOf } from "../state/threadStatus";
import { CheckCheck, Plus, Repeat } from "./icons";
import { JobCard, listening } from "./JobCard";
import { JobReview } from "./JobReview";
import { showJob } from "./JobRunBar";
import { LoopDialog } from "./LoopDialog";
import { ViewHeader } from "./MainHeader";
import { Callout, cx, Pill, Segments, Spinner } from "./primitives";
import { Loading, useEngineData } from "./settingsKit";

/** Poll quickly while a run is active; rest at 30s when idle. */
const IDLE_REFRESH_MS = 30_000;
const RUNNING_REFRESH_MS = 5_000;

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
    (!loop.machine && harness ? newThreadAccountOn(harness, lastAccountFor(harness.id), signedOut) : null) ||
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
  const { data, error, reload } = useEngineData(Method.loopsList, parseLoops);
  const sessions = useApp((s) => s.sessions);
  const harnesses = useApp((s) => s.harnesses);
  const signedOut = useApp((s) => s.signedOutLogins);
  const statusMarks = useApp((s) => s.statusMarks);
  const [tab, setTab] = useState<Tab>("jobs");
  const [editing, setEditing] = useState<Loop | null>(null);
  const [duplicating, setDuplicating] = useState(false);
  const [dialogOpen, setDialogOpen] = useState(false);
  const [starting, setStarting] = useState(false);
  const [reviewed, setReviewed] = useState(loadReviewed);
  const [now, setNow] = useState(() => Date.now());

  const loops = data?.loops ?? [];
  const hasRunning = loops.some((loop) => loopState(loop) === "running");

  useEffect(() => {
    const interval = hasRunning ? RUNNING_REFRESH_MS : IDLE_REFRESH_MS;
    const timer = setInterval(() => {
      setNow(Date.now());
      void reload();
    }, interval);
    return () => clearInterval(timer);
  }, [reload, hasRunning]);

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

  useEffect(() => {
    if (data) rememberLoops(data);
  }, [data]);

  const openJob = useCallback((loop: Loop, duplicate = false) => {
    setEditing(loop);
    setDuplicating(duplicate);
    setDialogOpen(true);
  }, []);

  /** Opened from one of a job's runs: that job's dialog, matched by id or, for old runs, by name. */
  const reveal = useLoopThreads((s) => s.reveal);
  const listRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!reveal || !data) return;
    const match = data.loops.find((loop) => loop.id === reveal || loop.name === reveal);
    if (!match) return;
    setTab("jobs");
    useLoopThreads.setState({ reveal: null });
    openJob(match);
    requestAnimationFrame(() =>
      listRef.current
        ?.querySelector(`[data-loop="${CSS.escape(match.id)}"]`)
        ?.scrollIntoView({ block: "nearest" }),
    );
  }, [reveal, data, openJob]);

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
        await deleteLoop(loop.id);
        toast("Job deleted", "success");
        setNow(Date.now());
        onDeleted?.();
        void reload();
      },
    });
  };

  const startDaemon = async () => {
    setStarting(true);
    try {
      await startScheduler();
      toast("The scheduler is running", "success");
      await reload();
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setStarting(false);
    }
  };

  const openNew = () => {
    setEditing(null);
    setDuplicating(false);
    setDialogOpen(true);
  };

  const live = editing && !duplicating ? loops.find((loop) => loop.id === editing.id) : undefined;

  return (
    <div className="flex min-h-0 flex-1 flex-col bg-bg">
      <ViewHeader title="Jobs" />
      <div ref={listRef} className="min-h-0 flex-1 overflow-y-auto">
        <div className="mx-auto flex w-full max-w-5xl flex-col gap-3 px-6 pt-2 pb-8">
          {data?.daemonRunning && loops.length > 0 && (
            <div className="flex items-center gap-2 pb-1">
              <Segments
                label="Show"
                value={tab}
                options={[
                  { value: "jobs", label: "Jobs" },
                  { value: "review", label: review.length ? `Review ${review.length}` : "Review" },
                ]}
                onChange={setTab}
              />
              <span className="flex-1" />
              {tab === "review" && review.length > 0 && (
                <Pill quiet onClick={() => markReviewed(review.flatMap((group) => group.runIds))}>
                  <CheckCheck size={13} />
                  Mark all reviewed
                </Pill>
              )}
              <Pill prominent onClick={openNew}>
                <Plus size={13} />
                New job
              </Pill>
            </div>
          )}
          {!data ? (
            <Loading error={error} onRetry={() => void reload()} />
          ) : !data.daemonRunning ? (
            <Callout
              tone="warning"
              action={
                <Pill size="sm" disabled={starting} onClick={() => void startDaemon()}>
                  {starting ? <Spinner size={12} /> : null}
                  Start scheduler
                </Pill>
              }
            >
              The scheduler is stopped, so jobs cannot be listed or run. It runs in the background, apart from
              the app, as <span className="font-mono">rc daemon start</span> does.
            </Callout>
          ) : loops.length === 0 ? (
            <div className="flex flex-col items-center gap-3 py-16 text-center">
              <span className="inline-flex size-10 items-center justify-center rounded-full bg-fill text-fg-muted">
                <Repeat size={18} />
              </span>
              <h2 className="font-semibold text-title3 text-fg">No jobs yet</h2>
              <p className="max-w-sm text-callout text-fg-muted">
                A job runs a prompt once, on a schedule, manually, or from a webhook—like “every 4 hours,
                check CI on main”—each run in a fresh thread that stays out of your thread list.
              </p>
              <Pill prominent onClick={openNew}>
                New job
              </Pill>
            </div>
          ) : tab === "review" ? (
            <JobReview groups={review} now={now} onReviewed={markReviewed} />
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
                  {group.loops.map((loop) => (
                    <JobCard
                      key={loop.id}
                      loop={loop}
                      now={now}
                      agent={jobAgent(loop, harnesses, signedOut)}
                      onOpen={() => openJob(loop)}
                      onDuplicate={() => openJob(loop, true)}
                      onDelete={() => confirmDelete(loop)}
                      act={act}
                    />
                  ))}
                </ul>
              </section>
            ))
          )}
        </div>
      </div>
      <LoopDialog
        open={dialogOpen}
        loop={editing}
        duplicate={duplicating}
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
