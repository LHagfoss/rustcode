# RustCode scheduler daemon

RustCode owns scheduled work in a local daemon. Jobs and run history are
stored under the RustCode config directory; the CLI and the harness
`manage_scheduled_jobs` tool use the same private Unix socket and protocol.

## Start and inspect the daemon

```sh
rustcode daemon start
rustcode daemon status
rustcode daemon logs --lines 100
rustcode daemon stop
```

`daemon run` is the foreground worker used by `daemon start`. It owns the
event-driven scheduler and executes due jobs. The socket and registration are
private to the current user.

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

## Safety and recovery

Every run is durably claimed with a lease and fence. A daemon crash after an
external action may have started records the run as `ambiguous`; RustCode does
not automatically replay that action. Transient startup/provider failures can
retry according to the job policy, while shell/MCP side effects remain
conservative about ambiguity. Inspect `cron history` before manually using
`cron run` after an ambiguous result.

