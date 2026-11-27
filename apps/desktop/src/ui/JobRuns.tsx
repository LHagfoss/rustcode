/**
 * A job's runs as the Jobs page shows them: recent run dots on a card, a timeline strip of recent
 * runs, and the list of every recorded run in the job's dialog, each opening the thread it ran in.
 */

import { agoText, type LoopRun, runDurationText, runTimestampText } from "../state/loops";
import { formatCost } from "../state/pricing";
import { formatTokens } from "../state/thread";
import { selectThread } from "../state/threadOpen";
import { ArrowUpRight } from "./icons";
import { cx, Spinner, Tooltip } from "./primitives";

export const FULL_RUN_FORMAT = new Intl.DateTimeFormat(undefined, {
  dateStyle: "medium",
  timeStyle: "short",
});

/** The thread a run went to on this computer; a hub job's thread is on its member. */
export const runThread = (run: { machine?: string; threadId?: string } | undefined) =>
  run && !run.machine ? run.threadId : undefined;

export const isFailedRun = (run: LoopRun) => run.state === "failed" || run.state === "ambiguous";

export function runDotTint(run: LoopRun | undefined): string {
  if (!run) return "border border-fg-faint";
  if (run.state === "running" || run.state === "claimed") return "bg-accent";
  if (isFailedRun(run)) return "bg-danger";
  if (run.state === "cancelled") return "bg-fg-faint";
  if (run.quiet || run.silent) return "bg-fg-faint";
  if (run.outcome === "blocked" || run.outcome === "attention") return "bg-warning";
  return "bg-success";
}

export function runStatusMeta(run: LoopRun): {
  label: string;
  tone: "ok" | "warn" | "bad" | "accent" | "muted";
} {
  if (run.state === "running" || run.state === "claimed") return { label: "Running", tone: "accent" };
  if (isFailedRun(run)) return { label: run.signedOut ? "Signed out" : "Failed", tone: "bad" };
  if (run.state === "cancelled") return { label: run.skipped ? "Skipped" : "Stopped", tone: "muted" };
  if (run.queued) return { label: "Queued", tone: "muted" };
  if (run.silent) return { label: "Silent", tone: "muted" };
  if (run.quiet) return { label: "Quiet", tone: "muted" };
  if (run.outcome === "blocked") return { label: "Blocked", tone: "warn" };
  if (run.outcome === "attention") return { label: "Needs attention", tone: "warn" };
  if (run.outcome === "clear") return { label: "All clear", tone: "ok" };
  return { label: "Succeeded", tone: "ok" };
}

/** Average duration across finished runs that recorded both timestamps. */
export function averageRunDuration(runs: readonly LoopRun[]): string | null {
  let total = 0;
  let count = 0;
  for (const run of runs) {
    if (run.startedAt !== undefined && run.finishedAt !== undefined && run.finishedAt >= run.startedAt) {
      total += run.finishedAt - run.startedAt;
      count += 1;
    }
  }
  if (!count) return null;
  return runDurationText(0, Math.round(total / count));
}

/** What one run did, in a few words. */
export function runText(run: LoopRun): string {
  if (run.state === "succeeded") {
    if (run.silent) return "Script printed nothing";
    if (run.queued) return `Waiting for ${run.machine ?? "its machine"} (offline)`;
    if (run.machine) return `Started on ${run.machine}`;
    if (run.quiet) return "Nothing to report";
    if (run.runId) return "Workflow finished";
    return run.verdict ?? "Finished";
  }
  if (run.state === "running" || run.state === "claimed") return "Running…";
  if (run.state === "cancelled") return run.skipped ? `Skipped: ${run.skipped}` : "Stopped";
  return `Failed${run.error ? `: ${run.error}` : ""}`;
}

/** Detail text for expanded run rows where the status badge is already shown above. */
function detailedRunText(run: LoopRun): string {
  if (isFailedRun(run)) return run.error ?? "Run failed";
  if (run.state === "cancelled") return run.skipped ?? "Stopped by user";
  return runText(run);
}

const HISTORY_DOTS = 5;
const STRIP_RUNS = 14;

/** The last few runs as dots, oldest first. */
export function RunHistory({ runs, onOpen }: { runs: readonly LoopRun[]; onOpen?: () => void }) {
  const recent = runs.slice(0, HISTORY_DOTS).reverse();
  if (!recent.length) return <span className="text-caption text-fg-faint">No runs yet</span>;
  const failed = recent.filter(isFailedRun).length;
  const label = failed
    ? `${failed} of the last ${recent.length} runs failed`
    : `The last ${recent.length} runs, oldest first`;
  const dots = recent.map((run) => (
    <span key={run.id} className={cx("size-1.5 rounded-full", runDotTint(run))} />
  ));
  return (
    <Tooltip label={label}>
      {onOpen ? (
        <button
          type="button"
          tabIndex={-1}
          aria-label={label}
          onClick={onOpen}
          className="relative flex shrink-0 items-center gap-1 py-1"
        >
          {dots}
        </button>
      ) : (
        <span role="img" aria-label={label} className="flex shrink-0 items-center gap-1">
          {dots}
        </span>
      )}
    </Tooltip>
  );
}

/** Visual strip of up to 14 recent runs (oldest to newest), each clickable to jump to that run. */
export function RunTimelineStrip({
  runs,
  now,
  onSelectRun,
}: {
  runs: readonly LoopRun[];
  now: number;
  onSelectRun?: (runId: string) => void;
}) {
  const recent = runs.slice(0, STRIP_RUNS).reverse();
  if (!recent.length) return null;
  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex items-center justify-between text-caption text-fg-faint">
        <span>Recent activity</span>
        <span>Oldest → newest</span>
      </div>
      <div className="flex items-center gap-1">
        {recent.map((run) => {
          const at = run.finishedAt ?? run.startedAt ?? run.scheduledAt;
          const duration = runDurationText(run.startedAt, run.finishedAt);
          const status = runStatusMeta(run);
          const barTint =
            status.tone === "accent"
              ? "bg-accent"
              : status.tone === "bad"
                ? "bg-danger"
                : status.tone === "warn"
                  ? "bg-warning"
                  : status.tone === "ok"
                    ? "bg-success"
                    : "bg-fill-strong";
          const label = `${status.label} · ${runTimestampText(at, now)}${duration ? ` (${duration})` : ""} · ${runText(run)}`;
          return (
            <Tooltip key={run.id} label={label}>
              {onSelectRun ? (
                <button
                  type="button"
                  aria-label={`Jump to ${status.label} run at ${runTimestampText(at, now)}`}
                  onClick={() => onSelectRun(run.id)}
                  className={cx(
                    "h-4 flex-1 max-w-8 cursor-pointer rounded-xs transition-opacity hover:opacity-80 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-accent",
                    barTint,
                  )}
                />
              ) : (
                <span
                  role="img"
                  aria-label={`${status.label} at ${runTimestampText(at, now)}`}
                  className={cx("h-4 flex-1 max-w-8 rounded-xs", barTint)}
                />
              )}
            </Tooltip>
          );
        })}
      </div>
    </div>
  );
}

/** Every recorded run, newest first; a run with a thread opens it. */
export function RunList({
  runs,
  now,
  threadOf = runThread,
  onOpenThread,
  highlightedRunId,
  detailed = false,
}: {
  runs: readonly LoopRun[];
  now: number;
  /** App thread key (`host/id` or local `id`) to open for a run. */
  threadOf?: (run: LoopRun) => string | undefined;
  /** After a run's thread opens: the dialog showing the list closes or marks the run reviewed. */
  onOpenThread?: (runId: string) => void;
  /** Run id briefly highlighted after clicking its bar in the timeline strip. */
  highlightedRunId?: string | null;
  /** Expanded presentation used in the Job Overview dialog. */
  detailed?: boolean;
}) {
  if (!runs.length) return <p className="py-2 text-caption text-fg-faint">No runs yet.</p>;
  return (
    <ol aria-label="Runs" className={cx("flex flex-col", detailed && "gap-1.5")}>
      {runs.map((run) => {
        const at = run.finishedAt ?? run.startedAt ?? run.scheduledAt;
        const duration = runDurationText(run.startedAt, run.finishedAt);
        const threadId = threadOf(run);
        const actionable = run.state === "succeeded" && !run.quiet && !run.silent;
        const active = run.state === "running" || run.state === "claimed";
        const status = runStatusMeta(run);
        const highlighted = highlightedRunId === run.id;

        if (detailed) {
          const badgeTone =
            status.tone === "accent"
              ? "bg-accent/14 text-accent"
              : status.tone === "bad"
                ? "bg-danger/12 text-danger"
                : status.tone === "warn"
                  ? "bg-warning/14 text-warning"
                  : status.tone === "ok"
                    ? "bg-success/12 text-success"
                    : "bg-fill text-fg-muted";
          const hasCost = typeof run.costUsd === "number" && Number.isFinite(run.costUsd) && run.costUsd >= 0;
          const hasTokens = typeof run.tokens === "number" && Number.isFinite(run.tokens) && run.tokens > 0;
          const content = (
            <div className="flex min-w-0 flex-1 flex-col gap-1">
              <div className="flex flex-wrap items-center gap-2">
                <span
                  className={cx(
                    "inline-flex shrink-0 items-center gap-1 rounded-full px-2 py-0.5 font-medium text-caption",
                    badgeTone,
                  )}
                >
                  {active ? (
                    <Spinner size={10} />
                  ) : (
                    <span aria-hidden className={cx("size-1.5 rounded-full", runDotTint(run))} />
                  )}
                  {status.label}
                </span>
                <span title={FULL_RUN_FORMAT.format(at)} className="tabular text-caption text-fg-muted">
                  {runTimestampText(at, now)}
                </span>
                <span className="text-caption text-fg-faint">{agoText(at, now)}</span>
                {duration && (
                  <span className="tabular rounded-xs bg-fill px-1.5 py-0.5 text-caption text-fg-muted">
                    {duration}
                  </span>
                )}
                {hasCost ? (
                  <Tooltip label={hasTokens ? `${formatTokens(run.tokens!)} tokens` : undefined}>
                    <span className="tabular rounded-xs bg-fill px-1.5 py-0.5 text-caption text-fg-muted">
                      {formatCost(run.costUsd)}
                    </span>
                  </Tooltip>
                ) : hasTokens ? (
                  <span className="tabular rounded-xs bg-fill px-1.5 py-0.5 text-caption text-fg-muted">
                    {formatTokens(run.tokens!)} tok
                  </span>
                ) : null}
              </div>
              <p
                className={cx(
                  "line-clamp-3 break-words text-callout",
                  isFailedRun(run) ? "text-danger" : actionable ? "text-fg" : "text-fg-muted",
                )}
              >
                {detailedRunText(run)}
              </p>
            </div>
          );
          return (
            <li
              key={run.id}
              data-run-id={run.id}
              className={cx(
                "flex min-w-0 rounded-lg bg-fill-subtle transition-shadow",
                highlighted && "bg-fill ring-1 ring-accent",
              )}
            >
              {threadId ? (
                <button
                  type="button"
                  onClick={() => {
                    onOpenThread?.(run.id);
                    selectThread(threadId);
                  }}
                  className="group/run flex min-w-0 flex-1 items-start gap-3 rounded-lg px-3 py-3 text-left hover:bg-fill"
                >
                  {content}
                  <span className="mt-0.5 inline-flex shrink-0 items-center gap-1 text-caption text-fg-faint group-hover/run:text-fg">
                    Open
                    <ArrowUpRight size={12} />
                  </span>
                </button>
              ) : (
                <div className="flex min-w-0 flex-1 items-start gap-3 px-3 py-3">{content}</div>
              )}
            </li>
          );
        }

        const body = (
          <>
            <span aria-hidden className={cx("mt-[5px] size-1.5 shrink-0 rounded-full", runDotTint(run))} />
            <span className="flex min-w-0 flex-1 flex-col gap-0.5">
              <span className="flex items-center gap-2 text-fg-faint">
                <span title={`${FULL_RUN_FORMAT.format(at)} (${agoText(at, now)})`} className="tabular">
                  {runTimestampText(at, now)}
                </span>
                {duration && <span className="tabular">{duration}</span>}
              </span>
              <span
                className={cx(
                  "line-clamp-2 break-words",
                  isFailedRun(run) ? "text-danger" : actionable ? "text-fg" : "text-fg-muted",
                )}
              >
                {runText(run)}
              </span>
            </span>
          </>
        );
        return (
          <li key={run.id} data-run-id={run.id} className="flex min-w-0">
            {threadId ? (
              <button
                type="button"
                onClick={() => {
                  onOpenThread?.(run.id);
                  selectThread(threadId);
                }}
                className="group/run -mx-2 flex min-w-0 flex-1 items-start gap-2 rounded-md px-2 py-1.5 text-left text-caption hover:bg-fill-subtle"
              >
                {body}
                <ArrowUpRight
                  size={12}
                  aria-label="Open thread"
                  className="mt-0.5 shrink-0 text-fg-faint opacity-0 group-hover/run:opacity-100 group-focus-visible/run:opacity-100"
                />
              </button>
            ) : (
              <div className="flex min-w-0 flex-1 items-start gap-2 py-1.5 text-caption">{body}</div>
            )}
          </li>
        );
      })}
    </ol>
  );
}
