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
directory becomes the tool workspace root. Model and tool settings come from
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

## Failure behavior

Per-session completion delivery is bounded so a disconnected or stalled client
cannot grow memory indefinitely. Overflow is explicit: the prompt returns an
error stating that the completion backlog overflowed and the session must be
reopened. RustCode does not silently drop a terminal completion and continue as
if delivery succeeded.

Malformed configuration and protocol errors are returned without overwriting
the user's configuration. Tool permission decisions continue to use the ACP
permission flow unless the server was deliberately started with `--yolo`.
