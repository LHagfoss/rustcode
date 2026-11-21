// Jobs (issues 3892, 3937): prompts the scheduler daemon runs once, on a schedule, manually, or
// from a webhook, each run in a fresh thread or one chosen thread (docs/daemon.md › Jobs). Reached
// from the sidebar's Jobs row. Each card puts the job's name, cadence and latest outcome first,
// keeps next-run timing calm underneath, and expands cleanly across the full row to show the
// prompt, agent details and timestamped run history. Review lists failed runs and open threads.

import { DropdownMenu } from "radix-ui";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { type HarnessSnapshot, Method, projectName } from "../acp/types";
import { catalogModelTitle } from "../lib/modelNames";
import { lastAccountFor } from "../state/harnessActions";
import { newThreadAccountOn } from "../state/logins";
import {
  agoText,
  cancelLoopRun,
  deleteLoop,
  type Loop,
  type LoopRun,
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
  runDurationText,
  runLoopNow,
  runTimestampText,
  saveReviewed,
  scheduleInWords,
  setLoopPaused,
  startScheduler,
  useLoopThreads,
} from "../state/loops";
import { askConfirm, errorText, toast, useApp } from "../state/store";
import { selectThread } from "../state/threadOpen";
import { statusOf } from "../state/threadStatus";
import {
  ArrowUpRight,
  ChevronRight,
  Copy,
  Ellipsis,
  Pencil,
  Play,
  Plus,
  Repeat,
  SquareStop,
  Terminal,
  Trash2,
  Webhook,
} from "./icons";
import { showJob } from "./JobRunBar";
import { LoopDialog } from "./LoopDialog";
import { ViewHeader } from "./MainHeader";
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

/** Poll quickly while a run is active; rest at 30s when idle. */
const IDLE_REFRESH_MS = 30_000;
const RUNNING_REFRESH_MS = 5_000;

type Tab = "jobs" | "review";

/** A job that is not paused and has a webhook: a call can start it at any time. */
const listening = (loop: Loop) => !loop.paused && loop.webhook;

export { showJob };

/** The thread a run went to on this computer; a hub job's thread is on its member. */
const runThread = (run: { machine?: string; threadId?: string } | undefined) =>
  run && !run.machine ? run.threadId : undefined;

const OUTCOME_TEXT: Record<"ok" | "bad" | "muted", string> = {
  ok: "text-fg",
  bad: "text-danger",
  muted: "text-fg-muted",
};

function runDotTint(run: LoopRun | undefined): string {
  if (!run) return "bg-fg-faint";
  if (run.state === "running" || run.state === "claimed") return "bg-accent";
  if (run.state === "failed" || run.state === "ambiguous") return "bg-danger";
  if (run.state === "cancelled") return "bg-fg-faint";
  return run.quiet || run.silent ? "bg-fg-faint" : "bg-success";
}

function IconAction({
  label,
  onClick,
  children,
  disabled,
  tone,
}: {
  label: string;
  onClick: () => void;
  children: React.ReactNode;
  disabled?: boolean;
  tone?: "danger";
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
        className={cx(
          "inline-flex size-7 items-center justify-center rounded-md disabled:opacity-40",
          tone === "danger" ? "text-danger hover:bg-danger/12" : "text-fg-muted hover:bg-fill hover:text-fg",
        )}
      >
        {children}
      </button>
    </Tooltip>
  );
}

const NEXT_RUN_FORMAT = new Intl.DateTimeFormat(undefined, {
  month: "short",
  day: "numeric",
  hour: "2-digit",
  minute: "2-digit",
  hour12: false,
});
const NEXT_RUN_TIME = new Intl.DateTimeFormat(undefined, {
  hour: "2-digit",
  minute: "2-digit",
  hour12: false,
});
const NEXT_RUN_WEEKDAY = new Intl.DateTimeFormat(undefined, {
  weekday: "short",
  hour: "2-digit",
  minute: "2-digit",
  hour12: false,
});
const FULL_RUN_FORMAT = new Intl.DateTimeFormat(undefined, {
  dateStyle: "medium",
  timeStyle: "short",
});
const WEEK_MS = 6 * 86_400_000;

/** "In 20 min · 09:59" today, "In 2 d · Mon 08:00" this week, then the date. */
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

function jobAgent(
  loop: Loop,
  harnesses: HarnessSnapshot | null,
  signedOut: Parameters<typeof newThreadAccountOn>[2],
): string {
  if (loop.threadId) return "Existing thread";
  const harness = harnesses?.harnesses.find(
    (item) => item.id === (loop.harness || harnesses.default.harness),
  );
  // Resolve the effective account the daemon will use when the job was saved without one.
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

function StatusChip({
  state,
  waiting,
  completed,
}: {
  state: ReturnType<typeof loopState>;
  waiting: boolean;
  completed: boolean;
}) {
  if (state === "running") {
    return (
      <span className="inline-flex shrink-0 items-center gap-1 rounded-full bg-accent/14 px-2 py-0.5 font-medium text-caption text-accent">
        <Spinner size={10} />
        Running
      </span>
    );
  }
  if (state === "failing") {
    return (
      <span className="inline-flex shrink-0 items-center gap-1 rounded-full bg-danger/12 px-2 py-0.5 font-medium text-caption text-danger">
        Paused · failing
      </span>
    );
  }
  if (state === "paused") {
    return (
      <span className="inline-flex shrink-0 items-center rounded-full bg-fill px-2 py-0.5 text-caption text-fg-muted">
        Paused
      </span>
    );
  }
  if (waiting) {
    return (
      <span className="inline-flex shrink-0 items-center gap-1 rounded-full bg-fill px-2 py-0.5 text-caption text-fg-muted">
        <Webhook size={10} />
        Webhook
      </span>
    );
  }
  if (completed) {
    return (
      <span className="inline-flex shrink-0 items-center rounded-full bg-fill px-2 py-0.5 text-caption text-fg-faint">
        Completed
      </span>
    );
  }
  return null;
}

function JobRow({
  loop,
  now,
  expanded,
  agent,
  onToggle,
  onEdit,
  onDuplicate,
  onDelete,
  act,
}: {
  loop: Loop;
  now: number;
  expanded: boolean;
  agent: string;
  onToggle: () => void;
  onEdit: () => void;
  onDuplicate: () => void;
  onDelete: () => void;
  act: (what: string, run: () => Promise<void>) => void;
}) {
  const state = loopState(loop);
  const result = lastResult(loop, now);
  const reason = pauseReason(loop);
  const running = state === "running";
  const latestRun = loop.runs[0];
  const lastThread = running ? undefined : runThread(latestRun);
  const next = !loop.paused && loop.nextDueAt !== undefined ? loop.nextDueAt : undefined;
  const waiting = state === "active" && next === undefined && listening(loop);
  const completed = state === "active" && next === undefined && !waiting && loop.schedule.kind === "once";
  const quietResult = latestRun?.state === "succeeded" && (latestRun.quiet || latestRun.silent);
  const secondarySchedule =
    next !== undefined
      ? `Next: ${nextRunLine(loop, next, now)}`
      : loop.paused
        ? "Paused"
        : waiting
          ? "Waits for webhook"
          : completed
            ? "Finished one-time run"
            : "Manual only";

  const handleHeaderClick = (e: React.MouseEvent<HTMLDivElement>) => {
    if ((e.target as HTMLElement).closest("[data-row-action]")) return;
    onToggle();
  };

  return (
    <li data-loop={loop.id} className="flex flex-col rounded-lg bg-surface">
      {/* Full-row hover & expand target; action buttons opt out via data-row-action. */}
      {/* biome-ignore lint/a11y/noStaticElementInteractions: expand toggle also lives on the accessible button inside */}
      {/* biome-ignore lint/a11y/useKeyWithClickEvents: keyboard users activate the button inside */}
      <div
        onClick={handleHeaderClick}
        className={cx(
          "flex cursor-pointer flex-wrap items-center gap-x-4 gap-y-2 px-4 py-3 hover:bg-fill-subtle",
          expanded ? "rounded-t-lg" : "rounded-lg",
        )}
      >
        <div className="flex min-w-[min(100%,220px)] flex-1 items-center gap-2">
          <button
            type="button"
            aria-expanded={expanded}
            aria-label={expanded ? `Collapse ${loop.name}` : `Expand ${loop.name}`}
            onClick={(e) => {
              e.stopPropagation();
              onToggle();
            }}
            className="inline-flex size-5 shrink-0 items-center justify-center rounded-sm text-fg-faint hover:text-fg"
          >
            <ChevronRight size={12} className={cx("transition-transform", expanded && "rotate-90")} />
          </button>
          <div className="flex min-w-0 flex-1 flex-col gap-0.5">
            <div className="flex min-w-0 flex-wrap items-center gap-2">
              <span className="truncate font-medium text-body text-fg">{loop.name}</span>
              <StatusChip state={state} waiting={waiting} completed={completed} />
            </div>
            <div className="flex min-w-0 items-center gap-1.5 text-caption text-fg-muted">
              <span className="truncate">
                {projectName(loop.workspace)}
                {loop.machine && ` on ${loop.machine}`} · {scheduleInWords(loop.schedule)}
              </span>
              {loop.script && (
                <Tooltip label="Runs a pre-check script first">
                  <span className="inline-flex shrink-0 items-center text-fg-faint">
                    <Terminal size={12} />
                  </span>
                </Tooltip>
              )}
              {loop.webhook && loop.schedule.kind !== "manual" && (
                <Tooltip label="Webhook trigger enabled">
                  <span className="inline-flex shrink-0 items-center text-fg-faint">
                    <Webhook size={12} />
                  </span>
                </Tooltip>
              )}
            </div>
          </div>
        </div>

        {/* Outcome first, next run second: aligned across rows. */}
        <div className="flex min-w-0 flex-1 flex-col gap-0.5 sm:max-w-[270px] sm:basis-64">
          <div className="flex min-w-0 items-center gap-1.5">
            <span aria-hidden className={cx("size-1.5 shrink-0 rounded-full", runDotTint(latestRun))} />
            {lastThread ? (
              <Tooltip label="Open the last run's thread">
                <button
                  type="button"
                  data-row-action
                  onClick={() => selectThread(lastThread)}
                  className={cx(
                    "group/last flex min-w-0 items-center gap-1 rounded text-left text-callout hover:text-fg",
                    quietResult ? "text-fg-muted" : OUTCOME_TEXT[result.tone],
                  )}
                >
                  <span className="truncate underline-offset-2 group-hover/last:underline">
                    {result.text}
                  </span>
                  <ArrowUpRight size={12} className="shrink-0 opacity-60 group-hover/last:opacity-100" />
                </button>
              </Tooltip>
            ) : (
              <span
                title={result.text}
                className={cx(
                  "truncate text-callout",
                  quietResult ? "text-fg-muted" : OUTCOME_TEXT[result.tone],
                )}
              >
                {result.text}
              </span>
            )}
          </div>
          {next !== undefined ? (
            <time
              dateTime={new Date(next).toISOString()}
              title={NEXT_RUN_FORMAT.format(next)}
              className="tabular truncate pl-3 text-caption text-fg-faint"
            >
              {secondarySchedule}
            </time>
          ) : (
            <span className="truncate pl-3 text-caption text-fg-faint">{secondarySchedule}</span>
          )}
        </div>

        {/* Compact right controls: Stop/Run, Pause switch, and overflow menu. */}
        <div data-row-action className="flex shrink-0 items-center gap-1.5">
          {running ? (
            <IconAction
              label="Stop running job"
              tone="danger"
              onClick={() => act("Stopped run", () => cancelLoopRun(loop.id))}
            >
              <SquareStop size={14} />
            </IconAction>
          ) : (
            <IconAction
              label={loop.paused ? "Resume to run now" : "Run now"}
              disabled={loop.paused}
              onClick={() => act("Started a run", () => runLoopNow(loop.id))}
            >
              <Play size={14} />
            </IconAction>
          )}
          <Tooltip label={loop.paused ? "Paused — click to enable" : "Enabled — click to pause"}>
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
              <DropdownMenu.Item className={menuItem} disabled={running} onSelect={onEdit}>
                <Pencil size={13} className="text-fg-muted" /> {running ? "Edit (wait for run)" : "Edit"}
              </DropdownMenu.Item>
              <DropdownMenu.Item className={menuItem} onSelect={onDuplicate}>
                <Copy size={13} className="text-fg-muted" /> Duplicate
              </DropdownMenu.Item>
              {lastThread && (
                <DropdownMenu.Item className={menuItem} onSelect={() => selectThread(lastThread)}>
                  <ArrowUpRight size={13} className="text-fg-muted" /> Open last run
                </DropdownMenu.Item>
              )}
              <DropdownMenu.Separator className={menuSeparator} />
              <DropdownMenu.Item className={cx(menuItem, menuItemDanger)} onSelect={onDelete}>
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
      {expanded && (
        <JobDetail
          loop={loop}
          now={now}
          agent={agent}
          running={running}
          onEdit={onEdit}
          onDuplicate={onDuplicate}
        />
      )}
    </li>
  );
}

function MetaChip({ children, tone }: { children: React.ReactNode; tone?: "warning" }) {
  return (
    <span
      className={cx(
        "inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-caption",
        tone === "warning" ? "bg-warning/12 text-warning" : "bg-fill-subtle text-fg-muted",
      )}
    >
      {children}
    </span>
  );
}

function JobDetail({
  loop,
  now,
  agent,
  running,
  onEdit,
  onDuplicate,
}: {
  loop: Loop;
  now: number;
  agent: string;
  running: boolean;
  onEdit: () => void;
  onDuplicate: () => void;
}) {
  const timezone = "timezone" in loop.schedule ? loop.schedule.timezone : undefined;
  return (
    <div className="mx-4 flex flex-col gap-3 border-t border-border pt-3 pb-4">
      <div className="flex items-center justify-between gap-2">
        <span className="font-medium text-caption text-fg-muted">Prompt</span>
        <div className="flex items-center gap-1">
          <Pill size="sm" quiet disabled={running} onClick={onEdit}>
            <Pencil size={12} /> Edit
          </Pill>
          <Pill size="sm" quiet onClick={onDuplicate}>
            <Copy size={12} /> Duplicate
          </Pill>
        </div>
      </div>
      <p className="selectable whitespace-pre-wrap break-words rounded-md bg-fill-subtle px-3 py-2 text-callout text-fg">
        {loop.prompt}
      </p>
      {loop.script && (
        <div className="flex flex-col gap-1">
          <span className="font-medium text-caption text-fg-muted">Pre-check script</span>
          <pre className="selectable overflow-x-auto whitespace-pre-wrap break-words rounded-md bg-fill-subtle px-3 py-2 font-mono text-caption text-fg">
            {loop.script}
          </pre>
        </div>
      )}
      <div className="flex flex-wrap items-center gap-1.5">
        {agent && <MetaChip>{agent}</MetaChip>}
        <MetaChip tone={loop.readOnly ? undefined : "warning"}>
          {loop.readOnly ? "Read-only" : "Full access"}
        </MetaChip>
        {timezone && <MetaChip>{timezone}</MetaChip>}
        {loop.webhook && (
          <MetaChip>
            <Webhook size={10} /> Webhook
          </MetaChip>
        )}
        {loop.machine && <MetaChip>Runs on {loop.machine}</MetaChip>}
      </div>
      {loop.runs.length > 0 && (
        <div className="flex flex-col gap-1 border-t border-border pt-3">
          <span className="font-medium text-caption text-fg-muted">Recent runs</span>
          <ol aria-label="Recent runs" className="flex flex-col">
            {loop.runs.map((run) => {
              const at = run.finishedAt ?? run.startedAt ?? run.scheduledAt;
              const duration = runDurationText(run.startedAt, run.finishedAt);
              const text =
                run.state === "succeeded"
                  ? run.silent
                    ? "Script printed nothing"
                    : run.queued
                      ? `Waiting for ${run.machine ?? "its machine"} (offline)`
                      : run.machine
                        ? `Started on ${run.machine}`
                        : run.quiet
                          ? "Nothing to report"
                          : (run.verdict ?? "Finished")
                  : run.state === "running" || run.state === "claimed"
                    ? "Running…"
                    : run.state === "cancelled"
                      ? run.skipped
                        ? `Skipped: ${run.skipped}`
                        : "Stopped"
                      : `Failed${run.error ? `: ${run.error}` : ""}`;
              const threadId = runThread(run);
              const isError = run.state === "failed" || run.state === "ambiguous";
              const isActionable = run.state === "succeeded" && !run.quiet && !run.silent;
              const body = (
                <>
                  <span aria-hidden className={cx("size-1.5 shrink-0 rounded-full", runDotTint(run))} />
                  <span
                    title={`${FULL_RUN_FORMAT.format(at)} (${agoText(at, now)})`}
                    className="tabular w-28 shrink-0 text-fg-faint"
                  >
                    {runTimestampText(at, now)}
                  </span>
                  <span className="tabular w-12 shrink-0 text-fg-faint">{duration ?? "—"}</span>
                  <span
                    className={cx(
                      "min-w-0 flex-1 break-words",
                      isError ? "text-danger" : isActionable ? "text-fg" : "text-fg-muted",
                    )}
                  >
                    {text}
                  </span>
                </>
              );
              return (
                <li key={run.id} className="flex min-w-0">
                  {threadId ? (
                    <button
                      type="button"
                      onClick={() => selectThread(threadId)}
                      className="group/run -mx-2 flex min-w-0 flex-1 items-center gap-2 rounded-md px-2 py-1.5 text-left text-caption hover:bg-fill-subtle"
                    >
                      {body}
                      <span className="flex shrink-0 items-center gap-1 text-fg-faint group-hover/run:text-fg">
                        Open <ArrowUpRight size={12} />
                      </span>
                    </button>
                  ) : (
                    <div className="flex min-w-0 flex-1 items-center gap-2 py-1.5 text-caption">{body}</div>
                  )}
                </li>
              );
            })}
          </ol>
        </div>
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
        Nothing to review. Failed runs and threads jobs left open in the last {REVIEW_DAYS} days show here.
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
            <span
              aria-hidden
              className={cx("size-1.5 shrink-0 rounded-full", kind === "failed" ? "bg-danger" : "bg-success")}
            />
            <div className="flex min-w-0 flex-1 flex-col gap-0.5">
              <div className="flex items-center gap-2">
                <span className="truncate font-medium text-callout text-fg">{loop.name}</span>
                <span className="tabular shrink-0 text-caption text-fg-faint">
                  {runTimestampText(at, now)}
                </span>
              </div>
              <span
                className={cx("truncate text-caption", kind === "failed" ? "text-danger" : "text-fg-muted")}
              >
                {kind === "failed"
                  ? `Failed${run.error ? `: ${run.error}` : ""}`
                  : (run.verdict ?? "Thread is open for review")}
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
  const signedOut = useApp((s) => s.signedOutLogins);
  const statusMarks = useApp((s) => s.statusMarks);
  const [tab, setTab] = useState<Tab>("jobs");
  const [expanded, setExpanded] = useState<string | null>(null);
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

  // Opened from one of a job's runs: match by stable id, falling back to legacy name.
  const reveal = useLoopThreads((s) => s.reveal);
  const listRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!reveal || !data) return;
    const match = data.loops.find((loop) => loop.id === reveal || loop.name === reveal);
    if (!match) return;
    setTab("jobs");
    setExpanded(match.id);
    useLoopThreads.setState({ reveal: null });
    requestAnimationFrame(() =>
      listRef.current
        ?.querySelector(`[data-loop="${CSS.escape(match.id)}"]`)
        ?.scrollIntoView({ block: "nearest" }),
    );
  }, [reveal, data]);

  const upcoming: Loop[] = [];
  const inactive: Loop[] = [];
  for (const loop of loops) {
    if (loopState(loop) === "running" || (!loop.paused && loop.nextDueAt !== undefined) || listening(loop))
      upcoming.push(loop);
    else inactive.push(loop);
  }
  upcoming.sort(
    (a, b) => (a.nextDueAt ?? Infinity) - (b.nextDueAt ?? Infinity) || a.name.localeCompare(b.name),
  );
  inactive.sort((a, b) => Number(b.paused) - Number(a.paused) || a.name.localeCompare(b.name));

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

  return (
    <div className="flex min-h-0 flex-1 flex-col bg-bg">
      <ViewHeader title="Jobs">
        <div className="flex items-center gap-2">
          {loops.length > 0 && (
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
                check CI on main”—each run in a fresh thread. Runs are read-only unless you say otherwise.
              </p>
              <Pill prominent onClick={openNew}>
                New job
              </Pill>
            </div>
          ) : tab === "review" ? (
            <ReviewList items={review} now={now} onReviewed={markReviewed} />
          ) : (
            [
              { title: "Active", loops: upcoming },
              { title: "Paused & inactive", loops: inactive },
            ].map(
              (group) =>
                group.loops.length > 0 && (
                  <section key={group.title} aria-label={group.title} className="flex flex-col gap-2 pb-3">
                    <h2 className="px-1 font-medium text-caption text-fg-muted">{group.title}</h2>
                    <ul className="flex flex-col gap-2">
                      {group.loops.map((loop) => (
                        <JobRow
                          key={loop.id}
                          loop={loop}
                          now={now}
                          agent={jobAgent(loop, harnesses, signedOut)}
                          expanded={expanded === loop.id}
                          onToggle={() => setExpanded((id) => (id === loop.id ? null : loop.id))}
                          onEdit={() => {
                            setEditing(loop);
                            setDuplicating(false);
                            setDialogOpen(true);
                          }}
                          onDuplicate={() => {
                            setEditing(loop);
                            setDuplicating(true);
                            setDialogOpen(true);
                          }}
                          onDelete={() => confirmDelete(loop)}
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
        duplicate={duplicating}
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
