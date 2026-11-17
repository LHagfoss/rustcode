// The mock engine's Loops (issue 3892), as `_rustcode/loops/*` and the scheduled run/delete calls
// answer them: three loops, one healthy with a thread left open for review, one whose last run was
// skipped while the app was closed, and one the daemon paused after three failed runs.

import { type JsonObject, str } from "./jsonrpc";

const HOUR = 3_600_000;
const ago = (hours: number) => new Date(Date.now() - hours * HOUR).toISOString();
const ahead = (hours: number) => new Date(Date.now() + hours * HOUR).toISOString();

const cron = (expression: string): JsonObject => ({
  kind: "cron",
  expression,
  timezone: "Europe/Oslo",
  misfire_policy: "run_once",
});

const done = (id: string, hours: number, threadId?: string): JsonObject => ({
  id,
  state: "succeeded",
  scheduledAt: ago(hours),
  startedAt: ago(hours),
  finishedAt: ago(hours - 0.1),
  ...(threadId ? { threadId } : {}),
});

const failed = (id: string, hours: number, error: string): JsonObject => ({
  id,
  state: "failed",
  scheduledAt: ago(hours),
  startedAt: ago(hours),
  finishedAt: ago(hours - 0.05),
  error,
  errorClass: "permanent",
});

const HARNESS_OFF = "Claude Code cannot run a thread: it is turned off in Settings › Harnesses";

function seed(): JsonObject[] {
  return [
    {
      id: "loop-ci",
      name: "CI watch",
      paused: false,
      schedule: cron("0 */4 * * *"),
      workspace: "/Users/me/code/rustcode",
      prompt: "Check CI on main. Tell me only if something broke, with the failing job and its first error.",
      harness: "codex",
      model: null,
      effort: "medium",
      readOnly: true,
      nextDueAt: ahead(1.5),
      runs: [done("run-ci-3", 2.5, "mock-loop-ci-3"), done("run-ci-2", 6.5), done("run-ci-1", 10.5)],
    },
    {
      id: "loop-issues",
      name: "New issues digest",
      paused: false,
      schedule: cron("0 9 * * 1-5"),
      workspace: "/Users/me/code/rustcode",
      prompt: "Summarize the GitHub issues opened since yesterday morning, grouped by area.",
      harness: null,
      model: null,
      effort: null,
      readOnly: true,
      nextDueAt: ahead(20),
      runs: [
        {
          id: "run-issues-2",
          state: "cancelled",
          scheduledAt: ago(27),
          errorClass: "cancelled",
          skipped: "RustCode was not open, so the run was skipped",
        },
        done("run-issues-1", 51),
      ],
    },
    {
      id: "loop-deps",
      name: "Dependency audit",
      paused: true,
      schedule: cron("0 8 * * 1"),
      workspace: "/Users/me/code/site",
      prompt: "Run the dependency audit and list anything with a known advisory.",
      harness: "claude-code",
      model: null,
      effort: null,
      readOnly: true,
      nextDueAt: ahead(100),
      runs: [
        failed("run-deps-3", 3, HARNESS_OFF),
        failed("run-deps-2", 27, HARNESS_OFF),
        failed("run-deps-1", 51, HARNESS_OFF),
      ],
    },
  ];
}

export class MockLoops {
  private loops: JsonObject[] = seed();
  private counter = 0;

  /** `_rustcode/loops/*`. */
  answer(method: string, params: JsonObject): JsonObject {
    switch (method) {
      case "_rustcode/loops/list":
        return { daemonRunning: true, loops: this.loops };
      case "_rustcode/loops/save":
        return { loop: this.save(params) };
      case "_rustcode/loops/set_paused": {
        const id = str(params.id) ?? "";
        const paused = params.paused === true;
        this.loops = this.loops.map((loop) => (loop.id === id ? { ...loop, paused } : loop));
        return { id, paused };
      }
      case "_rustcode/loops/start_daemon":
        return { daemonRunning: true, pid: 4242 };
      default:
        return {};
    }
  }

  /** `_rustcode/scheduled/delete`: a loop leaves the list. */
  forget(id: string): JsonObject {
    this.loops = this.loops.filter((loop) => loop.id !== id);
    return { id, deleted: true };
  }

  /** `_rustcode/scheduled/run`: a loop shows a run going. */
  run(id: string): JsonObject {
    const at = new Date().toISOString();
    this.loops = this.loops.map((loop) =>
      loop.id === id
        ? {
            ...loop,
            runs: [
              { id: `run-${id}-${++this.counter}`, state: "running", scheduledAt: at, startedAt: at },
              ...(Array.isArray(loop.runs) ? loop.runs : []),
            ],
          }
        : loop,
    );
    return { id, state: "claimed" };
  }

  private save(params: JsonObject): JsonObject {
    const id = str(params.id) || `loop-new-${++this.counter}`;
    const existing = this.loops.find((loop) => loop.id === id);
    const schedule =
      params.schedule && typeof params.schedule === "object" ? params.schedule : cron("0 */4 * * *");
    const saved: JsonObject = {
      paused: false,
      runs: [],
      ...existing,
      id,
      name: str(params.name) ?? "Loop",
      workspace: str(params.workspace) ?? "",
      prompt: str(params.prompt) ?? "",
      schedule: { ...(schedule as JsonObject), misfire_policy: "run_once" },
      harness: str(params.harness) ?? null,
      model: str(params.model) ?? null,
      effort: str(params.effort) ?? null,
      readOnly: params.readOnly !== false,
      nextDueAt: ahead(4),
    };
    this.loops = existing
      ? this.loops.map((loop) => (loop.id === id ? saved : loop))
      : [...this.loops, saved];
    return saved;
  }
}
