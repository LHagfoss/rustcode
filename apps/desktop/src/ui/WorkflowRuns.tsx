// A workflow run on screen (docs/workflows.md, issue 3394): the card in the chat that started it, and the
// row above the composer while it goes (inside `DelegateStatusPanel`, so there is one place for what
// a thread has running). Both draw the same `WorkflowRun` from `state/workflows.ts` and list the
// workers the same way: each is a delegate thread of the thread that started the run, so each row
// opens it.

import { useState } from "react";
import { modelName } from "../acp/types";
import type { WorkflowRun, WorkflowWorker } from "../acp/workflows";
import { useNow } from "../lib/useNow";
import { selectThread } from "../state/store";
import { formatDuration } from "../state/thread";
import { splitThreadKey } from "../state/threadKeys";
import { runUsageText } from "../state/workflowBudgets";
import {
  canRetry,
  elapsedMs,
  isLive,
  type PhaseStepState,
  phaseSteps,
  progressText,
  resumeWorkflow,
  retryWorkflow,
  runLine,
  runTitle,
  statusWord,
  stopWorkflow,
  useThreadRuns,
  useWorkflows,
  workerThread,
} from "../state/workflows";
import { ChevronDown, Workflow, X } from "./icons";
import { cx, Spinner, Tooltip } from "./primitives";

const WORKER_TINT: Record<WorkflowWorker["state"], string> = {
  running: "bg-accent",
  done: "bg-success",
  failed: "bg-danger",
};

const STEP_TINT: Record<PhaseStepState, string> = {
  active: "bg-accent",
  done: "bg-success",
  failed: "bg-danger",
};

/** The run's phases in order: a dot by how it went, the title, and `done/workers`. */
function PhaseSteps({ run }: { run: WorkflowRun }) {
  const steps = phaseSteps(run);
  if (!steps.length) return null;
  return (
    <ol aria-label="Phases" className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-0.5 text-caption">
      {steps.map((step) => (
        <li key={step.title} className="flex min-w-0 items-center gap-1.5">
          <span aria-hidden className={cx("size-1.5 shrink-0 rounded-full", STEP_TINT[step.state])} />
          <span className="min-w-0 truncate text-fg">{step.title}</span>
          {step.detail && <span className="shrink-0 text-fg-faint">{step.detail}</span>}
        </li>
      ))}
    </ol>
  );
}

function WorkerLine({
  worker,
  thread,
  now,
}: {
  worker: WorkflowWorker;
  thread: string | undefined;
  now: number;
}) {
  const ended = worker.finishedAt ?? now;
  const state =
    worker.state === "running"
      ? worker.sessionId
        ? "Working"
        : "Waiting for a slot"
      : worker.state === "done"
        ? "Done"
        : "Failed";
  const detail = [
    worker.phase ?? "",
    (worker.model ? modelName(worker.model) : undefined) ?? worker.harness ?? "",
    worker.error ?? "",
  ]
    .filter(Boolean)
    .join(" · ");
  const body = (
    <>
      <span className="flex min-w-0 items-center gap-2 text-callout">
        <span aria-hidden className={cx("size-1.5 shrink-0 rounded-full", WORKER_TINT[worker.state])} />
        <span className="min-w-0 truncate">{worker.prompt || `Worker ${worker.seq + 1}`}</span>
        <span
          className={cx(
            "ml-auto shrink-0 text-caption",
            worker.state === "failed" ? "text-danger" : "text-fg-muted",
          )}
        >
          {state} · {formatDuration(Math.max(0, ended - worker.startedAt))}
        </span>
      </span>
      {detail && <span className="truncate pl-4 text-caption text-fg-faint">{detail}</span>}
    </>
  );
  return thread ? (
    <Tooltip label="Open this worker's thread">
      <button
        type="button"
        onClick={() => selectThread(thread)}
        className="flex min-w-0 flex-col gap-0.5 rounded-lg px-2 py-1 text-left hover:bg-fill-subtle active:bg-fill-strong"
      >
        {body}
      </button>
    </Tooltip>
  ) : (
    <div className="flex min-w-0 flex-col gap-0.5 px-2 py-1">{body}</div>
  );
}

/** Every worker of a run, scrolled inside a bounded height: a click opens the worker's thread. */
export function WorkflowWorkers({ run, threadId }: { run: WorkflowRun; threadId: string }) {
  const now = useNow(isLive(run));
  const { host } = splitThreadKey(threadId);
  if (!run.workers.length) return <p className="px-2 py-1 text-caption text-fg-faint">No workers yet.</p>;
  return (
    <div
      className="flex max-h-[min(220px,30vh)] flex-col gap-0.5 overflow-y-auto"
      role="group"
      aria-label="Workers"
    >
      {run.workers.map((worker) => (
        <WorkerLine key={worker.seq} worker={worker} thread={workerThread(host, worker)} now={now} />
      ))}
    </div>
  );
}

function ResumeButton({ run, threadId }: { run: WorkflowRun; threadId: string }) {
  const [pending, setPending] = useState(false);
  return (
    <button
      type="button"
      disabled={pending}
      className="shrink-0 rounded-sm px-1.5 py-0.5 text-accent hover:bg-fill disabled:opacity-50"
      onClick={async () => {
        setPending(true);
        try {
          await resumeWorkflow(threadId, run.id);
        } finally {
          setPending(false);
        }
      }}
    >
      {pending ? "Resuming…" : "Resume"}
    </button>
  );
}

function RetryButton({ run, threadId }: { run: WorkflowRun; threadId: string }) {
  const [pending, setPending] = useState(false);
  return (
    <Tooltip label="Run the failed and unfinished workers again; finished ones are kept">
      <button
        type="button"
        disabled={pending}
        aria-label={`Retry ${runTitle(run)}`}
        className="shrink-0 rounded-sm px-1.5 py-0.5 text-accent hover:bg-fill disabled:opacity-50"
        onClick={async () => {
          setPending(true);
          try {
            await retryWorkflow(threadId, run.id);
          } finally {
            setPending(false);
          }
        }}
      >
        {pending ? "Retrying…" : "Retry"}
      </button>
    </Tooltip>
  );
}

function StopButton({ run, threadId }: { run: WorkflowRun; threadId: string }) {
  const stopping = useWorkflows((s) => s.stopping[run.id] === true);
  return (
    <Tooltip label={stopping ? "Stopping…" : "Stop this workflow and its workers"}>
      <button
        type="button"
        aria-label={`Stop ${runTitle(run)}`}
        disabled={stopping}
        onClick={() => void stopWorkflow(threadId, run.id)}
        className="flex size-[22px] shrink-0 items-center justify-center rounded-sm text-fg-faint hover:bg-fill hover:text-fg active:bg-fill-strong disabled:opacity-50"
      >
        {stopping ? <Spinner size={10} /> : <X size={12} strokeWidth={2.5} />}
      </button>
    </Tooltip>
  );
}

/**
 * The row above the composer for a run that goes: its name, phase, workers done and elapsed time on
 * one line, with Stop beside it. Opened, it lists the phases and every worker.
 */
export function WorkflowStatusRow({ run, threadId }: { run: WorkflowRun; threadId: string }) {
  const [open, setOpen] = useState(false);
  const now = useNow(true);
  const detailsId = `workflow-${run.id}`;
  return (
    <div className="flex flex-col gap-0.5">
      <div className="flex min-w-0 items-center gap-0.5">
        <button
          type="button"
          aria-expanded={open}
          aria-controls={open ? detailsId : undefined}
          onClick={() => setOpen(!open)}
          className="flex h-7 min-w-0 flex-1 items-center gap-1.5 rounded-lg px-2 text-left text-caption text-fg-muted hover:bg-fill-subtle active:bg-fill-strong"
        >
          <ChevronDown
            size={12}
            aria-hidden
            className={cx("shrink-0 text-fg-faint transition-transform duration-200", !open && "-rotate-90")}
          />
          <Workflow size={12} aria-hidden className="shrink-0 text-fg-faint" />
          <span className="min-w-0 truncate">{runLine(run, now)}</span>
          {run.driven ? (
            <Spinner size={10} className="shrink-0 text-accent" />
          ) : (
            <span className="shrink-0 text-fg-faint">· Awaiting resume</span>
          )}
        </button>
        {!run.driven && <ResumeButton run={run} threadId={threadId} />}
        <StopButton run={run} threadId={threadId} />
      </div>
      {open && (
        <div id={detailsId} className="flex flex-col gap-1 pb-1 pl-2">
          <PhaseSteps run={run} />
          <WorkflowWorkers run={run} threadId={threadId} />
        </div>
      )}
    </div>
  );
}

/** The run's last log line, or why it ended: the one thing worth a line besides its progress. */
function noteOf(run: WorkflowRun): { text: string; bad: boolean } | undefined {
  if (run.error && run.status !== "done") return { text: run.error, bad: true };
  const last = run.logs[run.logs.length - 1]?.text;
  return last ? { text: last, bad: false } : undefined;
}

/**
 * The card in the chat where a thread started a run (the `run_workflow` call's row): status, phases,
 * progress, elapsed time, Stop while it goes, and the workers behind a click. It reads the run from the
 * store, so it moves as the engine reports; a run the app has not been told of draws nothing.
 */
export function WorkflowCard({ threadId, runId }: { threadId: string; runId: string }) {
  const runs = useThreadRuns(threadId);
  const run = runs.find((r) => r.id === runId);
  const [open, setOpen] = useState(false);
  const live = run ? isLive(run) : false;
  const now = useNow(live);
  if (!run) return null;
  const note = noteOf(run);
  const detailsId = `workflow-card-${run.id}`;
  return (
    <section
      aria-label={`Workflow ${runTitle(run)}`}
      className="rc-expand mt-1.5 mb-1 ml-[26px] flex flex-col gap-1 rounded-xl border border-border bg-fill-subtle px-1 py-1.5"
    >
      <div className="flex min-w-0 items-center gap-1.5 px-2 text-caption text-fg-muted">
        <Workflow size={12} aria-hidden className="shrink-0 text-fg-faint" />
        <span className="min-w-0 truncate font-medium text-fg">{runTitle(run)}</span>
        <span
          className={cx(
            "shrink-0",
            run.status === "done" && "text-success",
            (run.status === "failed" || run.status === "timed_out") && "text-danger",
          )}
        >
          · {live && !run.driven ? "Awaiting resume" : statusWord(run.status)}
        </span>
        <span className="ml-auto shrink-0 text-fg-faint">{formatDuration(elapsedMs(run, now))}</span>
        {live && !run.driven && <ResumeButton run={run} threadId={threadId} />}
        {live && <StopButton run={run} threadId={threadId} />}
        {canRetry(run) && <RetryButton run={run} threadId={threadId} />}
      </div>
      <div className="px-2">
        <PhaseSteps run={run} />
      </div>
      <p className="px-2 text-caption text-fg-muted">{progressText(run)}</p>
      {run.usage && (
        <p className={cx("px-2 text-caption", run.usage.reached ? "text-warning" : "text-fg-faint")}>
          {runUsageText(run.usage)}
        </p>
      )}
      {note && (
        <p className={cx("truncate px-2 text-caption", note.bad ? "text-danger" : "text-fg-faint")}>
          {note.text}
        </p>
      )}
      <button
        type="button"
        aria-expanded={open}
        aria-controls={open ? detailsId : undefined}
        onClick={() => setOpen(!open)}
        className="flex h-6 w-fit items-center gap-1 rounded-lg px-2 text-caption text-fg-muted hover:bg-fill-subtle active:bg-fill-strong"
      >
        <ChevronDown
          size={12}
          aria-hidden
          className={cx("text-fg-faint transition-transform duration-200", !open && "-rotate-90")}
        />
        {run.workers.length === 1 ? "1 worker" : `${run.workers.length} workers`}
      </button>
      {open && (
        <div id={detailsId}>
          <WorkflowWorkers run={run} threadId={threadId} />
        </div>
      )}
    </section>
  );
}
