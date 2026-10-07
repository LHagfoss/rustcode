# Terminal UI notes

See [command panels](command-panels.md) for slash commands.

## Tool activity

The TUI shows active foreground tools with elapsed time and a bounded output
preview, including `no output yet` for quiet work. `Esc` interrupts foreground
work. A running command gets one indicator row — `• Running $ <command> · 12s ·
esc interrupt` — instead of repeating its state per line; the row above the
composer keeps the current state, the model and the turn token total. Several
live calls share one `• Running` (or `• Queued`) heading and hang beneath it as
the same tree the finished `• Ran` group uses. Tool groups use `•` for running,
`◦` for queued, `✓` for completed, `×` for failed, and `−` for cancelled work. Each child row carries a tree connector (`├` for a child with
siblings below, `└` for the last one) and its output hangs under a continuous
`│` side spine, so wrapped output keeps the same indentation. Background
tasks keep their task IDs and process IDs visible; `/ps` lists them and `/stop`
stops them. A completed background result withheld during a foreground turn is
marked `result ready` until the turn consumes it. Approvals and questions have
separate waiting states.

## Performance and agent threads

`/perf` displays the latest turn's measured context, schema/serialization,
provider and tool time, request/round counts, context bytes and reported token
usage. Each session writes `performance.json`; `rustcode bench --report PATH
--baseline PATH` compares recorded measurements. Missing provider usage remains
unavailable. Time to first token tracks generated deltas, not keep-alives.

Safe built-in inspection calls run in bounded groups of four. Results keep
announcement order; shell commands, mutations and unknown MCP tools remain
serial barriers. Workspace generations drive incremental symbol queries,
environment snapshots, read validity and conservative successful-verification
reuse. Persisted evidence includes a process epoch so resume cannot mistake an
old generation for current evidence. Pressure projection preserves durable
history and native call/result pairing while making old inspection evidence
recoverable by reading it again.

Agent threads have isolated context snapshots, parent/child relationships,
bounded mailboxes, explicit followups, waits, cancellation and session restore.
Use `spawn_agent`, `list_agents`, `inspect_agent`, `send_message`, `send_agent`,
`wait_agent` and `cancel_agent`; select a child in the TUI to inspect its history.
See [performance and validity](performance.md),
[agent behavior](multi-agent.md) and
[architecture research](codex-multi-agent-research.md).

Description-based skill routing requires multiple matching intent terms and
keeps coding requests from activating live-app workflows incidentally.
[Selection benchmarks](benchmarks/tui-selection.md) distinguish backend
frame latency from live terminal input-to-paint latency.
