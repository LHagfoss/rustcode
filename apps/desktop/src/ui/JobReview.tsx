/**
 * The Jobs page's Review tab: what jobs did on their own that the user has not looked at, as one
 * timeline by day. A job that failed the same way again and again is one row with a count; a row
 * opens its run's thread, and its check marks it reviewed.
 */

import { loopMachineLabel, runThreadKey } from "../state/fleetLoops";
import { type Loop, REVIEW_DAYS, type ReviewGroup, reviewAt, reviewDayLabel } from "../state/loops";
import { selfName, useLocalIdentity, useMachines } from "../state/machines";
import { useApp } from "../state/store";
import { selectThread } from "../state/threadOpen";
import { Archive, Check, Pencil } from "./icons";
import { cx, Tooltip } from "./primitives";

const TIME = new Intl.DateTimeFormat(undefined, { hour: "2-digit", minute: "2-digit", hour12: false });

function ReviewRow({
  group,
  showMachine,
  onReviewed,
  onReviewAndSettle,
  onEditJob,
}: {
  group: ReviewGroup;
  showMachine: boolean;
  onReviewed: (runIds: string[]) => void;
  onReviewAndSettle?: (runIds: string[], threadId?: string) => void;
  onEditJob?: (loop: Loop) => void;
}) {
  const { loop, run, kind } = group.item;
  const machines = useMachines((s) => s.machines);
  const local = useLocalIdentity();
  const localName = local.displayName ?? selfName();
  const threadId = runThreadKey(loop, run, machines, localName);
  const machineLabel = showMachine || loop.machine ? loopMachineLabel(loop, machines, localName) : null;
  const title = useApp((s) =>
    threadId ? s.sessions.find((session) => session.id === threadId)?.title : undefined,
  );
  const failed = kind === "failed";
  const blocked = kind === "blocked";
  const dotClass = failed ? "bg-danger" : blocked ? "bg-warning" : "bg-accent";
  const text = failed
    ? (run.error ?? "Run failed")
    : blocked
      ? (run.verdict ?? "Blocked")
      : (run.verdict ?? title ?? "Finished, and left its thread open for you");
  const count = group.runIds.length;
  const openRun = () => {
    if (!threadId) return;
    onReviewed(group.runIds);
    selectThread(threadId);
  };
  return (
    <li className="group/row relative grid grid-cols-[3rem_1rem_minmax(0,1fr)_auto] items-start gap-x-2 rounded-md py-2 pr-1 hover:bg-fill-subtle">
      <time
        dateTime={new Date(reviewAt(group.item)).toISOString()}
        className="tabular pt-0.5 text-right text-caption text-fg-faint"
      >
        {TIME.format(reviewAt(group.item))}
      </time>
      <span className="flex justify-center pt-[7px]">
        <span aria-hidden className={cx("size-1.5 rounded-full", dotClass)} />
      </span>
      <div className="flex min-w-0 flex-col gap-0.5">
        <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-0.5">
          {threadId ? (
            <button
              type="button"
              onClick={openRun}
              className="truncate text-left font-medium text-callout text-fg after:absolute after:inset-0 after:rounded-md focus-visible:outline-none focus-visible:after:ring-1 focus-visible:after:ring-accent"
            >
              {loop.name}
            </button>
          ) : (
            <span className="truncate font-medium text-callout text-fg">{loop.name}</span>
          )}
          {failed && (
            <span className="inline-flex shrink-0 items-center rounded-full bg-danger/12 px-1.5 py-0.2 font-medium text-[11px] text-danger">
              Failed
            </span>
          )}
          {blocked && (
            <span className="inline-flex shrink-0 items-center rounded-full bg-warning/14 px-1.5 py-0.2 font-medium text-[11px] text-warning">
              Blocked
            </span>
          )}
          {machineLabel && <span className="shrink-0 text-caption text-fg-faint">{machineLabel}</span>}
        </div>
        <p
          className={cx(
            "line-clamp-2 text-caption",
            failed ? "text-danger" : blocked ? "text-warning" : "text-fg-muted",
          )}
        >
          {text}
          {count > 1 && <span className="text-fg-faint"> · {count} runs</span>}
        </p>
      </div>
      <div className="relative z-10 flex shrink-0 items-center gap-0.5 opacity-0 transition-opacity focus-within:opacity-100 group-hover/row:opacity-100">
        {(failed || blocked) && onEditJob && (
          <Tooltip label="Edit job">
            <button
              type="button"
              aria-label={`Edit ${loop.name}`}
              onClick={() => onEditJob(loop)}
              className="inline-flex size-7 items-center justify-center rounded-md text-fg-faint hover:bg-fill hover:text-fg"
            >
              <Pencil size={13} />
            </button>
          </Tooltip>
        )}
        {kind === "open" && threadId && onReviewAndSettle && (
          <Tooltip label="Mark reviewed and settle thread">
            <button
              type="button"
              aria-label="Mark reviewed and settle thread"
              onClick={() => onReviewAndSettle(group.runIds, threadId)}
              className="inline-flex size-7 items-center justify-center rounded-md text-fg-faint hover:bg-fill hover:text-fg"
            >
              <Archive size={13} />
            </button>
          </Tooltip>
        )}
        <Tooltip label={count > 1 ? `Mark ${count} runs reviewed` : "Mark reviewed"}>
          <button
            type="button"
            aria-label="Mark reviewed"
            onClick={() => onReviewed(group.runIds)}
            className="inline-flex size-7 items-center justify-center rounded-md text-fg-faint hover:bg-fill hover:text-fg"
          >
            <Check size={14} />
          </button>
        </Tooltip>
      </div>
    </li>
  );
}

export function JobReview({
  groups,
  now,
  showMachine = false,
  onReviewed,
  onReviewAndSettle,
  onEditJob,
}: {
  groups: readonly ReviewGroup[];
  now: number;
  showMachine?: boolean;
  onReviewed: (runIds: string[]) => void;
  onReviewAndSettle?: (runIds: string[], threadId?: string) => void;
  onEditJob?: (loop: Loop) => void;
}) {
  if (!groups.length) {
    return (
      <p className="py-8 text-center text-callout text-fg-muted">
        All caught up. Failed runs, blocked checks, and threads jobs left open in the last {REVIEW_DAYS} days
        show here.
      </p>
    );
  }
  const days: { label: string; groups: ReviewGroup[] }[] = [];
  for (const group of groups) {
    const label = reviewDayLabel(reviewAt(group.item), now);
    const day = days.at(-1);
    if (day?.label === label) day.groups.push(group);
    else days.push({ label, groups: [group] });
  }
  return (
    <div className="flex flex-col gap-4">
      {days.map((day) => (
        <section key={day.label} aria-label={day.label}>
          <h2 className="pb-1 pl-[5rem] font-medium text-caption text-fg-muted">
            {day.label} · {day.groups.length}
          </h2>
          <ol className="relative flex flex-col before:absolute before:top-3 before:bottom-3 before:left-[calc(4rem-0.5px)] before:w-px before:bg-border">
            {day.groups.map((group) => (
              <ReviewRow
                key={`${group.item.loop.host}/${group.item.run.id}`}
                group={group}
                showMachine={showMachine}
                onReviewed={onReviewed}
                onReviewAndSettle={onReviewAndSettle}
                onEditJob={onEditJob}
              />
            ))}
          </ol>
        </section>
      ))}
    </div>
  );
}
