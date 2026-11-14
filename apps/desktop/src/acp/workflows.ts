// Durable workflow runs as the engine reports them (docs/workflows.md, issue 3394): the shapes and the
// parser of `_rustcode/workflows/*`. The engine drives a run and keeps it under the thread that
// started it; the app lists a thread's runs, hears each change as a notification carrying the run's
// summary, stops a run, and claims the one wake that tells the starting thread the run ended.

import { arr, type Json, num, obj, str } from "./jsonrpc";

export const WorkflowMethod = {
  list: "_rustcode/workflows/list",
  stop: "_rustcode/workflows/stop",
  wake: "_rustcode/workflows/wake",
  changed: "_rustcode/workflows/changed",
} as const;

export type WorkflowStatus = "running" | "done" | "failed" | "timed_out" | "cancelled";
export type WorkerState = "running" | "done" | "failed";

/** One `agent()` call: a delegate thread of the thread that started the run. */
export interface WorkflowWorker {
  seq: number;
  phase?: string;
  state: WorkerState;
  /** The prompt, cut by the engine to a preview. */
  prompt: string;
  harness?: string;
  model?: string;
  /** The worker's thread; absent while it waits for a delegate slot. */
  sessionId?: string;
  linkId?: string;
  startedAt: number;
  finishedAt?: number;
  error?: string;
}

export interface WorkflowPhase {
  title: string;
  startedAt: number;
  workers: number;
  running: number;
  done: number;
  failed: number;
}

export interface WorkflowRun {
  id: string;
  /** The thread that started the run, as the machine it runs on knows it. */
  thread: string;
  /** The file in `.rustcode/workflows/`, or none for an inline script. */
  name?: string;
  status: WorkflowStatus;
  startedAt: number;
  finishedAt?: number;
  /** How many times a restart picked the run up again. */
  resumes: number;
  limits: { concurrency: number; callBudget: number; timeoutMinutes: number };
  calls: { total: number; running: number; done: number; failed: number };
  phases: WorkflowPhase[];
  workers: WorkflowWorker[];
  logs: { at: number; text: string }[];
  error?: string;
  /** Whether it has a final result; `workflow_status` reads it. */
  hasResult: boolean;
  /** Whether the starting thread was told the run ended. */
  woken: boolean;
  /** False for a run a restart left running that nothing drives yet. */
  driven: boolean;
}

const STATUSES: readonly WorkflowStatus[] = ["running", "done", "failed", "timed_out", "cancelled"];
const WORKER_STATES: readonly WorkerState[] = ["running", "done", "failed"];

function count(value: unknown): number {
  const n = num(value);
  return n === undefined ? 0 : Math.max(0, n);
}

function parseWorker(value: Json | undefined): WorkflowWorker | undefined {
  const o = obj(value);
  const state = WORKER_STATES.find((s) => s === o?.state);
  if (!o || !state) return undefined;
  return {
    seq: count(o.seq),
    phase: str(o.phase),
    state,
    prompt: str(o.prompt) ?? "",
    harness: str(o.harness),
    model: str(o.model),
    sessionId: str(o.session_id),
    linkId: str(o.link_id),
    startedAt: count(o.started_at),
    finishedAt: num(o.finished_at),
    error: str(o.error),
  };
}

function parsePhase(value: Json | undefined): WorkflowPhase | undefined {
  const o = obj(value);
  const title = str(o?.title);
  if (!o || title === undefined) return undefined;
  return {
    title,
    startedAt: count(o.started_at),
    workers: count(o.workers),
    running: count(o.running),
    done: count(o.done),
    failed: count(o.failed),
  };
}

/** A run as `_rustcode/workflows/list` and `…/changed` carry it (the engine's snake_case). */
export function parseWorkflowRun(value: Json | undefined): WorkflowRun | undefined {
  const o = obj(value);
  const id = str(o?.run_id);
  const status = STATUSES.find((s) => s === o?.status);
  if (!o || !id || !status) return undefined;
  const limits = obj(o.limits);
  const calls = obj(o.calls);
  return {
    id,
    thread: str(o.thread) ?? "",
    name: str(o.name),
    status,
    startedAt: count(o.started_at),
    finishedAt: num(o.finished_at),
    resumes: count(o.resumes),
    limits: {
      concurrency: count(limits?.concurrency),
      callBudget: count(limits?.call_budget),
      timeoutMinutes: count(limits?.timeout_minutes),
    },
    calls: {
      total: count(calls?.total),
      running: count(calls?.running),
      done: count(calls?.done),
      failed: count(calls?.failed),
    },
    phases: arr(o.phases).flatMap((p) => parsePhase(p) ?? []),
    workers: arr(o.workers).flatMap((w) => parseWorker(w) ?? []),
    logs: arr(o.logs).flatMap((line) => {
      const l = obj(line);
      const text = str(l?.text);
      return l && text !== undefined ? [{ at: count(l.at), text }] : [];
    }),
    error: str(o.error),
    hasResult: o.has_result === true,
    woken: o.woken === true,
    driven: o.driven === true,
  };
}

export function parseWorkflowRuns(result: Json | undefined): WorkflowRun[] {
  return arr(obj(result)?.runs).flatMap((run) => parseWorkflowRun(run) ?? []);
}
