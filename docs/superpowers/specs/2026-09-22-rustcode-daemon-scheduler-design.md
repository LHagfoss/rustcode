# RustCode Daemon and Harness-Managed Scheduler

## Status

Design approved conversationally on 2026-09-22. The implementation remains
gated on review of this written specification and a subsequent implementation
plan.

## Context

RustCode currently runs interactive and headless agent turns, starts configured
MCP servers, and has an event-driven background-task manager. It does not own a
durable wall-clock scheduler. As a result, a model asked to poll Teams every
ten seconds has to invent a sequence of shell sleeps and repeated MCP calls.
The latest observed session hit replay suppression and loop warnings while
doing exactly that.

The desired product is a local RustCode daemon that owns scheduled work. The
agent harness should create and manage schedules through a built-in tool. The
CLI should remain available as an operator and recovery surface for starting,
stopping, inspecting, and editing the same daemon-managed jobs.

## Goals

- Run scheduled work while the interactive TUI is closed.
- Let the harness create, inspect, pause, resume, run, and delete jobs.
- Provide equivalent CLI operations for humans and scripts.
- Support recurring wall-clock schedules with an explicit timezone.
- Support deterministic direct MCP calls, normal headless prompts, and
  explicitly authorized shell commands.
- Replace model-authored sleep/poll loops with scheduler-owned bounded waits.
- Survive daemon restarts without losing jobs or creating duplicate firings.
- Provide durable run history, errors, retries, cancellation, and status.
- Keep scheduled executions separate from the user's interactive session unless
  the job explicitly targets one.

## Non-goals for the first implementation

- A hosted/cloud scheduler or multi-machine coordination.
- A public TCP API.
- Arbitrary user-defined code running inside the daemon.
- Automatic replay of an MCP or shell side effect whose outcome is ambiguous
  after a process crash.
- Full natural-language parsing inside the daemon. The harness translates
  natural language into typed scheduler arguments; the CLI requires explicit
  arguments.

## Design choices

### One daemon, two clients

`rustcode daemon` is the long-lived process and the sole scheduler owner.
The interactive harness and the CLI are clients of the same local control API;
neither writes scheduler state directly. This prevents the harness and CLI from
drifting into separate job implementations.

The daemon has two modes:

- `rustcode daemon run` runs in the foreground for launchd/systemd or manual
  supervision.
- `rustcode daemon start` starts a detached instance and waits for its health
  registration. `stop`, `status`, and `logs` operate on that instance.

The first local transport is a Unix domain socket under the RustCode config
directory with mode `0600`. The daemon writes its socket path, PID, process
start time, protocol version, and instance ID to an atomically published
registration file. PID plus process-start validation prevents signaling a
reused PID. The transport is isolated behind a small trait so a native local
transport can be added for Windows later.

### Harness-owned job lifecycle

The built-in `manage_scheduled_jobs` tool is the harness control surface. It
supports `create`, `list`, `pause`, `resume`, `run`, `history`, and `delete`.
The tool sends typed requests to the daemon and returns compact, bounded
results. It must not tell the model to edit a cron file, sleep, or poll a
status endpoint.

Creating a job is a user-visible side effect. The normal tool confirmation
policy applies when a model creates, changes, or deletes a job. A job records
the authorization and execution policy that were approved at creation time;
the daemon does not ask an interactive question when the TUI is closed.

The CLI exposes the same operations:

```text
rustcode daemon start
rustcode daemon run
rustcode daemon stop
rustcode daemon status
rustcode daemon logs

rustcode cron add ...
rustcode cron list
rustcode cron pause <id>
rustcode cron resume <id>
rustcode cron run <id>
rustcode cron history <id>
rustcode cron delete <id>
```

`cron add` is a convenience client of the daemon, not a second scheduler. It
may start the daemon when needed and reports whether the job was persisted and
whether the daemon accepted it.

### Durable storage

The daemon stores scheduler state in a SQLite database in the RustCode config
directory. The repository already bundles `rusqlite`, so this does not add a
new database runtime dependency.

The minimum tables are:

- `jobs`: stable ID, display name, enabled/paused state, schedule definition,
  timezone, action payload, workspace, target session, misfire policy, retry
  policy, next due instant, created/updated timestamps, and last run summary.
- `job_runs`: stable run ID and idempotency key, job ID, scheduled instant,
  claimed/running/terminal state, attempt number, lease owner/fence, start and
  finish times, result summary, error classification, and bounded output.

Job creation, next-due calculation, and run-history updates are transactional.
Claiming a due run verifies the job is enabled, its schedule revision has not
changed, and no active lease exists. Completion updates verify the same lease
fence, so a stale daemon cannot overwrite a newer owner.

### Scheduling semantics

The scheduler accepts a five-field cron expression plus an IANA timezone, and
also accepts structured daily/monthly forms from the harness tool. The harness
is responsible for converting phrases such as “every day at 8 in the morning”
into explicit values before calling the daemon.

Each loop iteration:

1. Loads the earliest due job from SQLite.
2. Sleeps until that instant, or wakes early on a job mutation, shutdown, or
   database recovery notification.
3. Transactionally claims all due jobs that can run.
4. Dispatches them through the execution boundary.
5. Records the outcome and computes the next civil-time occurrence.

The daemon also scans for overdue jobs at startup and after every wakeup. The
default recurring-job misfire policy is `skip_missed` so a laptop that was
closed overnight does not send a stale “good morning” at lunchtime. A job may
opt into `run_once` to execute one missed occurrence immediately. One-shot jobs
remain due until they are claimed or explicitly deleted.

For daylight-saving transitions, nonexistent local times move to the next
valid occurrence and ambiguous times fire once, using the first matching
instant. All persisted run timestamps are UTC; the schedule definition retains
the original timezone and civil-time rule.

### Action types

The first version supports four typed actions:

- `mcp_call`: server name, tool name, JSON arguments, and workspace/config
  context. This is the deterministic path for a hardcoded Teams message such
  as `send_chat_message`.
- `prompt`: a normal RustCode headless turn with a prompt, model profile,
  workspace, and dedicated or explicitly selected session. It uses the existing
  headless lifecycle and MCP tool policy rather than a separate agent loop.
- `shell_command`: command, working directory, environment allowlist, and
  timeout. Creation requires explicit confirmation and execution uses the
  existing non-interactive command safety policy; interactive sudo is never
  available.
- `poll`: a bounded recurring action built from a direct MCP call or shell
  command, with interval, deadline/max-runs, and an optional stop-on-change
  policy based on canonical result hashing. It exists specifically to prevent
  model-authored `sleep` loops. It does not automatically replay an ambiguous
  side effect after a crash.

MCP servers are resolved using the job's recorded workspace/config context,
not the daemon's incidental current directory. The daemon starts enabled MCP
servers as needed, isolates startup failures per server, and shuts down owned
server processes during daemon shutdown.

### Execution and concurrency

The scheduler owns claiming and lifecycle; an executor owns one run. Different
jobs may run concurrently, subject to a bounded daemon concurrency limit. Runs
targeting the same session are serialized. Jobs with dedicated sessions do not
interleave with the active TUI session by default.

Direct MCP calls go through the existing `McpClient` registry and `tools/call`
protocol. Prompt actions reuse the existing headless turn path after adding a
constructor that can load the job workspace/session context. Shell actions use
the existing command execution policy rather than spawning an untracked shell.

Every run has a cancellation token and a bounded execution timeout. `run`
from the CLI or harness requests cancellation only at a safe boundary and
reports whether the run was cancelled, completed, or already terminal.

### Retry and crash handling

Errors are classified as transient, permanent, cancelled, or ambiguous.
Transient failures use a persisted exponential backoff with a small jitter and
bounded attempts. Permanent failures pause only that run and leave the job
enabled for its next scheduled occurrence. Repeated failures are visible in
`history` and `status`; they do not silently disappear.

If the daemon exits while a run is leased, startup expires the lease. A run
that had not begun an external action may be safely retried. A run that may
have completed an external MCP or shell side effect is recorded as ambiguous
and is not automatically replayed; the CLI and harness can inspect it and
explicitly run it again.

### Observability

`daemon status` reports registration, PID/start time, uptime, socket, database,
active run count, and the next wake deadline. `cron list` reports job state and
next due time. `cron history` reports bounded per-run results and errors.

Scheduler lifecycle and claim/settlement transitions use the existing
operational-event logger and are attributed to the job and run IDs. Daemon
logs remain separate from interactive session transcripts. Prompt actions may
also append their bounded final result to the dedicated RustCode session.

## Testing requirements

- Cron and structured schedule parsing, including timezone and DST boundaries.
- Next-occurrence calculation and `skip_missed`/`run_once` behavior.
- SQLite schema creation, atomic job mutations, restart recovery, expired
  leases, and fencing against stale completion.
- Local daemon registration, status, start/stop, health checks, and CLI request
  round trips.
- Harness tool schemas and daemon-client error handling.
- Due-job wakeup on insertion, pause/resume, run-now, and shutdown.
- Per-session serialization and cross-job concurrency limits.
- Direct MCP action dispatch with a Teams-style hardcoded payload.
- Prompt and shell action policy, timeout, cancellation, retry, and ambiguous
  outcome handling.
- Bounded polling without repeated model-generated sleep/tool calls.
- Required repository verification: `cargo check --tests` and `cargo test`.

## Rollout boundary

The first implementation should deliver the daemon, local control API, durable
job store, harness management tool, CLI management commands, and all four
action types. Shell actions and the bounded `poll` action share the same
job/run model and safety tests; they do not introduce an alternate scheduler.
Native launchd/systemd installation can be added after the foreground daemon
mode is stable, because manual `daemon start` already gives the requested
always-on local behavior.
