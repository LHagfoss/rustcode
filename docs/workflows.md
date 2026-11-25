# Workflows

A workflow is a JavaScript script that fans work out to worker threads and
keeps its place on disk. It exists because `fan_out` starts threads but the
plan around them (review each file, then verify the findings, with a barrier
between) lives only in the orchestrating agent's context, and is lost to a
restart or a compaction. A workflow run holds that plan in a script and in a
folder instead (#3394).

How an objective is run as a workflow, and who owns a run, is decided in
[ADR 0009](adr/0009-objectives-and-coordination.md).

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
need no app to be open. The built-in RustCode harness offers them too, from
the same orchestration version, and the engine runs them the same way.
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

**Retrying a run.** A run that ended `failed`, `timed_out` or `cancelled` can
be retried (#4383): `run_workflow` with `retry_run_id` (and no `script`, `name`
or `args`), or **Retry** on the run's card in the app
(`_rustcode/workflows/retry`). The run goes back to `running` under the same id
with its saved script; its finished calls are answered from `calls.jsonl` with
no thread started and no budget spent, and only failed or missing calls start
workers. `concurrency`, `call_budget` and `timeout_minutes` may lower its
limits, the timeout counts from the retry, and the thread is told once when it
ends. A `done` or still `running` run is refused.

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

**Retention.** A thread keeps its newest settled runs (`done`, `failed`,
`timed_out`, `cancelled`), 20 unless `workflow-kept-runs` says otherwise; when
a run starts, older settled runs' folders are removed. A run still `running` is never removed, since it may be resumed.

**Who drives a run.** One engine process drives a run at a time, and stamps
the run's `driver.json` every 10 seconds with its owner id (the one delegate
links carry). A stamp younger than 45 seconds from another process means that
process drives it, so the app's engine and the background server, which share
one config folder, never drive the same run; a resume while it does is refused.
`<config>/workflows/running/<run id>` names the thread of every run still
`running`, so an engine finds them without reading every thread (#4387).

**When the engine starts** it looks at those runs, then again every minute, and
takes up each one no live process drives whose starting thread is at Full
access, through its own orchestration answers, with no app and without the
thread being opened. A run of a thread below Full access waits for its thread
as before. After a sleep or a clock jump nothing is taken up until the next
look.

**When a run resumes.** The engine resumes the runs a thread left `running`
when that thread registers with the engine's orchestrator hub (the thread is
opened or started on a harness with the orchestration tools). Until then
`workflow_status` reports the run as `running` with a note that it is not being
driven. The workflow card and status row offer **Resume** while the run is undriven.
Replay reads finished calls from disk and continues with the unfinished work.
A run resumed from **Resume** starts its workers through the app that pressed
it, with approval cards like any delegate the thread starts (#4380); the
engine answers the calls itself only when no app hosts the thread's
orchestration, and then only a thread at Full access starts workers.

## Limits

Four settings, read when a run starts (server settings, `settings.json`, with
the usual defaults when unset):

| Setting | Default | Most | What it limits |
| --- | --- | --- | --- |
| `workflow-concurrency` | 3 | 8 | Workers running at once. More calls wait for a permit. |
| `workflow-call-budget` | 40 | 500 | Distinct `agent()` calls in a run's whole life. A replayed call is not charged again. The call that goes over rejects in the script. |
| `workflow-timeout-minutes` | 60 | 1440 | How long a run may take, counted from each (re)start. When it passes, the run is `timed_out`, its waits stop and its unfinished workers are interrupted. |
| `workflow-kept-runs` | 20 | 200 | Settled runs a thread keeps. When a run starts, older settled runs past this count are deleted; running runs are always kept. A listing names the newest 20. |

`run_workflow`'s `concurrency`, `call_budget` and `timeout_minutes` can lower a
run's limits, never raise them past the settings.

## The app

The desktop app reads a run from the same record through these ACP messages
(`acp/workflows.rs`; the app side is `state/workflows.ts`):

| Method | What it does |
| --- | --- |
| `_rustcode/workflows/list` | `{sessionId}`: that thread's newest 20 runs. Asking, with or without a thread, subscribes the connection to changes. Each run is `workflow_status`'s answer without `result` (it can be large), with `thread`, `has_result`, `woken` and `driven` (whether this engine is driving it). |
| `_rustcode/workflows/changed` | A notification, `{sessionId, run}`, each time a run's record moves (a phase, a worker starting or ending, a log line, the end). The engine gathers changes for 300 ms and sends one summary per run. |
| `_rustcode/workflows/stop` | `{sessionId, runId}`: `stop_workflow` for the Stop button. |
| `_rustcode/workflows/resume` | `{sessionId, runId}`: resume one undriven run from its saved script and progress. |
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
`workflow_status`. With no app connected to the engine, the engine tells the thread itself
(#4382): it claims the same `woken` mark, and runs the same note as the
thread's next turn once its current turn ends, unless the thread is at Ask
access, where nobody could approve that turn; then the agent reads the run
with `workflow_status` and `wait_ms`.

**Limits in Settings.** A Workflows group under Orchestration in Settings has
the four settings in *Limits*, per machine like the delegate limits. A run
reads them when it starts.

## Not yet

- Outside agents (`mcp-connect`) as callers.
- A workflow list that can resume runs across threads.
