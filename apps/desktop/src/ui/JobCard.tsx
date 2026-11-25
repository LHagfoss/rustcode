/**
 * One job on the Jobs page: everything worth a glance without opening it (its agent, last run or
 * prompt preview, project, machine, schedule, next run, and recent runs). The card opens the job
 * overview dialog; Run now and the menu stay on the card.
 */

import { DropdownMenu } from "radix-ui";
import { projectName } from "../acp/types";
import { loopKey, runThreadKey } from "../state/fleetLoops";
import {
  cancelLoopRun,
  type Loop,
  lastResult,
  loopState,
  nextRunText,
  pauseReason,
  runLoopNow,
  scheduleInWords,
  setLoopPaused,
} from "../state/loops";
import { selfName, useLocalIdentity, useMachines } from "../state/machines";
import { openSettings } from "../state/store";
import { selectThread } from "../state/threadOpen";
import {
  ArrowUpRight,
  Clock,
  Copy,
  Ellipsis,
  Folder,
  Pause,
  Pencil,
  Play,
  ShieldAlert,
  SquareStop,
  Terminal,
  Trash2,
  Webhook,
} from "./icons";
import { FULL_RUN_FORMAT, RunHistory } from "./JobRuns";
import {
  cx,
  MenuContent,
  menuItem,
  menuItemDanger,
  menuSeparator,
  Pill,
  Skeleton,
  Spinner,
  Tooltip,
} from "./primitives";

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
const NEXT_RUN_DATE = new Intl.DateTimeFormat(undefined, {
  month: "short",
  day: "numeric",
  hour: "2-digit",
  minute: "2-digit",
  hour12: false,
});
const WEEK_MS = 6 * 86_400_000;

/** "Next 09:59" today, "Next Mon 08:00" this week, then the date. */
function nextRunLine(next: number, now: number): string {
  const when =
    new Date(next).toDateString() === new Date(now).toDateString()
      ? NEXT_RUN_TIME
      : next - now < WEEK_MS
        ? NEXT_RUN_WEEKDAY
        : NEXT_RUN_DATE;
  return `Next ${when.format(next)}`;
}

/** A job that is not paused and has a webhook: a call can start it at any time. */
export const listening = (loop: Loop) => !loop.paused && loop.webhook;

function StatusChip({ loop, waiting }: { loop: Loop; waiting: boolean }) {
  const state = loopState(loop);
  const chip = "inline-flex shrink-0 items-center gap-1 rounded-full px-2 py-0.5 text-caption";
  if (state === "running")
    return (
      <span className={cx(chip, "bg-accent/14 font-medium text-accent")}>
        <Spinner size={10} />
        Running
      </span>
    );
  if (state === "failing")
    return <span className={cx(chip, "bg-danger/12 font-medium text-danger")}>Needs attention</span>;
  if (state === "paused") return <span className={cx(chip, "bg-fill text-fg-muted")}>Paused</span>;
  if (waiting)
    return (
      <span className={cx(chip, "bg-fill text-fg-muted")}>
        <Webhook size={10} />
        Webhook
      </span>
    );
  return null;
}

function IconAction({
  label,
  onClick,
  children,
  tone,
}: {
  label: string;
  onClick: () => void;
  children: React.ReactNode;
  tone?: "danger";
}) {
  return (
    <Tooltip label={label}>
      <button
        type="button"
        aria-label={label}
        onClick={onClick}
        className={cx(
          "inline-flex size-7 items-center justify-center rounded-md",
          tone === "danger" ? "text-danger hover:bg-danger/12" : "text-fg-muted hover:bg-fill hover:text-fg",
        )}
      >
        {children}
      </button>
    </Tooltip>
  );
}

export function JobCard({
  loop,
  now,
  agent,
  machineLabel,
  onOpen,
  onEdit,
  onDuplicate,
  onDelete,
  act,
}: {
  loop: Loop;
  now: number;
  agent: string;
  /** Shown in the footer when the network has more than one machine, or for hub-dispatched jobs. */
  machineLabel?: string;
  onOpen: () => void;
  onEdit: () => void;
  onDuplicate: () => void;
  onDelete: () => void;
  act: (what: string, run: () => Promise<void>) => void;
}) {
  const machines = useMachines((s) => s.machines);
  const local = useLocalIdentity();
  const localName = local.displayName ?? selfName();
  const state = loopState(loop);
  const running = state === "running";
  const reason = pauseReason(loop);
  const result = lastResult(loop, now);
  const latest = loop.runs[0];
  const lastThread = running ? undefined : runThreadKey(loop, latest, machines, localName);
  const next = !loop.paused && loop.nextDueAt !== undefined ? loop.nextDueAt : undefined;
  const waiting = state === "active" && next === undefined && listening(loop);
  const quiet = latest?.state === "succeeded" && (latest.quiet || latest.silent);
  const hasRuns = loop.runs.length > 0;
  const promptPreview = loop.prompt.trim();
  const togglePause = () =>
    act(loop.paused ? "Resumed" : "Paused", () => setLoopPaused(loop.id, !loop.paused, loop.host));

  return (
    <li
      data-loop={loopKey(loop)}
      className={cx(
        "group/job relative flex min-w-0 flex-col gap-2 rounded-xl bg-surface p-4 hover:bg-surface-2",
        reason && "ring-1 ring-danger/35",
        loop.paused && !reason && "opacity-75",
      )}
    >
      <div className="flex items-start gap-2">
        <button
          type="button"
          onClick={onOpen}
          className="min-w-0 flex-1 text-left font-semibold text-title3 text-fg after:absolute after:inset-0 after:rounded-xl focus-visible:outline-none focus-visible:after:ring-1 focus-visible:after:ring-accent"
        >
          <span className="line-clamp-2">{loop.name}</span>
        </button>
        <StatusChip loop={loop} waiting={waiting} />
        <div className="relative -mt-1 -mr-2 flex shrink-0 items-center">
          {running ? (
            <IconAction
              label="Stop running job"
              tone="danger"
              onClick={() => act("Stopped run", () => cancelLoopRun(loop.id, loop.host))}
            >
              <SquareStop size={14} />
            </IconAction>
          ) : (
            !loop.paused && (
              <span className="opacity-0 transition-opacity group-focus-within/job:opacity-100 group-hover/job:opacity-100">
                <IconAction
                  label="Run now"
                  onClick={() => act("Started a run", () => runLoopNow(loop.id, loop.host))}
                >
                  <Play size={14} />
                </IconAction>
              </span>
            )
          )}
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
              <DropdownMenu.Item className={menuItem} onSelect={togglePause}>
                {loop.paused ? (
                  <Play size={13} className="text-fg-muted" />
                ) : (
                  <Pause size={13} className="text-fg-muted" />
                )}
                {loop.paused ? "Resume" : "Pause"}
              </DropdownMenu.Item>
              <DropdownMenu.Item className={menuItem} onSelect={onEdit}>
                <Pencil size={13} className="text-fg-muted" /> Edit
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

      {(agent || !loop.readOnly) && (
        <div className="-mt-1 flex min-w-0 items-center gap-2 text-caption text-fg-muted">
          {agent && (
            <span title={agent} className="min-w-0 truncate">
              {agent}
            </span>
          )}
          {!loop.readOnly && (
            <Tooltip label="Runs with full access (can edit files, commit, and push)">
              <span className="relative inline-flex shrink-0 items-center gap-1 text-warning">
                <ShieldAlert size={12} />
                Full access
              </span>
            </Tooltip>
          )}
        </div>
      )}

      {reason ? (
        <div className="relative flex items-center gap-2 pt-0.5">
          <p title={reason} className="line-clamp-2 min-w-0 flex-1 text-callout text-danger">
            {reason}
          </p>
          {latest?.signedOut ? (
            <Pill size="sm" onClick={() => openSettings("agents")}>
              Sign in
            </Pill>
          ) : (
            <Pill size="sm" onClick={togglePause}>
              <Play size={12} /> Resume
            </Pill>
          )}
        </div>
      ) : hasRuns ? (
        <p
          title={result.text}
          className={cx(
            "line-clamp-2 pt-0.5 text-callout",
            result.tone === "bad"
              ? "text-danger"
              : quiet || result.tone === "muted"
                ? "text-fg-muted"
                : "text-fg",
          )}
        >
          {result.text}
        </p>
      ) : (
        <p title={promptPreview || undefined} className="line-clamp-2 pt-0.5 text-callout text-fg-muted">
          {promptPreview || "No prompt set"}
        </p>
      )}

      <div className="mt-auto flex flex-col gap-1.5 border-t border-border pt-3 text-caption text-fg-muted">
        <div className="flex min-w-0 items-center justify-between gap-2">
          <span className="flex min-w-0 items-center gap-1.5 truncate">
            <Folder size={12} className="shrink-0 text-fg-faint" />
            <span className="truncate">
              {projectName(loop.workspace)}
              {(machineLabel || loop.machine) && ` · ${machineLabel || loop.machine}`}
            </span>
          </span>
          {next !== undefined ? (
            <time
              dateTime={new Date(next).toISOString()}
              title={`${nextRunText(loop, now)} · ${FULL_RUN_FORMAT.format(next)}`}
              className="tabular shrink-0 text-fg-faint"
            >
              {nextRunLine(next, now)}
            </time>
          ) : (
            <span className="shrink-0 text-fg-faint">{nextRunText(loop, now)}</span>
          )}
        </div>
        <div className="flex min-w-0 items-center justify-between gap-2">
          <span className="flex min-w-0 items-center gap-1.5">
            <Clock size={12} className="shrink-0 text-fg-faint" />
            <span className="truncate">{scheduleInWords(loop.schedule)}</span>
            {loop.script && (
              <Tooltip label="Runs a pre-check script first">
                <span className="relative inline-flex shrink-0 text-fg-faint">
                  <Terminal size={12} />
                </span>
              </Tooltip>
            )}
            {loop.webhook && loop.schedule.kind !== "manual" && (
              <Tooltip label="Webhook trigger enabled">
                <span className="relative inline-flex shrink-0 text-fg-faint">
                  <Webhook size={12} />
                </span>
              </Tooltip>
            )}
          </span>
          <span className="flex shrink-0 items-center gap-1.5 text-fg-faint">
            <RunHistory runs={loop.runs} onOpen={onOpen} />
            {hasRuns && (
              <span className="tabular">
                {loop.runs.length} {loop.runs.length === 1 ? "run" : "runs"}
              </span>
            )}
          </span>
        </div>
      </div>
    </li>
  );
}

/** Where the job cards will be while the first list loads: the grid of cards, each shaped like one. */
export function JobCardsSkeleton() {
  return (
    <ul
      role="status"
      aria-label="Loading"
      className="grid grid-cols-[repeat(auto-fill,minmax(min(100%,320px),1fr))] gap-3"
    >
      {[58, 72, 46, 64, 52, 68].map((w, i) => (
        <li key={i} aria-hidden className="flex min-w-0 flex-col gap-2 rounded-xl bg-surface p-4">
          <Skeleton className="h-4" style={{ width: `${w}%` }} />
          <Skeleton className="h-2.5 w-28" />
          <div className="flex flex-col gap-1.5 pt-1.5">
            <Skeleton className="h-3 w-full" />
            <Skeleton className="h-3" style={{ width: `${w + 10}%` }} />
          </div>
          <div className="mt-2 flex flex-col gap-2 border-t border-border pt-3">
            <div className="flex items-center justify-between gap-2">
              <Skeleton className="h-2.5 w-24" />
              <Skeleton className="h-2.5 w-14" />
            </div>
            <div className="flex items-center justify-between gap-2">
              <Skeleton className="h-2.5 w-32" />
              <Skeleton className="h-2.5 w-10" />
            </div>
          </div>
        </li>
      ))}
    </ul>
  );
}
