// Scheduled Jobs (issues 3892, 3990; "Loops" in code): prompts the scheduler daemon runs once or on
// a schedule, each run in a fresh thread (docs/daemon.md › Scheduled Jobs). Reached from the
// sidebar's Scheduled Jobs row. The list says when each loop runs,
// when next and how its last run went, with pause/resume, run now, edit and delete; a row opens its
// prompt and run history. Review lists what loops did on their own that is still worth a look.

import { DropdownMenu } from "radix-ui";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { type HarnessSnapshot, Method, projectName } from "../acp/types";
import {
  agoText,
  deleteLoop,
  type Loop,
  lastResult,
  loadReviewed,
  loopState,
  nextRunText,
  parseLoops,
  pauseReason,
  REVIEW_DAYS,
  type ReviewItem,
  rememberLoops,
  reviewItems,
  runLoopNow,
  saveReviewed,
  scheduleInWords,
  setLoopPaused,
  startScheduler,
  useLoopThreads,
} from "../state/loops";
import { askConfirm, errorText, toast, useApp } from "../state/store";
import { selectThread } from "../state/threadOpen";
import { statusOf } from "../state/threadStatus";
import { setView } from "../state/viewActions";
import { ViewHeader } from "./Board";
import {
  ArrowUpRight,
  ChevronRight,
  Ellipsis,
  Play,
  Plus,
  Repeat,
  SquarePen,
  Trash2,
  Webhook,
} from "./icons";
import { LoopDialog } from "./LoopDialog";
import {
  Callout,
  cardInset,
  cx,
  MenuContent,
  menuItem,
  menuItemDanger,
  menuSeparator,
  Pill,
  Segments,
  Spinner,
  Switch,
  Tooltip,
} from "./primitives";
import { Loading, useEngineData } from "./settingsKit";

/** How often the open page asks again, so a run going or ending shows without a click. */
const REFRESH_MS = 30_000;

type Tab = "loops" | "review";

/** A loop that is not paused and has a webhook: a call can start it at any time. */
const listening = (loop: Loop) => !loop.paused && loop.webhook;

const STATE_BADGE: Record<ReturnType<typeof loopState>, { label: string; tint: string }> = {
  running: { label: "Running", tint: "bg-accent" },
  failing: { label: "Paused", tint: "bg-danger" },
  paused: { label: "Paused", tint: "bg-fg-faint" },
  active: { label: "Scheduled", tint: "bg-success" },
};

/** Open the Scheduled Jobs view on one job, expanded: the way back from one of its runs. */
export function showJob(id: string): void {
  useLoopThreads.setState({ reveal: id });
  setView({ kind: "loops" });
}

/** The thread a run went to on this computer; a hub Loop's thread is on its member. */
const runThread = (run: { machine?: string; threadId?: string } | undefined) =>
  run && !run.machine ? run.threadId : undefined;

const TONE: Record<"ok" | "bad" | "muted", string> = {
  ok: "text-fg-muted",
  bad: "text-danger",
  muted: "text-fg-faint",
};

function IconAction({
  label,
  onClick,
  children,
  disabled,
}: {
  label: string;
  onClick: () => void;
  children: React.ReactNode;
  disabled?: boolean;
}) {
  return (
    <Tooltip label={label}>
      <button
        type="button"
        aria-label={label}
        disabled={disabled}
        onClick={(e) => {
          e.stopPropagation();
          onClick();
        }}
        className="inline-flex size-7 items-center justify-center rounded-md text-fg-muted hover:bg-fill hover:text-fg disabled:opacity-40"
      >
        {children}
      </button>
    </Tooltip>
  );
}

const NEXT_RUN_FORMAT = new Intl.DateTimeFormat(undefined, {
  month: "short",
  day: "numeric",
  hour: "numeric",
  minute: "2-digit",
});
const NEXT_RUN_TIME = new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit" });
const NEXT_RUN_WEEKDAY = new Intl.DateTimeFormat(undefined, {
  weekday: "short",
  hour: "numeric",
  minute: "2-digit",
});
const WEEK_MS = 6 * 86_400_000;

/** "In 20 min · 9:59 AM" today, "In 20 h · Thu 5:44 AM" this week, then the date: how long, then when. */
function nextRunLine(loop: Loop, next: number, now: number): string {
  const left = nextRunText(loop, now);
  const when =
    new Date(next).toDateString() === new Date(now).toDateString()
      ? NEXT_RUN_TIME
      : next - now < WEEK_MS
        ? NEXT_RUN_WEEKDAY
        : NEXT_RUN_FORMAT;
  return `${left[0]?.toUpperCase()}${left.slice(1)} · ${when.format(next)}`;
}

function loopAgent(loop: Loop, harnesses: HarnessSnapshot | null): string {
  if (loop.threadId) return "Existing thread";
  const harness = harnesses?.harnesses.find(
    (item) => item.id === (loop.harness || harnesses.default.harness),
  );
  const account = harness?.accounts?.find((item) => item.id === loop.account);
  const model = harness?.models.find((item) => item.id === loop.model);
  return [
    harness?.name ?? loop.harness,
    account?.name ?? loop.account,
    model?.name ?? loop.model,
    loop.effort && `${loop.effort} effort`,
  ]
    .filter(Boolean)
    .join(" · ");
}

function LoopRow({
  loop,
  now,
  expanded,
  agent,
  onToggle,
  onEdit,
  act,
}: {
  loop: Loop;
  now: number;
  expanded: boolean;
  agent: string;
  onToggle: () => void;
  onEdit: () => void;
  act: (what: string, run: () => Promise<void>) => void;
}) {
  const state = loopState(loop);
  const badge = STATE_BADGE[state];
  const result = lastResult(loop, now);
  const reason = pauseReason(loop);
  const running = state === "running";
  // The latest run's thread, opened from the row like any thread.
  const lastThread = running ? undefined : runThread(loop.runs[0]);
  const next = !loop.paused && loop.nextDueAt !== undefined ? loop.nextDueAt : undefined;
  // A loop with no schedule but a webhook is waiting for a call, not unscheduled.
  const waiting = state === "active" && next === undefined && listening(loop);
  const badgeLabel = waiting
    ? "On webhook"
    : state === "active" && next === undefined
      ? loop.schedule.kind === "once"
        ? "Completed"
        : "Unscheduled"
      : badge.label;
  return (
    <li data-loop={loop.id} className="flex flex-col rounded-lg bg-surface">
      <div className="flex flex-wrap items-center gap-x-2 px-3 py-2">
        <button
          type="button"
          aria-expanded={expanded}
          onClick={onToggle}
          className="flex min-w-[min(100%,210px)] flex-1 items-center gap-2 rounded-md px-2 py-1.5 text-left hover:bg-fill-subtle"
        >
          <ChevronRight
            size={12}
            className={cx("shrink-0 text-fg-faint transition-transform", expanded && "rotate-90")}
          />
          <span className="flex min-w-0 flex-1 flex-col gap-0.5">
            <span className="flex min-w-0 flex-wrap items-center gap-x-2">
              <span className="truncate font-medium text-body text-fg">{loop.name}</span>
              <span className="inline-flex shrink-0 items-center gap-1.5 text-caption text-fg-muted">
                <span
                  aria-hidden
                  className={cx(
                    "size-1.5 rounded-full",
                    next === undefined && state === "active" && !waiting ? "bg-fg-faint" : badge.tint,
                  )}
                />
                {badgeLabel}
              </span>
            </span>
            <span className="truncate text-caption text-fg-muted">
              {projectName(loop.workspace)}
              {loop.machine && ` on ${loop.machine}`} · {scheduleInWords(loop.schedule)}
              {loop.webhook && loop.schedule.kind !== "manual" && (
                <span className="ml-2 inline-flex items-center gap-1 align-[-2px] text-fg-faint">
                  <Webhook size={12} />
                  Webhook
                </span>
              )}
            </span>
          </span>
        </button>
        <div className="min-w-0 basis-44 px-2 py-1">
          {next !== undefined && (
            <time
              dateTime={new Date(next).toISOString()}
              title={NEXT_RUN_FORMAT.format(next)}
              className="block truncate font-medium text-callout text-fg"
            >
              {nextRunLine(loop, next, now)}
            </time>
          )}
          {lastThread ? (
            <Tooltip label="Open the last run">
              <button
                type="button"
                onClick={() => selectThread(lastThread)}
                className={cx(
                  "group/last flex max-w-full items-center gap-1 rounded text-left text-caption hover:text-fg",
                  TONE[result.tone],
                )}
              >
                <span className="truncate underline-offset-2 group-hover/last:underline">{result.text}</span>
                <ArrowUpRight size={12} className="shrink-0 opacity-60 group-hover/last:opacity-100" />
              </button>
            </Tooltip>
          ) : (
            <span title={result.text} className={cx("block truncate text-caption", TONE[result.tone])}>
              {result.text}
            </span>
          )}
        </div>
        <div className="flex shrink-0 items-center gap-1">
          {running && <Spinner size={12} className="mr-1 text-fg-muted" />}
          <IconAction
            label={running ? "Edit it once the run ends" : "Edit"}
            disabled={running}
            onClick={onEdit}
          >
            <SquarePen size={14} />
          </IconAction>
          <IconAction
            label={loop.paused ? "Resume it to run now" : "Run now"}
            disabled={running || loop.paused}
            onClick={() => act("Started a run", () => runLoopNow(loop.id))}
          >
            <Play size={14} />
          </IconAction>
          <Tooltip label={loop.paused ? "Paused" : "On"}>
            <span className="inline-flex px-1">
              <Switch
                label={loop.paused ? `Resume ${loop.name}` : `Pause ${loop.name}`}
                checked={!loop.paused}
                onChange={(on) => act(on ? "Resumed" : "Paused", () => setLoopPaused(loop.id, !on))}
              />
            </span>
          </Tooltip>
          <DropdownMenu.Root modal={false}>
            <DropdownMenu.Trigger asChild>
              <button
                type="button"
                aria-label={`More for ${loop.name}`}
                className="inline-flex size-7 items-center justify-center rounded-md text-fg-muted hover:bg-fill hover:text-fg data-[state=open]:bg-fill"
              >
                <Ellipsis size={14} />
              </button>
            </DropdownMenu.Trigger>
            <MenuContent align="end">
              {lastThread && (
                <>
                  <DropdownMenu.Item className={menuItem} onSelect={() => selectThread(lastThread)}>
                    <ArrowUpRight size={13} className="text-fg-muted" /> Open last run
                  </DropdownMenu.Item>
                  <DropdownMenu.Separator className={menuSeparator} />
                </>
              )}
              <DropdownMenu.Item
                className={cx(menuItem, menuItemDanger)}
                onSelect={() =>
                  askConfirm({
                    title: `Delete “${loop.name}”?`,
                    message:
                      "It stops at once, and its run history goes with it. The threads it started stay.",
                    action: "Delete",
                    destructive: true,
                    run: () => act("Deleted", () => deleteLoop(loop.id)),
                  })
                }
              >
                <Trash2 size={13} /> Delete
              </DropdownMenu.Item>
            </MenuContent>
          </DropdownMenu.Root>
        </div>
      </div>
      {reason && (
        <Callout className="mx-4 mb-3 bg-bg">
          {/* A signed-out login's reason already says how to sign in. */}
          {loop.runs[0]?.signedOut ? reason : `${reason}. Fix the cause, then resume it.`}
        </Callout>
      )}
      {expanded && <LoopDetail loop={loop} now={now} agent={agent} />}
    </li>
  );
}

function LoopDetail({ loop, now, agent }: { loop: Loop; now: number; agent: string }) {
  return (
    <div className="mx-5 flex flex-col gap-3 border-t border-border py-3">
      <p className="selectable whitespace-pre-wrap break-words rounded-md bg-fill-subtle px-3 py-2 text-callout text-fg">
        {loop.prompt}
      </p>
      {loop.script && (
        <div className="flex flex-col gap-1">
          <span className="font-medium text-caption text-fg-muted">Script, run first</span>
          <pre className="selectable overflow-x-auto whitespace-pre-wrap break-words rounded-md bg-fill-subtle px-3 py-2 font-mono text-caption text-fg">
            {loop.script}
          </pre>
        </div>
      )}
      <p className="text-caption text-fg-muted">
        {agent} · {loop.readOnly ? "Read-only" : "Can edit, commit and push"}
        {loop.schedule.kind === "cron" && loop.schedule.timezone ? ` · ${loop.schedule.timezone}` : ""}
      </p>
      {loop.runs.length > 0 && (
        <ol aria-label="Recent runs" className="flex flex-col border-t border-border pt-2">
          {loop.runs.map((run) => {
            const at = run.finishedAt ?? run.startedAt ?? run.scheduledAt;
            const text =
              run.state === "succeeded"
                ? run.silent
                  ? "The script printed nothing"
                  : run.queued
                    ? `Waiting for ${run.machine ?? "its machine"}, which is offline`
                    : run.machine
                      ? `Started on ${run.machine}`
                      : run.quiet
                        ? "Nothing to report"
                        : "Finished"
                : run.state === "running" || run.state === "claimed"
                  ? "Running"
                  : run.state === "cancelled"
                    ? run.skipped
                      ? `Skipped: ${run.skipped}`
                      : "Stopped"
                    : `Failed${run.error ? `: ${run.error}` : ""}`;
            const threadId = runThread(run);
            const body = (
              <>
                <span className="w-20 shrink-0 text-fg-faint">{agoText(at, now)}</span>
                <span
                  className={cx(
                    "min-w-0 flex-1 break-words",
                    run.state === "failed" || run.state === "ambiguous" ? "text-danger" : "text-fg-muted",
                  )}
                >
                  {text}
                </span>
              </>
            );
            // A run with a thread opens it from anywhere on its line, like a thread in the list.
            return (
              <li key={run.id} className="flex min-w-0">
                {threadId ? (
                  <button
                    type="button"
                    onClick={() => selectThread(threadId)}
                    className="group/run -mx-2 flex min-w-0 flex-1 items-center gap-3 rounded-md px-2 py-1 text-left text-caption hover:bg-fill-subtle"
                  >
                    {body}
                    <span className="flex shrink-0 items-center gap-1 text-fg-faint group-hover/run:text-fg">
                      Open <ArrowUpRight size={12} />
                    </span>
                  </button>
                ) : (
                  <div className="flex min-w-0 flex-1 items-center gap-3 py-1 text-caption">{body}</div>
                )}
              </li>
            );
          })}
        </ol>
      )}
    </div>
  );
}

function ReviewList({
  items,
  now,
  onReviewed,
}: {
  items: ReviewItem[];
  now: number;
  onReviewed: (runId: string) => void;
}) {
  if (!items.length) {
    return (
      <p className="py-8 text-center text-callout text-fg-muted">
        Nothing to review. Failed runs and threads scheduled jobs left open in the last {REVIEW_DAYS} days
        show here.
      </p>
    );
  }
  return (
    <ul className="flex flex-col gap-2">
      {items.map(({ loop, run, kind }) => {
        const at = run.finishedAt ?? run.startedAt ?? run.scheduledAt;
        const threadId = run.machine ? undefined : run.threadId;
        return (
          <li key={run.id} className={cx("flex items-center gap-3 rounded-lg bg-surface", cardInset)}>
            <div className="flex min-w-0 flex-1 flex-col gap-0.5">
              <span className="truncate font-medium text-callout text-fg">{loop.name}</span>
              <span
                className={cx("truncate text-caption", kind === "failed" ? "text-danger" : "text-fg-muted")}
              >
                {kind === "failed"
                  ? `Failed ${agoText(at, now)}${run.error ? `: ${run.error}` : ""}`
                  : `Its thread is open · ${agoText(at, now)}`}
              </span>
            </div>
            {threadId && (
              <Pill size="sm" onClick={() => selectThread(threadId)}>
                Open thread
              </Pill>
            )}
            <Pill size="sm" quiet onClick={() => onReviewed(run.id)}>
              Mark reviewed
            </Pill>
          </li>
        );
      })}
    </ul>
  );
}

export function Loops() {
  const { data, error, reload } = useEngineData(Method.loopsList, parseLoops);
  const sessions = useApp((s) => s.sessions);
  const harnesses = useApp((s) => s.harnesses);
  const statusMarks = useApp((s) => s.statusMarks);
  const [tab, setTab] = useState<Tab>("loops");
  const [expanded, setExpanded] = useState<string | null>(null);
  const [editing, setEditing] = useState<Loop | null>(null);
  const [dialogOpen, setDialogOpen] = useState(false);
  const [starting, setStarting] = useState(false);
  const [reviewed, setReviewed] = useState(loadReviewed);
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    const timer = setInterval(() => {
      setNow(Date.now());
      void reload();
    }, REFRESH_MS);
    return () => clearInterval(timer);
  }, [reload]);

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

  // Opened from one of a job's runs: that job is shown expanded and scrolled to, once.
  const reveal = useLoopThreads((s) => s.reveal);
  const listRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!reveal || !data?.loops.some((loop) => loop.id === reveal)) return;
    setTab("loops");
    setExpanded(reveal);
    useLoopThreads.setState({ reveal: null });
    requestAnimationFrame(() =>
      listRef.current
        ?.querySelector(`[data-loop="${CSS.escape(reveal)}"]`)
        ?.scrollIntoView({ block: "nearest" }),
    );
  }, [reveal, data]);

  const loops = data?.loops ?? [];
  const upcoming: Loop[] = [];
  const inactive: Loop[] = [];
  for (const loop of loops) {
    if (loopState(loop) === "running" || (!loop.paused && loop.nextDueAt !== undefined) || listening(loop))
      upcoming.push(loop);
    else inactive.push(loop);
  }
  // Loops that only wait for their webhook go after the ones with a time.
  upcoming.sort(
    (a, b) => (a.nextDueAt ?? Infinity) - (b.nextDueAt ?? Infinity) || a.name.localeCompare(b.name),
  );
  inactive.sort((a, b) => Number(b.paused) - Number(a.paused) || a.name.localeCompare(b.name));
  // Open: still in the list and not archived. `statusMarks` changes when the user archives one.
  // biome-ignore lint/correctness/useExhaustiveDependencies: statusMarks is what statusOf reads
  const open = useMemo(() => {
    const listed = new Set(sessions.map((session) => session.id));
    const state = useApp.getState();
    return (id: string) => listed.has(id) && statusOf(state, id) !== "done";
  }, [sessions, statusMarks]);
  const review = useMemo(() => reviewItems(loops, open, reviewed, now), [loops, open, reviewed, now]);

  const markReviewed = (runId: string) => {
    const next = new Set(reviewed);
    next.add(runId);
    setReviewed(next);
    saveReviewed(next);
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
    setDialogOpen(true);
  };

  return (
    <div className="flex min-h-0 flex-1 flex-col bg-bg">
      <ViewHeader title="Scheduled Jobs">
        <div className="flex items-center gap-2">
          {loops.length > 0 && (
            <Segments
              label="Show"
              value={tab}
              options={[
                { value: "loops", label: "Jobs" },
                { value: "review", label: review.length ? `Review ${review.length}` : "Review" },
              ]}
              onChange={setTab}
            />
          )}
          {data?.daemonRunning && loops.length > 0 && (
            <Pill prominent onClick={openNew}>
              <Plus size={13} />
              New job
            </Pill>
          )}
        </div>
      </ViewHeader>
      <div ref={listRef} className="min-h-0 flex-1 overflow-y-auto">
        <div className="mx-auto flex w-full max-w-3xl flex-col gap-3 px-6 pt-2 pb-8">
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
              The scheduler is stopped, so scheduled jobs cannot be listed or run. It runs in the background,
              apart from the app, as <span className="font-mono">rustcode daemon start</span> does.
            </Callout>
          ) : loops.length === 0 ? (
            <div className="flex flex-col items-center gap-3 py-16 text-center">
              <span className="inline-flex size-10 items-center justify-center rounded-full bg-fill text-fg-muted">
                <Repeat size={18} />
              </span>
              <h2 className="font-semibold text-title3 text-fg">No scheduled jobs yet</h2>
              <p className="max-w-sm text-callout text-fg-muted">
                A scheduled job runs a prompt once or on a schedule, like “every 4 hours, check CI on main”,
                each run in a fresh thread. Runs are read-only unless you say otherwise.
              </p>
              <Pill prominent onClick={openNew}>
                New job
              </Pill>
            </div>
          ) : tab === "review" ? (
            <ReviewList items={review} now={now} onReviewed={markReviewed} />
          ) : (
            [
              { title: "Upcoming", loops: upcoming },
              { title: "Inactive", loops: inactive },
            ].map(
              (group) =>
                group.loops.length > 0 && (
                  <section key={group.title} aria-label={group.title} className="flex flex-col gap-2 pb-3">
                    <h2 className="px-1 font-medium text-caption text-fg-muted">{group.title}</h2>
                    <ul className="flex flex-col gap-2">
                      {group.loops.map((loop) => (
                        <LoopRow
                          key={loop.id}
                          loop={loop}
                          now={now}
                          agent={loopAgent(loop, harnesses)}
                          expanded={expanded === loop.id}
                          onToggle={() => setExpanded((id) => (id === loop.id ? null : loop.id))}
                          onEdit={() => {
                            setEditing(loop);
                            setDialogOpen(true);
                          }}
                          act={act}
                        />
                      ))}
                    </ul>
                  </section>
                ),
            )
          )}
        </div>
      </div>
      <LoopDialog
        open={dialogOpen}
        loop={editing}
        onOpenChange={setDialogOpen}
        onSaved={() => {
          toast(editing ? "Job saved" : "Job scheduled", "success");
          void reload();
        }}
      />
    </div>
  );
}
