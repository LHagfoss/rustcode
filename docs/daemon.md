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
rc daemon start
rc daemon status
rc daemon logs --lines 100
rc daemon stop
```

`daemon run` is the foreground worker used by `daemon start`. It owns the
event-driven scheduler and executes due jobs. The socket and registration are
private to the current user. On Windows the worker detaches from the calling
console, so closing the terminal leaves scheduled jobs running.

### In the server

The RustCode server runs the scheduler too, in its own process
(`daemon/host.rs`), so with the server installed (`rc server install`) jobs
run from login with no terminal or app open. The daemon's owner lock keeps
one scheduler per config directory:

- A scheduler already running (`rc daemon start`) is left alone. The server
  looks again every minute and takes over once that one is gone.
- `rc daemon stop` stops the server's scheduler until the server restarts.
- Why the server could not run it goes to `rc server logs`, once per reason.

## Create a morning Teams job

Start the daemon, then create a job through the harness tool or with the CLI.
The MCP server name must exist in the workspace configuration so RustCode can
store its execution snapshot with the job:

```sh
rc jobs add \
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
rc jobs list
rc jobs pause morning-teams
rc jobs resume morning-teams
rc jobs run morning-teams
rc jobs stop morning-teams
rc jobs history morning-teams --limit 20
rc jobs delete morning-teams
```

`rc loops` and `rc cron` stay as aliases for `rc jobs`. The built-in
`manage_scheduled_jobs` tool exposes the same operations as `create`, `list`,
`pause`, `resume`, `run`, `history`, and `delete`. History output is bounded
and settled runs beyond `MAX_HISTORY_ROWS` (100 per job) are pruned on write.
Polling is also bounded by interval, maximum runs, and deadline; it does not
create a shell `sleep` loop or ask the model to repeatedly poll.

## From an agent

An orchestrating thread or an outside agent can manage jobs through RustCode's
MCP tools: `list_jobs` lists them (including job configuration), `create_job`
creates one on any harness, `update_job` edits, pauses or resumes one,
`run_job` triggers a run now, and `delete_job` removes one. Legacy callers
using `list_loops`, `create_loop`, `update_loop`, `run_loop`, `delete_loop`,
`list_scheduled_tasks`, `run_scheduled_task_now`, `delete_scheduled_task` and
`schedule_task` keep working through `orchestration/mcp_names.rs` and
`_rustcode/scheduled/*` (`engine/src/acp/scheduled.rs`). Nothing starts the
daemon for them: when it is stopped, `list_jobs` says `daemon_running: false`
with a `hint` (start it from Jobs in the app, or `rc daemon start`), and
the other calls refuse with the same reason.

The user-facing name for all of this is **Jobs**: the app's page, its dialog,
the CLI (`rc jobs`), agent instructions and MCP tool names use "job" (once,
recurring, manual or webhook-triggered). Internal daemon persistence
(`JobStore`, `daemon/jobs.sqlite`), wire action `"type": "loop"`, ACP
`_rustcode/loops/*` routes and legacy tool/CLI aliases keep their names for
existing callers.

## Jobs (the `loop` action)

A job in the app is a daemon job whose action is `loop`: a prompt run once,
on a schedule, manually, or from a webhook, each run in a fresh top-level
thread on any harness, or every run in one thread the job names. The desktop
app's Jobs page creates, edits, pauses, resumes, runs, stops and deletes them
through `_rustcode/loops/*` (`engine/src/acp/loops.rs`), so `rc jobs list`
shows the same entries. Agents running in a RustCode thread can use
`list_jobs`, `create_job`, `update_job`, `run_job` and `delete_job`. The
desktop shows the schedule, project, agent, access and prompt for approval.
Pass a five-field cron or one ISO 8601 time, and use `list_harnesses` for
harness and account IDs. Outside agents (`rc mcp-connect`) have `list_jobs` at
Read-only access and `create_job`, `update_job`, `run_job` and `delete_job` at
Manage access with an absolute project path. Their connection's approval
setting applies, and cron uses the scheduler machine’s time zone when they
omit it.

```json
{
  "type": "loop",
  "prompt": "Check CI on main and tell me only if something broke.",
  "workspace": "/absolute/project",
  "harness": "codex",
  "account": null,
  "model": null,
  "effort": "medium",
  "read_only": true,
  "thread_id": null
}
```

A named account applies to fresh threads on its harness. Selecting one in the
editor also saves the harness. A loop aimed at an existing thread uses that
thread's account. A job with no account runs on the one a new thread would
get: the editor shows and saves it, and for a job saved without one the run
takes the named account the user's turns on that harness last ran on, else
its first named account. It never falls back to the harness's bare default
login while named accounts exist.

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
- **Quiet by default, verdicts when actionable.** The prompt asks the agent to
  start with “Nothing to report” when nothing needs the user, or a one-line
  verdict when something does. A fresh run's thread whose last reply starts
  with “Nothing to report” is archived (status `done`) and its output adds
  `"quiet": true`; the page says "Nothing to report" and the review list
  leaves it out. For non-quiet runs, the opening prose line (or a Smart UI
  view's title when the reply opens with a fenced block) is captured as
  `verdict` in the run output and shown on the Jobs page and Review tab. A
  job's own thread is never archived.
- **Transient retries before pausing.** Transient failures (timeouts, rate
  limits, 502/503/504, connection resets) retry with backoff according to the
  job's `retry_policy` before counting toward the three-failure auto-pause.
- **Stop an active run.** An in-flight run can be cancelled from the Jobs page
  (`_rustcode/loops/cancel`) or `rc jobs stop <id>`.
- **Run threads stay out of the thread list.** Each fresh thread a run starts
  carries its owning job's stable id in `scheduledJob` metadata
  (`_meta.rustcode.scheduledJob` in `session/list`); the sidebar leaves it to
  the Jobs page, which opens it from the run.
- **Script mode.** A job may carry a `script` (set in the app's editor; an
  agent's `create_loop` cannot). Each run executes it first in the project
  folder through the same sandbox as `shell_command`, read-only when the job
  is, for at most five minutes. Empty standard output records a succeeded run
  with `{"quiet": true, "silent": true}` and starts no agent turn, so it costs
  no tokens; the page says "The script printed nothing". Anything printed is
  appended to the prompt and the run continues as above. A script that exits
  non-zero fails the run ("The script failed: …"), which counts toward the
  three-failure pause.
- **Once.** A Loop saved with `{ "kind": "at", "at": <RFC 3339> }` runs one
  time; the dialog reads "in 2h", "in 30 min" or "at 09:00".
- **Read-only by default.** A read-only run is prompted at Ask access and
  every permission request is declined, so edits, commands that ask, commits
  and pushes do not happen. Turned off, the run is at Full access, as a
  delegate is.
- **Webhook.** A Loop with its webhook on also runs when
  `POST /hooks/<loop-id>/<secret>` (or `/hooks/<loop-id>` with
  `Authorization: Bearer <secret>`) reaches the outside-agent listener: on
  this computer's loopback port, and through the public address when
  `[outside] public_url` is set. The request body (up to 64 KB) is kept on
  the run and added to the prompt, fenced and marked as outside data the
  agent must not take instructions from. The daemon stores only the secret's
  SHA-256, so the URL is shown once, when the webhook is turned on or its
  secret replaced; `rc jobs list` and agents never see it, and an
  agent's `create_job` cannot turn a webhook on. An unknown job, one
  without a webhook and a wrong secret all answer 404; a paused job or one
  with a run going answers 409, and calls that arrive before a run starts
  collapse into one with the latest body. A request with
  `X-GitHub-Event: ping`, which GitHub sends when a webhook is added, checks
  the secret and answers 200 without a run. A job saved with
  `{ "kind": "manual" }` has no schedule and runs only from its webhook or
  Run now.
- **Scheduled runs can start with the app closed.** While the scheduler
  daemon is running, it uses the app's engine when available and starts the
  shared RustCode server on demand otherwise. The run and its result appear
  in Jobs when the app reopens. Webhooks still require a running listener.
- **A missed run fires once.** A job's cron uses the `run_once` misfire
  policy: runs missed while the computer slept coalesce into one.
- **No overlapping runs.** The scheduler never claims a job with a run still
  active, so a run waits for the previous one to end.
- **Three failures pause it.** When the last three settled runs all failed
  (or ended ambiguous), the scheduler pauses the job; the page shows the last
  error as the reason. Resuming starts counting again.
- **A signed-out login pauses it at once.** A run whose harness answers that
  it is not signed in fails with an error naming the login, its folder and
  the command that signs it in, and the job pauses after that one run.
- A run's agent turn may take 25 minutes before it is stopped.

### Across paired machines (and optional hub fallback)

Paired machines form one bidirectional network: the remote ACP relay exposes
`_rustcode/loops/*`, so the Jobs page reads and manages jobs directly on every
connected paired computer's own scheduler daemon—with full project browsing,
that machine's harnesses and accounts, pre-check scripts, existing-thread runs
and clickable run threads—without requiring a hub.

On a hub (`docs/remote.md` › The hub), a job may also name a hub member in
`machine` (the editor's *Machine* choice for an offline paired member or an
unpaired member reachable through a hub, or `machine` on an outside agent's
`create_loop`). The hub keeps the schedule, so a run is due while that machine
sleeps, and `daemon/hub_run.rs` sends each run over the hub link: a fresh
thread is `start_session` there (standalone, in the folder `workspace` names
on that machine), a job with a thread is `message_session` to it. While the
machine is offline the hub's mailbox holds the run, for up to seven days.

- The run ends once the member has the thread (*Started on laptop*) or the
  hub holds it (*Waiting for laptop*). The thread runs on there, and the hub
  does not see whether it found anything to report.
- The member answers at the access it granted the hub when it joined. A
  read-only job asks for `access: "ask"`, which `start_session` takes as a
  ceiling, so it never runs with more than that.
- A job for another machine takes no script (the script would run on the
  hub). The editor sends no account, so the member uses its own default
  login; an agent may name one of the member's.
- A hub with no remote gateway running skips the run.

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

