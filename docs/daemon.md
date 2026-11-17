# RustCode scheduler daemon

RustCode owns scheduled work in a local daemon. Jobs and run history are
stored under the RustCode config directory; the CLI and the harness
`manage_scheduled_jobs` tool use the same private local endpoint and protocol.
On macOS and Linux this is a Unix socket (`daemon/control.sock`). On Windows
it is loopback TCP with a fresh token published in an owner-only
`daemon/control.endpoint` file, using the same ACL protection as other private
RustCode files. The task store remains `daemon/jobs.sqlite` on every platform.

## Start and inspect the daemon

```sh
rustcode daemon start
rustcode daemon status
rustcode daemon logs --lines 100
rustcode daemon stop
```

`daemon run` is the foreground worker used by `daemon start`. It owns the
event-driven scheduler and executes due jobs. The socket and registration are
private to the current user. On Windows the worker detaches from the calling
console, so closing the terminal leaves scheduled jobs running. Starting at
login still requires configuring a service or startup task.

## Create a morning Teams job

Start the daemon, then create a job through the harness tool or with the CLI.
The MCP server name must exist in the workspace configuration so RustCode can
store its execution snapshot with the job:

```sh
rustcode cron add \
  --id morning-teams \
  --name 'Morning Teams message' \
  --workspace "$PWD" \
  --schedule '{"kind":"daily","hour":8,"minute":0,"timezone":"Europe/Oslo"}' \
  --action '{"type":"mcp_call","server":"teams","tool":"send_chat_message","workspace":"'"$PWD"'","arguments":{"chat_name":"Daily","message":"Good morning!"}}'
```

The daily schedule uses the named IANA timezone, including daylight-saving
transitions. Use `--json` for scriptable output.

The equivalent harness operation is:

```json
{
  "operation": "create",
  "id": "morning-teams",
  "name": "Morning Teams message",
  "workspace": "/absolute/workspace",
  "schedule": {"kind":"daily", "hour":8, "minute":0, "timezone":"Europe/Oslo"},
  "action": {
    "type":"mcp_call",
    "server":"teams",
    "tool":"send_chat_message",
    "workspace":"/absolute/workspace",
    "arguments":{"chat_name":"Daily","message":"Good morning!"}
  }
}
```

RustCode snapshots the configured MCP command and environment at creation
time, so later config changes do not silently redirect a durable job.

## Manage jobs

```sh
rustcode cron list
rustcode cron pause morning-teams
rustcode cron resume morning-teams
rustcode cron run morning-teams
rustcode cron history morning-teams --limit 20
rustcode cron delete morning-teams
```

The harness tool exposes the same operations as `create`, `list`, `pause`,
`resume`, `run`, `history`, and `delete`. History output is bounded. Polling is
also bounded by interval, maximum runs, and deadline; it does not create a
shell `sleep` loop or ask the model to repeatedly poll.

## From an agent

An orchestrating thread can schedule work with the same client
(`docs/orchestrator.md` v10): `schedule_task` adds a prompt on a cron
expression or once at an ISO time, and `list_scheduled_tasks`,
`delete_scheduled_task` and `run_scheduled_task_now` manage what is there.
They go through `_rustcode/scheduled/*` (`engine/src/acp/scheduled.rs`), the
same socket and protocol `rustcode cron` uses — one scheduler, one store — and
each changing call asks the user first. A scheduled prompt runs on the
built-in harness in a recorded session, so it takes `model_profile` rather
than a harness or account. Nothing starts the daemon for them: without
`rustcode daemon start`, `list_scheduled_tasks` says `daemon_running: false`
and the other calls refuse with that instruction.

## Loops

A Loop is a job whose action is `loop`: a prompt run on a schedule, each run
in a fresh top-level thread on any harness, or every run in one thread the
Loop names. The desktop app's Loops page
creates, edits, pauses, resumes, runs and deletes them through
`_rustcode/loops/*` (`engine/src/acp/loops.rs`) and the scheduled run/delete
methods, so `rustcode cron list` shows the same jobs.

```json
{
  "type": "loop",
  "prompt": "Check CI on main and tell me only if something broke.",
  "workspace": "/absolute/project",
  "harness": "codex",
  "model": null,
  "effort": "medium",
  "read_only": true,
  "thread_id": null
}
```

- **One thread per run.** The daemon opens the ACP endpoint the app's engine
  publishes (`run/engine.json`, else a shared server's) and sends the same
  `session/new` and `session/prompt` the app does
  (`orchestration/local_threads.rs` › `run_loop_thread`). The thread is titled
  with the Loop's name and runs in the project's own folder, no worktree. The
  run's output records `{"thread_id": …}`, which the page links to.
- **Or one thread every time.** With `thread_id` set, each run loads that
  thread (`session/load`) and sends the prompt as its next message; harness,
  model and effort are the thread's own. A run while the thread works on a
  turn, or while it is open in another RustCode process, is skipped like a
  run with the app closed; a deleted thread fails the run.
- **Quiet by default.** The prompt asks the agent to start with “Nothing to
  report” when nothing needs the user. A fresh run's thread whose last reply
  starts that way is archived (status `done`) and its output adds
  `"quiet": true`; the page says "Nothing to report" and the review list
  leaves it out. A Loop's own thread is never archived.
- **Once.** A Loop saved with `{ "kind": "at", "at": <RFC 3339> }` runs one
  time; the dialog reads "in 2h", "in 30 min" or "at 09:00".
- **Read-only by default.** A read-only run is prompted at Ask access and
  every permission request is declined, so edits, commands that ask, commits
  and pushes do not happen. Turned off, the run is at Full access, as a
  delegate is.
- **The app must be open.** With no engine to reach, the run is recorded as
  cancelled with `{"skipped": …}` and does not count as a failure. (A
  headless engine the daemon starts itself would lift this; not built yet,
  issue 3937.)
- **A missed run fires once.** A Loop's cron uses the `run_once` misfire
  policy: runs missed while the computer slept coalesce into one.
- **No overlapping runs.** The scheduler never claims a job with a run still
  active, so a run waits for the previous one to end.
- **Three failures pause it.** When the last three settled runs all failed
  (or ended ambiguous), the scheduler pauses the Loop; the page shows the last
  error as the reason. Resuming starts counting again.
- A run's agent turn may take 25 minutes before it is stopped.

Editing a job (`DaemonRequest::Update`) keeps its id, creation time, paused
state and history, moves its next run to what the new schedule says, and is
refused while a run is going.

## Safety and recovery

Every run is durably claimed with a lease and fence. A daemon crash after an
external action may have started records the run as `ambiguous`; RustCode does
not automatically replay that action. Transient startup/provider failures can
retry according to the job policy, while shell/MCP side effects remain
conservative about ambiguity. Inspect `cron history` before manually using
`cron run` after an ambiguous result.

