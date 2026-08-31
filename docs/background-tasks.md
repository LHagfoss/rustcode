# Background tasks and cancellation

RustCode uses background commands for work that must continue while an agent
turn is paused, such as development servers, file watchers, and long test
runs. The task manager is shared by the process, while ownership, visibility,
events, and cancellation are scoped to a RustCode session.

## Starting a task

The model starts a task through `run_command`:

```json
{
  "command": "npm run dev",
  "background": true,
  "timeout_ms": 120000
}
```

RustCode immediately returns a pending result containing a generated task ID.
When the command exits, RustCode injects exactly one terminal result into the
originating session and resumes the paused logical turn where supported.

Short discovery commands such as simple file or environment inspection may be
kept in the foreground even when `background` is requested. Background mode is
primarily for long-running or mutating commands.

## Waiting and task management

Completion notifications are automatic. Do not repeatedly call `manage_task`
to poll for completion. Stop issuing tools and allow the harness to wait.

`manage_task` supports:

- `list`: list live tasks owned by the active session.
- `status`: inspect one live task owned by the active session.
- `kill`: request cancellation of one task owned by the active session.

A kill requested before the child publishes its process ID is reported as a
request, not as an already completed termination. RustCode applies it as soon
as the process starts. A task ID belonging to another session is treated as
not found.

## Process cleanup

On Unix, commands run in a process group and cancellation or timeout terminates
the group. On Windows, RustCode terminates the process tree. This prevents
shell children, test runners, watchers, and servers from becoming orphans.

Foreground commands use the same process-tree cleanup behavior when their turn
is cancelled or their timeout expires.

## Session behavior

- Interactive completions are routed by the task event's immutable session ID.
- Switching sessions does not discard the previous session's completion.
- Headless prompts wait only for background tasks created by that prompt, not
  unrelated tasks that were already running.
- ACP persists completion even if the prompt was cancelled, but never resumes
  a cancelled or stale prompt.
- Closing an ACP session cancels its active turn and its live tasks.

Task completion queues and terminal-ID ledgers are bounded. If an ACP consumer
falls far enough behind to overflow its completion backlog, RustCode returns an
explicit error asking the client to reopen the session instead of silently
losing a completion or hanging indefinitely.

## Delivery guarantees

For a successfully registered task, RustCode guarantees:

1. At most one terminal task event.
2. The pending/in-progress state precedes the terminal state.
3. The terminal state retains the provider tool-call ID when one exists.
4. Cancellation and completion races resolve to one terminal outcome.
5. A slow or disconnected observer cannot block task lifecycle transitions.

These are lifecycle guarantees, not command-success guarantees. A command can
still exit unsuccessfully, time out, or fail to spawn; that outcome is returned
as its terminal tool result.
