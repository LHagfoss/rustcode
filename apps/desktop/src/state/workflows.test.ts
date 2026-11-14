import { afterEach, expect, test } from "bun:test";
import { AcpConnection, type JsonObject } from "../acp/jsonrpc";
import { parseWorkflowRun, parseWorkflowRuns, WorkflowMethod, type WorkflowRun } from "../acp/workflows";
import { resetForTests, useApp, watchMachineConnection } from "./store";
import { emptyThread, isContinueMessage, WORKFLOW_MARK, wakeLine } from "./thread";
import { runIdOf } from "./toolNames";
import {
  currentPhase,
  elapsedMs,
  endNotice,
  liveRuns,
  loadWorkflowRuns,
  onWorkflowChanged,
  phaseSteps,
  progressText,
  resetWorkflows,
  runLine,
  runTitle,
  shouldWake,
  statusWord,
  stopWorkflow,
  upsertRun,
  useWorkflows,
  WAKE_WINDOW_MS,
  workerThread,
} from "./workflows";

afterEach(() => {
  resetForTests();
  resetWorkflows();
});

/** A run as the engine's summary says it (snake_case), running by default. */
function wire(over: JsonObject = {}): JsonObject {
  return {
    run_id: "wf-1-aaaaaaaa",
    thread: "t1",
    name: "review",
    status: "running",
    started_at: 1_000,
    finished_at: null,
    resumes: 0,
    limits: { concurrency: 3, call_budget: 40, timeout_minutes: 60 },
    calls: { total: 3, running: 1, done: 1, failed: 1 },
    phases: [
      { title: "review", started_at: 1_000, workers: 2, running: 0, done: 1, failed: 1 },
      { title: "verify", started_at: 2_000, workers: 1, running: 1, done: 0, failed: 0 },
    ],
    workers: [
      {
        seq: 0,
        phase: "review",
        state: "done",
        prompt: "Review a.rs",
        session_id: "s-a",
        link_id: "l-a",
        started_at: 1_000,
        finished_at: 1_500,
      },
      {
        seq: 1,
        phase: "review",
        state: "failed",
        prompt: "Review b.rs",
        session_id: "s-b",
        link_id: "l-b",
        started_at: 1_000,
        finished_at: 1_800,
        error: "the worker failed",
      },
      { seq: 2, phase: "verify", state: "running", prompt: "Verify", started_at: 2_000 },
    ],
    logs: [{ at: 1_200, text: "reviewed 2 files" }],
    has_result: false,
    woken: false,
    driven: true,
    ...over,
  };
}

const run = (over: JsonObject = {}): WorkflowRun => {
  const parsed = parseWorkflowRun(wire(over));
  if (!parsed) throw new Error("run did not parse");
  return parsed;
};

test("a run is read from the engine's summary, and a broken one is dropped", () => {
  const parsed = run();
  expect(parsed).toMatchObject({
    id: "wf-1-aaaaaaaa",
    name: "review",
    status: "running",
    startedAt: 1_000,
    limits: { concurrency: 3, callBudget: 40, timeoutMinutes: 60 },
    calls: { total: 3, running: 1, done: 1, failed: 1 },
    driven: true,
    woken: false,
  });
  expect(parsed.phases.map((p) => p.title)).toEqual(["review", "verify"]);
  expect(parsed.workers[2]).toMatchObject({ state: "running", sessionId: undefined });
  expect(parsed.workers[1]).toMatchObject({ sessionId: "s-b", error: "the worker failed" });
  expect(parseWorkflowRun({ ...wire(), status: "paused" })).toBeUndefined();
  expect(parseWorkflowRun({ status: "done" })).toBeUndefined();
  expect(parseWorkflowRuns({ runs: [wire(), { nope: 1 }] })).toHaveLength(1);
});

test("the labels say where a run is", () => {
  const r = run();
  expect(runTitle(r)).toBe("review");
  expect(runTitle(run({ name: null }))).toBe("Workflow");
  expect(currentPhase(r)).toBe("verify");
  expect(statusWord("timed_out")).toBe("Timed out");
  expect(statusWord("cancelled")).toBe("Stopped");
  expect(progressText(r)).toBe("1 of 3 workers done · 1 running · 1 failed");
  expect(progressText(run({ calls: { total: 0, running: 0, done: 0, failed: 0 } }))).toBe("no workers yet");
  expect(elapsedMs(r, 126_000)).toBe(125_000);
  expect(elapsedMs(run({ finished_at: 4_000, status: "done" }), 999_999)).toBe(3_000);
  expect(runLine(r, 126_000)).toBe("review · verify · 1 of 3 workers done · 1 running · 1 failed · 2m 5s");
  expect(workerThread("", r.workers[0]!)).toBe("s-a");
  expect(workerThread("gw-1", r.workers[0]!)).toBe("gw-1/s-a");
  expect(workerThread("", r.workers[2]!)).toBeUndefined();
});

test("phases are steps: the last one is active while the run goes and failed when it ended badly", () => {
  expect(phaseSteps(run())).toEqual([
    { title: "review", state: "done", detail: "1/2 · 1 failed" },
    { title: "verify", state: "active", detail: "0/1 · 1 running" },
  ]);
  expect(phaseSteps(run({ status: "done" })).map((s) => s.state)).toEqual(["done", "done"]);
  expect(phaseSteps(run({ status: "cancelled" })).map((s) => s.state)).toEqual(["done", "failed"]);
  expect(phaseSteps(run({ phases: [] }))).toEqual([]);
});

test("a run that ended is announced to its thread once, inside an hour", () => {
  const ended = run({ status: "done", finished_at: 10_000_000 });
  expect(shouldWake(ended, 10_000_000 + 1_000)).toBe(true);
  expect(shouldWake(ended, 10_000_000 + WAKE_WINDOW_MS + 1)).toBe(false);
  expect(shouldWake(run(), 5_000)).toBe(false);
  expect(shouldWake({ ...ended, woken: true }, 10_000_001)).toBe(false);
  expect(liveRuns([run(), ended])).toHaveLength(1);
});

test("the note a thread gets is one marked line for the row, then what an agent needs", () => {
  const text = endNotice(
    run({ status: "failed", finished_at: 126_000, error: "the call budget is spent", has_result: false }),
  );
  expect(isContinueMessage(text)).toBe(true);
  expect(text.endsWith(WORKFLOW_MARK)).toBe(true);
  expect(wakeLine(text)).toBe("Workflow review: failed · 3 workers · 2m 5s");
  expect(text).toContain("1 worker failed.");
  expect(text).toContain("Error: the call budget is spent");
  expect(text).toContain("workflow_status (run_id wf-1-aaaaaaaa)");
  expect(endNotice(run({ status: "done", finished_at: 2_000, has_result: true }))).toContain(
    "Read the result with workflow_status",
  );
});

test("a run_workflow answer names its run for the card", () => {
  expect(runIdOf('{"run_id":"wf-19b2c3d4e5f-0a1b2c3d","status":"running"}')).toBe("wf-19b2c3d4e5f-0a1b2c3d");
  expect(runIdOf('{"status":"running"}')).toBeUndefined();
  expect(runIdOf(undefined)).toBeUndefined();
});

test("the newest runs of a thread are kept, a run is replaced by its next summary", () => {
  upsertRun("t1", run());
  upsertRun("t1", run({ run_id: "wf-2-bbbbbbbb", started_at: 5_000 }));
  upsertRun("t1", run({ status: "done", finished_at: 3_000 }));
  const runs = useWorkflows.getState().runs.t1 ?? [];
  expect(runs.map((r) => [r.id, r.status])).toEqual([
    ["wf-2-bbbbbbbb", "running"],
    ["wf-1-aaaaaaaa", "done"],
  ]);
});

/** A connection that answers the workflow methods and records what it was asked. */
function engine(answer: (method: string, params: JsonObject) => JsonObject) {
  const calls: { method: string; params: JsonObject }[] = [];
  const conn: AcpConnection = new AcpConnection((line) => {
    const frame = JSON.parse(line) as JsonObject;
    const method = frame.method as string;
    const params = frame.params as JsonObject;
    calls.push({ method, params });
    queueMicrotask(() =>
      conn.receive(JSON.stringify({ jsonrpc: "2.0", id: frame.id, result: answer(method, params) })),
    );
  });
  return { conn, calls };
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

test("an ended run tells its thread once, even when two clients hear of it", async () => {
  let claimed = false;
  const { conn, calls } = engine((method): JsonObject => {
    if (method !== WorkflowMethod.wake) return { runs: [] };
    const first = !claimed;
    claimed = true;
    return { claimed: first };
  });
  watchMachineConnection("machine", conn);
  const key = "machine/t1";
  useApp.setState({ threads: { [key]: { ...emptyThread(key, "/tmp"), loaded: true, running: true } } });
  const ended = {
    status: "done",
    finished_at: Date.now(),
    calls: { total: 2, running: 0, done: 2, failed: 0 },
  };
  const params = { sessionId: "t1", run: wire(ended) };
  onWorkflowChanged(params, "machine");
  onWorkflowChanged(params, "machine");
  await settle();
  await settle();
  const queued = useApp.getState().threads[key]?.queue.map((q) => q.text) ?? [];
  expect(queued).toHaveLength(1);
  expect(wakeLine(queued[0] ?? "")).toStartWith("Workflow review: done · 2 workers");
  expect(calls.filter((c) => c.method === WorkflowMethod.wake)[0]?.params).toEqual({
    sessionId: "t1",
    runId: "wf-1-aaaaaaaa",
  });
  // The run is kept, told.
  expect(useWorkflows.getState().runs[key]?.[0]?.woken).toBe(true);
  // A run still going tells nobody.
  onWorkflowChanged({ sessionId: "t1", run: wire({ run_id: "wf-3-cccccccc" }) }, "machine");
  await settle();
  expect(useApp.getState().threads[key]?.queue).toHaveLength(1);
  conn.close("test finished");
});

test("a thread's runs load once from the engine, and an old ended run is not announced", async () => {
  const { conn, calls } = engine(
    (method): JsonObject =>
      method === WorkflowMethod.list
        ? {
            runs: [
              wire(),
              wire({ run_id: "wf-0-00000000", status: "done", finished_at: 2_000, woken: false }),
            ],
          }
        : { claimed: true },
  );
  watchMachineConnection("machine", conn);
  const key = "machine/t1";
  useApp.setState({ threads: { [key]: { ...emptyThread(key, "/tmp"), loaded: true, running: true } } });
  await loadWorkflowRuns(key);
  await loadWorkflowRuns(key);
  await settle();
  // Connecting asked to hear of every run (no thread); the thread's own list was asked once.
  expect(calls.filter((c) => c.method === WorkflowMethod.list && !c.params.sessionId)).toHaveLength(1);
  expect(calls.filter((c) => c.method === WorkflowMethod.list && c.params.sessionId === "t1")).toHaveLength(
    1,
  );
  expect(useWorkflows.getState().runs[key]).toHaveLength(2);
  // Finished in 1970: past the window, so no wake is asked for.
  expect(calls.some((c) => c.method === WorkflowMethod.wake)).toBe(false);
  conn.close("test finished");
});

test("Stop asks the engine and marks the run ended; a refusal is told", async () => {
  const { conn, calls } = engine(
    (method): JsonObject =>
      method === WorkflowMethod.stop
        ? { run: { run_id: "wf-1-aaaaaaaa", status: "cancelled", finished_at: 9_000 } }
        : {},
  );
  watchMachineConnection("machine", conn);
  const key = "machine/t1";
  upsertRun(key, run());
  const stopping = stopWorkflow(key, "wf-1-aaaaaaaa");
  expect(useWorkflows.getState().stopping["wf-1-aaaaaaaa"]).toBe(true);
  await stopping;
  expect(calls.find((c) => c.method === WorkflowMethod.stop)?.params).toEqual({
    sessionId: "t1",
    runId: "wf-1-aaaaaaaa",
  });
  const stopped = useWorkflows.getState().runs[key]?.[0];
  expect(stopped?.status).toBe("cancelled");
  // Its workers stay in the list: only how it ended is taken from the answer.
  expect(stopped?.workers).toHaveLength(3);
  expect(useWorkflows.getState().stopping["wf-1-aaaaaaaa"]).toBeUndefined();
  conn.close("test finished");

  const refused = engine(() => ({
    error: { code: "failed", message: "no run wf-1-aaaaaaaa in this thread" },
  }));
  watchMachineConnection("other", refused.conn);
  upsertRun("other/t1", run());
  await stopWorkflow("other/t1", "wf-1-aaaaaaaa");
  expect(useWorkflows.getState().runs["other/t1"]?.[0]?.status).toBe("running");
  expect(useWorkflows.getState().stopping["wf-1-aaaaaaaa"]).toBeUndefined();
  expect(useApp.getState().toasts.some((t) => t.text.includes("Could not stop the workflow"))).toBe(true);
  refused.conn.close("test finished");
});
