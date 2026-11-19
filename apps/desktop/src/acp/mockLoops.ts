// The mock engine's Scheduled Jobs (issues 3892, 3937), as `_rustcode/loops/*` and the scheduled
// run/delete calls answer them: one healthy with a thread left open for review, one whose last run
// was skipped while the app was closed, one the daemon paused after three failed runs, one in
// script mode whose last run printed nothing; one more runs only from its webhook.

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
      id: "loop-red-main",
      name: "Red main alert",
      paused: false,
      schedule: cron("*/30 * * * *"),
      workspace: "/Users/me/code/rustcode",
      prompt: "Main is red. Find the failing job and its first error, and say who should look.",
      script: "gh run list --branch main --status failure --limit 3",
      harness: null,
      model: null,
      effort: null,
      readOnly: true,
      nextDueAt: ahead(0.3),
      runs: [
        { ...done("run-red-2", 0.2), quiet: true, silent: true },
        done("run-red-1", 0.7, "mock-loop-ci-3"),
      ],
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
      id: "loop-triage",
      name: "Issue triage",
      paused: false,
      schedule: { kind: "manual", misfire_policy: "run_once" },
      workspace: "/Users/me/code/rustcode",
      prompt: "Label the new issue by area, and ask for a reproduction if it has none.",
      harness: null,
      model: null,
      effort: null,
      readOnly: true,
      webhook: true,
      nextDueAt: null,
      runs: [{ ...done("run-triage-1", 5), input: '{"action":"opened"}' }],
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
      case "_rustcode/loops/save": {
        const id = str(params.id);
        const before = this.loops.find((loop) => loop.id === id);
        const saved = this.save(params);
        // A new webhook secret is answered once, as the engine does.
        const made = params.webhook === true && (before?.webhook !== true || params.rotateWebhook === true);
        if (!made) return { loop: saved };
        const secret = "5e3c".repeat(16);
        const url = `http://127.0.0.1:43871/hooks/${str(saved.id)}/${secret}`;
        return { loop: saved, webhook: { url, localUrl: url, public: false, secret } };
      }
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
      name: str(params.name) ?? "Job",
      workspace: str(params.workspace) ?? "",
      prompt: str(params.prompt) ?? "",
      schedule: { ...(schedule as JsonObject), misfire_policy: "run_once" },
      harness: str(params.harness) ?? null,
      account: str(params.account) ?? null,
      threadId: str(params.threadId) ?? null,
      script: str(params.script) ?? null,
      model: str(params.model) ?? null,
      effort: str(params.effort) ?? null,
      readOnly: params.readOnly !== false,
      webhook: params.webhook === true,
      nextDueAt: (schedule as JsonObject).kind === "manual" ? null : ahead(4),
    };
    this.loops = existing
      ? this.loops.map((loop) => (loop.id === id ? saved : loop))
      : [...this.loops, saved];
    return saved;
  }
}
