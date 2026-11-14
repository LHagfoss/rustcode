# Workflows

A workflow is a JavaScript script that fans work out to worker threads and
keeps its place on disk. It exists because `fan_out` starts threads but the
plan around them (review each file, then verify the findings, with a barrier
between) lives only in the orchestrating agent's context, and is lost to a
restart or a compaction. A workflow run holds that plan in a script and in a
folder instead (#3394).

An agent starts a run with `run_workflow`, reads it with `workflow_status` and
ends it with `stop_workflow`. The desktop app shows it as a card in the chat, a
row above the composer with Stop, and a list of its worker threads
(`docs/desktop.md`, *Workflow runs*), and tells the starting thread when it
ends.

## The script

The script is the body of an async function: it may `await`, and what it
`return`s (any JSON value) is the run's result.

```js
phase("review");
const reviews = await parallel(
  args.files.map((file) => () =>
    agent(`Review ${file} for bugs. Be specific.`, {
      schema: {
        type: "object",
        properties: { issues: { type: "array", items: { type: "string" } } },
        required: ["issues"],
      },
    }),
  ),
);

phase("verify");
const verdict = await agent(
  "Check these findings and drop the ones that are wrong:\n" + JSON.stringify(reviews),
  { harness: "codex" },
);
log("verified", reviews.length, "files");
return { reviews, verdict };
```

| Global | What it does |
| --- | --- |
| `agent(prompt, options?)` | Starts a normal delegate thread with `prompt` and resolves to its reply. Options: `schema` (a JSON Schema: the worker is asked for JSON, and the reply is parsed and checked against it), `harness`, `model`, `effort`, `title`. Rejects when the worker fails or its reply breaks the schema, so a script can `try`/`catch` it. |
| `parallel(items)` | `Promise.all` over an array of functions (called in order) or promises. Results come back in the array's order. The run's concurrency limit, not `parallel`, decides how many workers run at once. |
| `phase(title)` | Names the step the next calls belong to. Phases and the workers in them are what the run reports. |
| `log(...parts)` | Adds a line to the run's log (at most 200 are kept). |
| `args` | The JSON the run was started with. |

Without a `schema` the resolved value is the worker's reply text. A worker's
reply reaches the script cut at 8 KiB, like a `wait_session` answer, so ask for
short structured results and keep long output in files.

`schema` is checked for `type`, `enum`, `required`, `properties`, `items` and
`additionalProperties: false`. Other keywords still go to the worker as part of
the schema it is asked to follow, and are not checked.

Anything else is a plain error: an unknown option, an empty prompt, or a call
over the budget.

## The sandbox

The script runs in QuickJS (the `rquickjs` crate) with nothing added that
reaches outside it:

- **No filesystem or shell.** There is no `require`, no `import` of anything,
  no `std` or `os`, no `process`. Dynamic `import()` fails: the runtime has no
  module loader.
- **No network.** No `fetch`, `XMLHttpRequest`, `WebSocket`.
- **No clock and no randomness.** The context is built without `Date` and
  `performance`, there are no timers (`setTimeout` and friends), and
  `Math.random` is deleted. This is on purpose, not only caution: a script
  that cannot read the time or draw a number does the same thing every time,
  which is what lets a restarted run replay its calls and arrive at the same
  next call. If a script needs a value that changes, the caller passes it in
  `args`.
- **Bounded.** The runtime has a 64 MiB memory cap and a 512 KiB stack, and
  stops a script that is still computing when the run's timeout passes (a
  `while (true) {}` ends with "the script ran past its time limit").

The only way out of the sandbox is `agent`, `phase` and `log`. A worker is a
thread like any other, with its own sandbox, approvals and access; the
workflow does not widen what the user allowed.

### Why QuickJS

An embeddable engine was needed, and none was in the lockfile. QuickJS through
`rquickjs` was chosen over the pure-Rust `boa_engine` because it can be built
with exactly the intrinsics a script is allowed (leaving out `Date` and
`Performance` is a build choice, not a global to delete and hope), it has a
memory limit, a stack limit and an interrupt handler for a runaway loop, and
`rquickjs` supports `async` host functions natively, which is what `agent()`
is. It compiles from C like the bundled SQLite already does, and adds one
crate set (`rquickjs`, `rquickjs-core`, `rquickjs-sys`) with no modules and
no loader.

## Starting and reading a run

Both are tools of the `rustcode` MCP server, answered by the engine's hub, so
they reach any harness that has the orchestration tools (version 2 and up) and
need no app to be open. The built-in RustCode harness does not have them yet.
The workflow tools are three: `run_workflow`, `workflow_status` and
`stop_workflow`.

**`run_workflow`** takes `script` (inline, at most 64 KiB) or `name` (the file
`.rustcode/workflows/<name>.js` in the thread's project: letters, digits `-`,
`_` and `.`), and optionally `args`, `concurrency`, `call_budget` and
`timeout_minutes`. It answers at once with `{ run_id, status: "running", limits }`
and the run goes on in the background.

**`workflow_status`** takes a `run_id` and answers the run in full, or with no
`run_id` lists this thread's runs. `wait_ms` waits up to that long for the run
to end and answers sooner when it does (at most 25 minutes). The answer holds:

- `status`: `running`, `done`, `failed`, `timed_out` or `cancelled`, with `error`
  when it did not finish;
- `phases`: each `{ title, started_at, workers, running, done, failed }`;
- `workers`: each call's `{ seq, phase, state, prompt, harness, model,
  session_id, link_id, started_at, finished_at, error }`, so a worker's thread
  can be opened, read or interrupted like any delegate;
- `calls` (counts), `limits`, `elapsed_ms`, `resumes`, the last 20 `logs`;
- `result`, the script's return value. One over 96 KiB is not inlined: the
  answer says `result_truncated` and names `result_file`.

**`stop_workflow`** takes a `run_id` and ends the run: the script stops, no
further `agent()` call starts, and the workers still running are interrupted
(the same call `interrupt_session` makes). The run becomes `cancelled` and keeps
the calls it had finished. It answers with the run once it has wound down, or
with `stopping: true` when that takes more than 15 seconds. A run that already
ended is answered as it is. A run a restart left running that nothing drives is
ended on its record, so it is not resumed; its workers, if any are still alive,
are threads the user can stop like any other.

Workers are started with `start_session` as the thread that started the run, so
they **nest under it**, count against its delegate slots (`delegates-per-thread`,
`delegates-total`), and use the same approvals and access rules as a delegate
the agent starts by hand. When every slot is taken, the run waits and tries
again rather than failing.

## Durability

Everything is under the starting thread's folder,
`<config>/threads/<yyyy>/<mm>/<dd>/<thread id>/workflows/<run id>/`:

| File | What it holds |
| --- | --- |
| `run.json` | The record: status, phases, log lines, limits, result, error. Replaced atomically on every change. |
| `script.js` | The script as it was started, so a restart runs that one even if the named file changed. |
| `calls.jsonl` | One line per state change of each `agent()` call. The last line of a key wins; a torn last line is skipped. |
| `result.json` | The final result, written when the run is done. |

A call's **key** is a hash of its prompt and options plus the number of earlier
calls with the same hash. Two identical calls are two keys, and a call is found
again whatever order other calls settled in. When a run is picked up again:

- a call whose key finished is answered from the file with no thread started;
- a call whose worker was started but not seen to finish waits on that worker
  again (the start carries a stable `client_request_id`, so asking again finds
  the same thread rather than starting a second one);
- a call that failed, or has no record, runs as new. A failed call is not
  cached: the script may have caught the error, and a retry is more useful than
  replaying the failure;
- a call whose prompt or options changed has a different key and is a miss, so
  editing a script's tail and resuming keeps the work before it.

The script is deterministic, so the replayed calls are made in the order they
were the first time and the run reaches its first unfinished call. If a script
branches on which of several calls settled first, a resumed run can order later
calls differently; keying by hash and occurrence (not by position) keeps those
calls matched to their answers anyway.

**When a run resumes.** The engine resumes the runs a thread left `running`
when that thread registers with the engine's orchestrator hub (the thread is
opened or started on a harness with the orchestration tools). Until then
`workflow_status` reports the run as `running` with a note that it is not being
driven. Replay needs no worker to be alive: the finished calls are on disk.

## Limits

Three settings, read when a run starts (server settings, `settings.json`, with
the usual defaults when unset):

| Setting | Default | Most | What it limits |
| --- | --- | --- | --- |
| `workflow-concurrency` | 3 | 8 | Workers running at once. More calls wait for a permit. |
| `workflow-call-budget` | 40 | 500 | Distinct `agent()` calls in a run's whole life. A replayed call is not charged again. The call that goes over rejects in the script. |
| `workflow-timeout-minutes` | 60 | 1440 | How long a run may take, counted from each (re)start. When it passes, the run is `timed_out`, its waits stop and its unfinished workers are interrupted. |

`run_workflow`'s `concurrency`, `call_budget` and `timeout_minutes` can lower a
run's limits, never raise them past the settings.

## The app

The desktop app reads a run from the same record through four ACP methods
(`acp/workflows.rs`; the app side is `state/workflows.ts`):

| Method | What it does |
| --- | --- |
| `_rustcode/workflows/list` | `{sessionId}`: that thread's newest 20 runs. Asking, with or without a thread, subscribes the connection to changes. Each run is `workflow_status`'s answer without `result` (it can be large), with `thread`, `has_result`, `woken` and `driven` (whether this engine is driving it). |
| `_rustcode/workflows/changed` | A notification, `{sessionId, run}`, each time a run's record moves (a phase, a worker starting or ending, a log line, the end). The engine gathers changes for 300 ms and sends one summary per run. |
| `_rustcode/workflows/stop` | `{sessionId, runId}`: `stop_workflow` for the Stop button. |
| `_rustcode/workflows/wake` | `{sessionId, runId}`: claim the wake of a run that ended. Answers `{claimed}`, true once. |

**Waking the starting thread.** When a run ends the app tells the thread that
started it, once, with a short note marked as the app's own (`Workflow review:
done · 5 workers · 4m 12s`, then how to read the result). It goes after the
turn the thread is in, like a delegate's finish. The claim is kept in the run
record (`woken`), so a second window or a reload does not repeat it. A thread
whose agent already read the end (`workflow_status` on a settled run, or its
own `stop_workflow`) is not woken for it. The app announces only a run that
ended within the hour before it first saw it, so opening an old thread does not
wake it for a run from last week; the agent can always read the run with
`workflow_status`. With no app open nothing wakes the thread: the agent reads
the run with `workflow_status` and `wait_ms`.

**Limits in Settings.** A Workflows group under Orchestration in Settings has
the three settings in *Limits*, per machine like the delegate limits. A run
reads them when it starts.

## Not yet

- The built-in RustCode harness and outside agents (`mcp-connect`) as callers.
- A button to resume a run a restart left undriven without opening its thread.
- Retention of old runs.
