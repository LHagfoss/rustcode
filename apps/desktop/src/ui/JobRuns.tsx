/**
 * A job's runs as the Jobs page shows them: the dots of its last few runs on a card, and the list
 * of every recorded run in the job's dialog, each opening the thread it ran in.
 */

import { agoText, type LoopRun, runDurationText, runTimestampText } from "../state/loops";
import { selectThread } from "../state/threadOpen";
import { ArrowUpRight } from "./icons";
import { cx, Tooltip } from "./primitives";

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
  return run.quiet || run.silent ? "bg-fg-faint" : "bg-success";
}

/** What one run did, in a few words. */
export function runText(run: LoopRun): string {
  if (run.state === "succeeded") {
    if (run.silent) return "Script printed nothing";
    if (run.queued) return `Waiting for ${run.machine ?? "its machine"} (offline)`;
    if (run.machine) return `Started on ${run.machine}`;
    if (run.quiet) return "Nothing to report";
    return run.verdict ?? "Finished";
  }
  if (run.state === "running" || run.state === "claimed") return "Running…";
  if (run.state === "cancelled") return run.skipped ? `Skipped: ${run.skipped}` : "Stopped";
  return `Failed${run.error ? `: ${run.error}` : ""}`;
}

const HISTORY_DOTS = 5;

/** The last few runs as dots, oldest first. */
export function RunHistory({ runs }: { runs: readonly LoopRun[] }) {
  const recent = runs.slice(0, HISTORY_DOTS).reverse();
  if (!recent.length) return <span className="text-caption text-fg-faint">No runs yet</span>;
  const failed = recent.filter(isFailedRun).length;
  const label = failed
    ? `${failed} of the last ${recent.length} runs failed`
    : `The last ${recent.length} runs, oldest first`;
  return (
    <Tooltip label={label}>
      <span role="img" aria-label={label} className="flex shrink-0 items-center gap-0.5">
        {recent.map((run) => (
          <span key={run.id} className={cx("size-1.5 rounded-full", runDotTint(run))} />
        ))}
      </span>
    </Tooltip>
  );
}

/** Every recorded run, newest first; a run with a thread opens it. */
export function RunList({
  runs,
  now,
  threadOf = runThread,
  onOpenThread,
}: {
  runs: readonly LoopRun[];
  now: number;
  /** App thread key (`host/id` or local `id`) to open for a run. */
  threadOf?: (run: LoopRun) => string | undefined;
  /** After a run's thread opens: the dialog showing the list closes. */
  onOpenThread?: () => void;
}) {
  if (!runs.length) return <p className="py-2 text-caption text-fg-faint">No runs yet.</p>;
  return (
    <ol aria-label="Runs" className="flex flex-col">
      {runs.map((run) => {
        const at = run.finishedAt ?? run.startedAt ?? run.scheduledAt;
        const duration = runDurationText(run.startedAt, run.finishedAt);
        const threadId = threadOf(run);
        const actionable = run.state === "succeeded" && !run.quiet && !run.silent;
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
          <li key={run.id} className="flex min-w-0">
            {threadId ? (
              <button
                type="button"
                onClick={() => {
                  onOpenThread?.();
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
