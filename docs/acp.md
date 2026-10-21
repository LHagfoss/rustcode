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

Two extension requests back the app's Settings pages:

- `_rustcode/capabilities` (`{}`) answers with `{"skills": [...],
  "mcpServers": [...], "configPath"}`. A skill has `name`, `description`,
  `path`, `source` (`workspace`, `universal`, `rustcode` or `configured`) and
  `enabled`. A server has `name`, `transport` (`stdio` or `http`), `target`
  (credentials masked), `enabled`, `scope` (`user`, `project` or `session`),
  `connection` (`connected`, `not_connected`, `disabled` or `failed`), `tools`
  and `error`. Header and environment values are never sent.
- `_rustcode/capabilities/set` (`{"kind": "skill" | "mcp", "name", "enabled"}`)
  saves the toggle to the user config, starts or stops an MCP server in this
  engine, and answers with the fresh snapshot. An unknown skill or a server the
  user config does not declare is an `invalid_params` error; a server that
  fails to start is reported as `failed` in the snapshot.

Semantics and scope are described in
[configuration.md](configuration.md#turning-skills-and-mcp-servers-on-and-off).

## Failure behavior

Per-session completion delivery is bounded so a disconnected or stalled client
cannot grow memory indefinitely. Overflow is explicit: the prompt returns an
error stating that the completion backlog overflowed and the session must be
reopened. RustCode does not silently drop a terminal completion and continue as
if delivery succeeded.

Malformed configuration and protocol errors are returned without overwriting
the user's configuration. Tool permission decisions continue to use the ACP
permission flow unless the server was deliberately started with `--yolo`.
