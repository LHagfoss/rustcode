# ACP server integration

Run RustCode as an Agent Client Protocol v1 server over standard input/output:

```bash
rustcode --acp
# equivalent: rustcode acp
```

ACP stdout is reserved for protocol traffic. Launch the process as a child,
initialize it, create a session, and submit prompts through the ACP v1 methods.

## Sessions and configuration

`session/new` creates an isolated RustCode session. The request's working
directory becomes the task working directory and default project scope. The
trusted default permits paths outside the launch workspace. Explicitly
restricted modes retain the workspace boundary; YOLO overrides those restrictions
and auto-approves tool permissions without changing the saved configuration. Model and tool settings come from
the normal RustCode configuration stack:

```text
CLI overrides > nearest .rustcode/config.toml > global config.toml > defaults
```

Configured MCP servers start before ACP prompts are handled. The optional
MCP-over-ACP transport is not required.

RustCode advertises session-close support. `session/close` removes the session
route, cancels its active prompt, aborts pending task-start barriers, and stops
that session's live background processes. Closing the stdio transport performs
equivalent cleanup for all sessions.

## Prompt serialization and cancellation

Prompts within one session are serialized. Separate sessions remain isolated.
A cancel notification targets the active turn for that session.

Turns of different sessions can run at the same time in one engine. Each
turn's debug lines and `[op]` events, including those logged by its tools,
are filed under that turn's own session (`sessions/…/<id>/logs/debug.log` and
the `session_id` field), not under whichever session most recently became the
process's active one. When reading a session's log, a sudden jump in
`rendered_history_messages` between rounds is therefore real history, not
another chat's turn.

Cancellation stops model/tool execution for the prompt, but a command already
detached into the background is not implicitly discarded solely because the
model turn was cancelled. Its eventual completion is persisted independently.
The cancelled prompt is never resumed. Use `manage_task`, session close, or an
explicit session stop to terminate detached work.

## Background tool calls

ACP background calls follow this sequence:

1. RustCode registers the provider's original tool-call ID before spawning.
2. The client receives `ToolCall` with `InProgress`.
3. The detached command runs while the logical prompt is paused.
4. RustCode persists the terminal result in session history.
5. The client receives exactly one terminal `ToolCallUpdate` using the same
   provider tool-call ID.
6. If the prompt remains active, RustCode resumes it with the existing turn
   context, tool-round budget, and verification state.

Fast commands cannot publish a terminal update before `InProgress`; a start
barrier enforces ordering. Synthetic completion history remains visible to the
model but is not replayed as a duplicate ACP terminal update.

Terminal statuses reflect the real outcome: completed, failed, or cancelled.
The generated RustCode task ID remains an internal task-management identifier
and is not substituted for the provider call ID in ACP updates.

## Questions

ACP v1 has no method for asking the user a question, so a pending
`ask_question` is sent as `session/request_permission`. Its options are the
question's answers (`question_option_<index>`, kind `allow_once`) plus
`question_cancel` (kind `reject_once`); choosing Cancel, or answering
`cancelled`, cancels the turn, as Escape does in the terminal. Every question in
a chain is sent in turn, each at most once.

The request carries `_meta.rustcode` with `kind: "question"`, `version: 2`, the
`questionId` and `answerKinds: ["options", "text"]`, and the full question in
`toolCall.rawInput`. A RustCode-aware client can answer with several options or
free text by selecting any answer option and adding `_meta.rustcode.answer`:
`{"options": ["A", "B"]}` or `{"text": "..."}`. The engine validates that answer
like a terminal answer; one it cannot take cancels the question.

## Localhost preview

A client that sends `clientCapabilities._meta.rustcode.preview` (version 1) at
initialize gets the `localhost_preview` tool in its sessions; no other client
sees it. Each call is sent to the client as the extension request
`_rustcode/preview` with `{"sessionId", "operation": {"action", …}}`, and the
client answers `{"ok": true, …}` or `{"ok": false, "error": {"code",
"message"}}`. The engine reports success only for `"ok": true` and bounds every
call with a timeout and the turn's cancellation. The protocol, the client's
approval and origin rules, and the limits are in [preview.md](preview.md).

## Skills and MCP servers

These extension requests back the app's Settings pages:

- `_rustcode/capabilities` (`{}`) answers with `{"skills": [...],
  "mcpServers": [...], "configPath"}`. A skill has `name`, `description`,
  `path`, `source` (`workspace`, `universal`, `rustcode`, `configured`,
  `claude-project` or `claude-user`; the last two are a harness's own read-only
  roots), `readOnly` (true for exactly those two sources, so a client hides its
  Edit action) and `enabled`. A server has `name`, `transport` (`stdio` or
  `http`),
  `target` (credentials masked), `enabled`, `scope` (`user`, `project` or
  `session`), `connection` (`connected`, `not_connected`, `disabled` or
  `failed`), `tools` and `error`. Header and environment values are never sent.
- `_rustcode/capabilities/set` (`{"kind": "skill" | "mcp", "name", "enabled"}`)
  saves the toggle to the user config, starts or stops an MCP server in this
  engine, and answers with the fresh snapshot. An unknown skill or a server the
  user config does not declare is an `invalid_params` error; a server that
  fails to start is reported as `failed` in the snapshot.
- `_rustcode/capabilities/mcp/add` (`{"name", "target": [...], "transport"?:
  "stdio" | "http", "env"?: ["KEY=value"], "headers"?: ["Name: value"]}`)
  validates the server like `rustcode mcp add`, saves it to the user config,
  starts it in this engine and answers with the fresh snapshot. A duplicate
  name or a target that does not fit the transport is an `invalid_params`
  error; a server that fails to start is still saved and reported as `failed`.
- `_rustcode/capabilities/mcp/remove` (`{"name"}`) deletes a server from the
  user config, stops it, and answers with the fresh snapshot.

Semantics and scope are described in
[configuration.md](configuration.md#turning-skills-and-mcp-servers-on-and-off).

## Harnesses

A thread can run on an external coding agent (Codex, Claude Code, Gemini
CLI, OpenCode, any ACP agent) instead of the built-in harness. The
contract is in [harnesses.md](harnesses.md); the requests are:

- `_rustcode/harnesses` (`{}`): every harness with its detection,
  capabilities and models, the defaults and `onboarded`.
- `_rustcode/harnesses/models` (`{"harness", "refresh"?}`): `{"models": [...]}`,
  starting the harness briefly when its models are not known yet, or always
  with `refresh`.
- `_rustcode/harnesses/set` (`{"id", "enabled"?, "command"?, "env"?, "name"?,
  "kind"?}`): turn a harness on or off, override its command, or add a
  custom ACP harness.
- `_rustcode/harnesses/remove` (`{"id"}`): remove a custom harness.
- `_rustcode/harnesses/default` (`{"harness", "model"?, "effort"?}`): what
  new threads start with.
- `_rustcode/onboarding/complete` (`{}`): first-run onboarding is done.

Each answers with the fresh snapshot (`models` with its list). `session/new`
with `_meta.rustcode.harness` starts a thread on that harness; the rest of
the thread is standard ACP.

## Orchestration

A client that hosts threads advertises `clientCapabilities._meta.rustcode.orchestration`
in `initialize`: `1` for the M1 tools (`list_sessions`, `message_session`,
`wait_session`), `2` to add `read_session`, `start_session` and `fan_out`
([orchestrator.md](orchestrator.md)). The engine sends each call to the
client as a `_rustcode/orchestration` request
(`{"sessionId", "tool", "args", "callId"}`); the client answers with the
result or `{"error": {"code", "message"}}`, and the engine sends
`_rustcode/orchestration/cancelled` (`{"sessionId", "callId"}`) when it drops
a call before that answer arrives. With `2`, external harness threads get the
same tools through the injected `rustcode mcp-orchestrator`, and their
calls arrive the same way under the external thread's id; the thread's
`_meta.rustcode.harness.capabilities.orchestration` says whether it has them.

## Worktrees, thread status and checkpoints

The contract is in [orchestrator.md](orchestrator.md); the requests are:

- `_rustcode/worktrees/create` (`{"repo", "branch"?, "title"?, "base"?}`):
  a git worktree under `<config>/worktrees/<repo-name>/<slug>`, answered as
  the `Worktree`.
- `_rustcode/worktrees/list` (`{"repo"}`): `{"worktrees": [...]}` with
  `dirty`, `ahead`, `behind` and `session_ids`.
- `_rustcode/worktrees/remove` (`{"path", "force"?}`): refuses a dirty or
  unpushed worktree (`error.data.reason`) unless `force`.
- `_rustcode/session/status` (`{"session_id", "status": "review" | "done" |
  "idle"}`): `{}`.
- `_rustcode/session/pin` (`{"session_id", "pinned": bool}`): `{}`. The pin
  is stored with the thread (`pin.json`), so the terminal UI and the apps
  agree; archiving a thread is the `done` status.
- `_rustcode/checkpoints/list` (`{"session_id"}`),
  `_rustcode/checkpoints/diff` (`{"session_id", "turn"}`) and
  `_rustcode/checkpoints/restore` (`{"session_id", "turn", "rewind"?}`).

`session/new` with `_meta.rustcode.worktree` starts the thread in a new
worktree. `session/new`, `session/load` and `session/list` carry
`_meta.rustcode.status` and `_meta.rustcode.pinned` (and `worktree` for a
worktree thread); a status or pin change is sent as `session/update`
`session_info_update` with `_meta.rustcode.status` or
`_meta.rustcode.pinned`.

## Replay cursor

Every update of a session's stream (what its turns, tools and title changes
send, and a prompt shown to its other clients) carries a cursor in
`_meta.rustcode.cursor`, a number that only grows. Each update is numbered,
logged and queued to every client under one lock, so a client gets cursors in
increasing order and never an update behind one it has not been sent.

What a load sends to rebuild a view (the transcript, the running turn, the
title, commands and usage, and the reset marker below) is not in that stream
and carries no cursor. The `session/load` answer carries the client's cursor
in `_meta.rustcode.cursor`: the newest update queued to that connection before
the answer, so everything up to it has been sent. A client keeps the greatest
cursor it has, with two rules:

- Do not store the cursor of a load before its answer has arrived. A load cut
  off halfway leaves nothing to resume from, so the next one is a full load.
- On a `reset`, drop the stored cursor along with the view, and store the
  answer's. (A cursor that was too old to resume stays too old, so asking
  again with it also rebuilds.)

Thread status updates sent to every client for the thread list have no cursor.

`session/load` accepts `_meta.rustcode.resume: {"cursor"}`, the cursor the
client last saw:

- The engine keeps the latest updates of each open session in memory (8,192
  updates or 8 MiB as JSON, whichever comes first). When every update after
  `cursor` is still kept it sends only those, each with its own cursor, then
  streams live, with nothing missed or sent twice. Nothing else is sent: not
  the transcript, and not the title, commands and usage a plain load adds.
- Otherwise the cursor is too old, from before the engine restarted or the
  session was closed (cursors start from the clock, so an earlier run's never
  fall in a later log, as long as the clock does not step back), or from the
  future. The engine sends an empty `session_info_update` with
  `_meta.rustcode.reset: true` (also in the answer), then everything a load
  without `resume` sends. The client clears what it shows of the session at
  the marker.
- A rewind (`_rustcode/rewind`, a checkpoint restore that rewinds the chat)
  cuts the history without an update, so it ends every resume from before it.
- A load without `resume` behaves as before, plus the cursors, and never gets
  a `reset`.

Each connection keeps its own position; the engine holds none for them. A
prompt shown to the session's other clients is logged too, but not sent to the
connection that sent it, so that connection's cursors skip it; if it drops
before the next update it gets its own prompt again on resume.

## Slash commands

A session's commands arrive as `session/update` `available_commands_update`
when it starts (`session/new`) and when it is reopened (`session/load`), with
the RustCode action in `_meta.rustcode.action` and `_meta.rustcode.report` on
the bare read-only ones. `_rustcode/commands` (`{}`) answers the same update
as a single value (`sessionUpdate`, `availableCommands`), so a client can offer
the commands before it has a session to advertise them on: the desktop app's
new thread shows its drawer from it, then follows the harness that thread
starts on.

## Themes

`_rustcode/themes` (`{"cwd"?}`) answers `{"active", "themes": [{"name",
"description", "source", "path"?, "mode", "tokens", "radius"}]}` with every
desktop token resolved; `_rustcode/themes/set` (`{"name", "cwd"?}`) makes a
theme active and answers the same; `_rustcode/themes/changed` carries the
same snapshot when a theme file or the active theme changes (debounced
200 ms). See [themes.md](themes.md).

## Failure behavior

Per-session completion delivery is bounded so a disconnected or stalled client
cannot grow memory indefinitely. Overflow is explicit: the prompt returns an
error stating that the completion backlog overflowed and the session must be
reopened. RustCode does not silently drop a terminal completion and continue as
if delivery succeeded.

Malformed configuration and protocol errors are returned without overwriting
the user's configuration. Tool permission decisions continue to use the ACP
permission flow unless the server was deliberately started with `--yolo`.
