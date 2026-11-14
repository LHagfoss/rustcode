// Durable workflow runs in the app (docs/workflows.md, issue 3394). The engine drives a run and keeps it
// under the thread that started it; this module holds each thread's runs as the engine last said
// them (a list when the thread opens, a notification on every change), stops one, and tells the
// starting thread once when a run ended. The pure parts (labels, steps, the wake's words) are
// exported for the run card, the status row and the tests.

import { create } from "zustand";
import { type AcpConnection, type JsonObject, obj, str } from "../acp/jsonrpc";
import {
  parseWorkflowRun,
  parseWorkflowRuns,
  WorkflowMethod,
  type WorkflowRun,
  type WorkflowStatus,
  type WorkflowWorker,
} from "../acp/workflows";
import { count as plural } from "../lib/plural";
import { deliver } from "./host";
import { connectionFor, errorText, openThread, toast, useApp } from "./store";
import { formatDuration, withWorkflowMark } from "./thread";
import { splitThreadKey, threadKey } from "./threadKeys";

/** The newest runs kept per thread, as many as the engine lists. */
const MAX_RUNS = 20;
/** A run that ended longer ago than this is not announced to its thread when first seen. */
export const WAKE_WINDOW_MS = 60 * 60 * 1000;

interface WorkflowStore {
  /** A thread's runs by thread key (`state/threadKeys.ts`), newest first. */
  runs: Record<string, WorkflowRun[]>;
  /** Runs Stop was pressed on, until the engine says they ended. */
  stopping: Record<string, true>;
}

export const useWorkflows = create<WorkflowStore>(() => ({ runs: {}, stopping: {} }));

const NONE: readonly WorkflowRun[] = [];
const loaded = new Set<string>();
const waking = new Set<string>();

/** Forget everything (tests, a machine going away). */
export function resetWorkflows(): void {
  loaded.clear();
  waking.clear();
  useWorkflows.setState({ runs: {}, stopping: {} });
}

// ------------------------------------------------------------ pure

export const isLive = (run: Pick<WorkflowRun, "status">): boolean => run.status === "running";

/** The thread's runs still going. */
export const liveRuns = (runs: readonly WorkflowRun[] | undefined): WorkflowRun[] =>
  (runs ?? NONE).filter(isLive);

const STATUS_WORD: Record<WorkflowStatus, string> = {
  running: "Running",
  done: "Done",
  failed: "Failed",
  timed_out: "Timed out",
  cancelled: "Stopped",
};

export const statusWord = (status: WorkflowStatus): string => STATUS_WORD[status];

/** `review` for a run started by name, `Workflow` for an inline script. */
export const runTitle = (run: Pick<WorkflowRun, "name">): string => run.name ?? "Workflow";

/** How long the run has gone, or went. */
export function elapsedMs(run: Pick<WorkflowRun, "startedAt" | "finishedAt">, now: number): number {
  return Math.max(0, (run.finishedAt ?? now) - run.startedAt);
}

/** The phase the script is in now, or the last one it reached. */
export const currentPhase = (run: Pick<WorkflowRun, "phases">): string | undefined =>
  run.phases[run.phases.length - 1]?.title;

/** `5 of 8 workers done`, with the ones still running and the failures after it. */
export function progressText(run: Pick<WorkflowRun, "calls">): string {
  const { total, running, done, failed } = run.calls;
  if (!total) return "no workers yet";
  return [
    `${done} of ${plural(total, "worker")} done`,
    running ? `${running} running` : "",
    failed ? `${failed} failed` : "",
  ]
    .filter(Boolean)
    .join(" · ");
}

export type PhaseStepState = "done" | "active" | "failed";

/** Each phase as a step: the last is active while the run goes, failed when the run did not finish. */
export function phaseSteps(
  run: Pick<WorkflowRun, "phases" | "status">,
): { title: string; state: PhaseStepState; detail: string }[] {
  return run.phases.map((phase, index) => {
    const last = index === run.phases.length - 1;
    const state: PhaseStepState = !last
      ? "done"
      : run.status === "running"
        ? "active"
        : run.status === "done"
          ? "done"
          : "failed";
    const detail = [
      phase.workers ? `${phase.done}/${phase.workers}` : "",
      phase.running ? `${phase.running} running` : "",
      phase.failed ? `${phase.failed} failed` : "",
    ]
      .filter(Boolean)
      .join(" · ");
    return { title: phase.title, state, detail };
  });
}

/** A worker's thread key on the machine the run is on, when it has started. */
export function workerThread(host: string, worker: Pick<WorkflowWorker, "sessionId">): string | undefined {
  return worker.sessionId ? threadKey(host, worker.sessionId) : undefined;
}

/** The status row's one line: `review · verify · 3 of 8 workers done · 2m 5s`. */
export function runLine(run: WorkflowRun, now: number): string {
  return [runTitle(run), currentPhase(run) ?? "", progressText(run), formatDuration(elapsedMs(run, now))]
    .filter(Boolean)
    .join(" · ");
}

/** Whether the starting thread is yet to be told this run ended. */
export function shouldWake(run: WorkflowRun, now: number): boolean {
  return !isLive(run) && !run.woken && now - (run.finishedAt ?? 0) <= WAKE_WINDOW_MS;
}

/**
 * What the starting thread is told when its run ended: one line the notice row shows, then what an
 * agent needs to carry on (the error, and how to read the result). Marked as the app's own.
 */
export function endNotice(run: WorkflowRun): string {
  const lines = [
    `Workflow ${runTitle(run)}: ${statusWord(run.status).toLowerCase()} · ${plural(run.calls.total, "worker")} · ${formatDuration(elapsedMs(run, 0))}`,
  ];
  if (run.calls.failed) lines.push(`${plural(run.calls.failed, "worker")} failed.`);
  if (run.error && run.status !== "done") lines.push(`Error: ${run.error}`);
  lines.push(
    run.hasResult
      ? `Read the result with workflow_status (run_id ${run.id}).`
      : `Read the run with workflow_status (run_id ${run.id}).`,
  );
  return withWorkflowMark(lines.join("\n"));
}

// ------------------------------------------------------------ the store

/** Put `run` into the thread's list, replacing the run of the same id. */
export function upsertRun(key: string, run: WorkflowRun): void {
  useWorkflows.setState((state) => {
    const rest = (state.runs[key] ?? NONE).filter((r) => r.id !== run.id);
    const runs = [run, ...rest].sort((a, b) => b.startedAt - a.startedAt).slice(0, MAX_RUNS);
    const stopping = { ...state.stopping };
    if (!isLive(run)) delete stopping[run.id];
    return { runs: { ...state.runs, [key]: runs }, stopping };
  });
}

/** The thread's runs, newest first. */
export function useThreadRuns(key: string): readonly WorkflowRun[] {
  return useWorkflows((state) => state.runs[key] ?? NONE);
}

// ------------------------------------------------------------ the engine

/** Ask the machine to tell this connection of every run's changes. */
export async function subscribeWorkflows(conn: AcpConnection): Promise<void> {
  await conn.request(WorkflowMethod.list, {}, 15_000);
}

/** The thread's runs from the engine, once per thread and connection; an ended one is announced. */
export async function loadWorkflowRuns(key: string): Promise<void> {
  const { host, id } = splitThreadKey(key);
  const conn = connectionFor(host);
  if (!conn || loaded.has(key)) return;
  loaded.add(key);
  try {
    const runs = parseWorkflowRuns(await conn.request(WorkflowMethod.list, { sessionId: id }, 15_000));
    for (const run of runs) upsertRun(key, run);
    for (const run of runs) void wakeIfEnded(key, run);
  } catch {
    // An older engine has no workflows; ask again the next time the thread opens.
    loaded.delete(key);
  }
}

/** `_rustcode/workflows/changed`: one run moved. */
export function onWorkflowChanged(params: JsonObject, host = ""): void {
  const sessionId = str(params.sessionId);
  const run = parseWorkflowRun(params.run);
  if (!sessionId || !run) return;
  const key = threadKey(host, sessionId);
  upsertRun(key, run);
  void wakeIfEnded(key, run);
}

/**
 * Tell the thread its run ended, once: the engine hands the claim to one client, so a second window,
 * a reconnect or a reload cannot send it again. It goes after the turn the thread is in, like any
 * wake, and a thread this window has not opened is opened first.
 */
async function wakeIfEnded(key: string, run: WorkflowRun): Promise<void> {
  if (!shouldWake(run, Date.now()) || waking.has(run.id)) return;
  waking.add(run.id);
  try {
    await openThread(key);
    if (!useApp.getState().threads[key]?.loaded) return;
    const { host, id } = splitThreadKey(key);
    const conn = connectionFor(host);
    if (!conn) return;
    const answer = obj(await conn.request(WorkflowMethod.wake, { sessionId: id, runId: run.id }, 15_000));
    if (answer?.claimed !== true) return;
    upsertRun(key, { ...run, woken: true });
    deliver(key, endNotice(run), "", "");
  } catch {
    // Unclaimed, so the next change or the next open tries again.
  } finally {
    waking.delete(run.id);
  }
}

/** Stop a run: its script ends, no worker starts, and the ones running are interrupted. */
export async function stopWorkflow(key: string, runId: string): Promise<void> {
  const { host, id } = splitThreadKey(key);
  const conn = connectionFor(host);
  if (!conn) {
    toast("That machine is not connected, so the workflow could not be stopped.", "error");
    return;
  }
  useWorkflows.setState((state) => ({ stopping: { ...state.stopping, [runId]: true } }));
  try {
    const answer = obj(await conn.request(WorkflowMethod.stop, { sessionId: id, runId }, 30_000));
    const error = obj(answer?.error);
    if (error) throw new Error(str(error.message) ?? "The workflow could not be stopped.");
    const run = parseWorkflowRun(answer?.run);
    // What the answer settles is how it ended; the engine's next notification brings the rest.
    const known = useWorkflows.getState().runs[key]?.find((r) => r.id === runId);
    if (run)
      upsertRun(
        key,
        known
          ? { ...known, status: run.status, finishedAt: run.finishedAt, error: run.error, driven: run.driven }
          : run,
      );
  } catch (error) {
    useWorkflows.setState((state) => {
      const stopping = { ...state.stopping };
      delete stopping[runId];
      return { stopping };
    });
    toast(`Could not stop the workflow: ${errorText(error)}`, "error");
  }
}
