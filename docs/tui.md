# Terminal UI notes

See [command panels](command-panels.md) for slash commands.

## Tool activity

Tool calls appear in the transcript as one kind of block. While a batch is in
flight the block is headed `• Running`; once it finishes the same block is
headed `• Ran`. Each call is a row under the heading that names what it was
(`Bash cargo test`, `Read src/main.rs`, `Edit src/lib.rs (+12 -3)`) with its
state behind it: elapsed time while it runs, `waiting` until it starts, and
`exit 1`, `failed`, `cancelled` or `background` afterwards. Finished rows lead
with `✓`, `×` or `−`. A call that finishes within 200 ms is never drawn as
running. `Esc` interrupts foreground work.

A row is one terminal line and its state is always on it. The state's width is
set aside first and the command or path takes what is left: a command or query
is cut at its end, a path at its start, and a command that spans lines is
joined into one. A terminal too narrow for that drops the row's extras
(`+N earlier`, diff counts), then the target, then the end of the label. Only a
question and its answer (`Asked …`) wrap, since they are shown whole. A command
row whose command history no longer holds reads `(command not recorded)`, or
`(task <id>)` for a background launch.

A command started in the background keeps its row: it reads `· background`
while the task runs and takes the finished state (`✓`, `× … · exit 1`,
`× … · failed`, `− … · cancelled`) once the task has ended. The launch row has
no output under it; the task's output comes with the task. Finished tasks
report as `Task <command>` (the task id when no command was recorded); several
finishing together draw one row for the latest with `+N earlier` behind it, and
opening that row shows the latest task's output. With
`preserve_transcript_scrollback` the copy already written to the terminal's own
scrollback is not revised.

Every tool row carries what the tool returned: command output, a file edit's
diff, the lines a read or search found, an MCP or skill result. Output hangs
under its row on a `│` spine. Verbosity only sets whether it starts
open: `high` shows the rows alone, `low` shows a five-row preview. Hovering a
block lights it and clicking it opens or closes that block's output; `ctrl+o`
does the same for every block, and `ctrl+shift+o` steps one entry at a time.

The row under the transcript names the state of the turn (`Generating`,
`Thinking`, `Working`, `Queued`) with the model and the turn's token total. It
never names a tool. Approvals and questions have separate waiting states.
The clock on the row, and the `Worked for` line it becomes, count the turn
from the submitted prompt, including model and tool time but not the time an
approval or a question waited for the user: the row is hidden while one is
open and the clock resumes where it stopped. `performance.json` reports that
wait on its own as `user_wait_us`.

Background tasks are counted in the footer (`2 tasks · 1 done`), where `done`
is a result the model has not read yet. Clicking the counter, or `/tasks`
(`/ps`), opens the tasks panel. It lists what is running first (command and
elapsed time), then what finished this session, dimmed, with its outcome
(`done`, `exit 1`, `failed`, `stopped`) and how long it ran; with nothing
running it says so. The list is rebuilt on every frame, so times and states stay
current while it is open.

| Key | Tasks panel |
|-----|-------------|
| `↑`/`↓` (`k`/`j`), mouse wheel | Move the selection; in the log, scroll. |
| `enter` (`→`) | Show the tail of the selected task's log (last 16 KiB). An open log of a running task follows its output. |
| `x` | Stop the selected task, or the one whose log is shown, if it is still running. |
| `esc` (`←`) | Leave the log for the list; from the list, close the panel. |
| `q` | Close the panel. |

`/stop` stops every running task.

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
