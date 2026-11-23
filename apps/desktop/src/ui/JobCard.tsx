/**
 * One job on the Jobs page: everything worth a glance without opening it (its agent, what the last
 * run said, the schedule, recent runs and the next one). The card opens the job's dialog, where it
 * is edited and its runs are listed; Run now and the menu stay on the card.
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
  Copy,
  Ellipsis,
  Pause,
  Pencil,
  Play,
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
    return <span className={cx(chip, "bg-danger/12 font-medium text-danger")}>Paused</span>;
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
  const togglePause = () =>
    act(loop.paused ? "Resumed" : "Paused", () => setLoopPaused(loop.id, !loop.paused, loop.host));

  return (
    <li
      data-loop={loopKey(loop)}
      className={cx(
        "group/job relative flex min-w-0 flex-col gap-2 rounded-lg bg-surface p-4 hover:bg-fill-subtle",
        reason && "ring-1 ring-danger/35",
        loop.paused && !reason && "opacity-70",
      )}
    >
      <div className="flex items-start gap-2">
        <button
          type="button"
          onClick={onOpen}
          className="min-w-0 flex-1 text-left font-semibold text-title3 text-fg after:absolute after:inset-0 after:rounded-lg focus-visible:outline-none focus-visible:after:ring-1 focus-visible:after:ring-accent"
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

      {agent && (
        <span title={agent} className="-mt-1 truncate text-caption text-fg-muted">
          {agent}
        </span>
      )}

      {reason ? (
        <div className="relative flex items-center gap-2">
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
      ) : (
        <p
          title={result.text}
          className={cx(
            "line-clamp-2 text-callout",
            result.tone === "bad"
              ? "text-danger"
              : quiet || result.tone === "muted"
                ? "text-fg-muted"
                : "text-fg",
          )}
        >
          {result.text}
        </p>
      )}

      <div className="mt-auto flex min-w-0 items-center gap-2 border-t border-border pt-2 text-caption text-fg-muted">
        <span className="min-w-0 flex-1 truncate">
          {projectName(loop.workspace)}
          {(machineLabel || loop.machine) && ` on ${machineLabel || loop.machine}`} ·{" "}
          {scheduleInWords(loop.schedule)}
        </span>
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
      <div className="flex items-center justify-between gap-2 text-caption text-fg-muted">
        <span className="flex items-center gap-2">
          {loop.runs.length > 0 && (
            <span>
              {loop.runs.length} {loop.runs.length === 1 ? "run" : "runs"}
            </span>
          )}
          <RunHistory runs={loop.runs} />
        </span>
        <span className="shrink-0">
          Run history <ArrowUpRight size={12} className="inline" />
        </span>
      </div>
    </li>
  );
}
