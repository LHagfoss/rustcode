# Inspectable agent threads

Subagent tools are available by default. `/delegate` arms them for the next
task only, `/delegate on` keeps them available for the session, and
`/delegate off` disables them for the session until an explicit `/delegate`
command. Set
`delegation_enabled = false` in the user `config.toml` to disable delegation
entirely; project config cannot override this user-level setting. `spawn_agent` returns
immediately with a JSON receipt: `agent_id` (the stable session-local numeric ID
every other agent tool takes as `id`), `nickname` (`agent-<id>`, the name used
in notices and the agent picker), `agent_type`, the resolved `model` and the
`status` the spawn left the child in. Children retain separate
histories, model context budgets and loop detectors. The existing configurable
`subagent_concurrency_limit` controls admitted execution; additional children
queue. A session supports up to 64 recorded children and three levels of
nesting. Nested delegation is read-only; write-enabled workers retain their
explicit path and isolated-workspace contracts and return a handoff before
further delegation.

Open the agent picker to see parent relationships, queued/running/terminal
status, model and elapsed time. Select a child to read its full conversation and
send additional instructions. Returning to main restores its scroll position
and bottom-follow setting. Child histories never replace the root transcript.

The model can use `list_agents` for metadata and `inspect_agent` for a bounded
tail (eight entries, 768 characters each). `send_message` queues additional
information at a safe request boundary and leaves idle children idle.
`followup_task` and the compatible `send_agent` alias start an idle child or
queue instructions for an active child. Mailboxes retain at most 32 messages;
each message and selected evidence are limited to 8192 bytes. Delivery preserves
arrival order and does not rewrite a request already in flight.

With `interrupt: true`, `followup_task` and `send_agent` redirect a running
child instead: its current turn is cancelled, the call returns once that turn
has cleaned up, and a new turn starts with the message as its instruction. Work
in flight is abandoned and children the interrupted turn started are cancelled
with it, as with any parent turn. The child keeps its history, the root gets no
completion notice for the stopped turn, and the new turn queues for an
execution slot like any other. A child that is idle or still queued has nothing
to interrupt and is handled as an ordinary follow-up. `send_message` is
queue-only and rejects `interrupt`; an agent cannot interrupt itself or an
ancestor.

`wait_agent` accepts `id`, or `ids` to wait for whichever of several children
finishes first, and optional `timeout_ms` (default 60000, range 1–3600000). It
wakes for completion or agent mailbox activity; a timeout leaves
the child available. Waiting children yield their execution slot so a parent
can join a nested child even with concurrency one. Self/ancestor waits are
rejected. `cancel_agent` cancels a child and its descendants; parent cancellation
propagates through child cancellation tokens. Admitted runners finish process
and blocking cleanup before publishing terminal completion. Session switches
signal cancellation; terminal shutdown waits for cleanup.

`spawn_agent` takes an optional `agent_type` role, listed in its schema:

- `default`: read-only unless `write_access` is passed.
- `explorer`: read-only investigation. Combining it with `write_access: true`
  is an error.
- `worker`: presets `write_access: true`. Combining it with
  `write_access: false` is an error.

A role is only a preset over `write_access`. A worker is held to everything an
explicit `write_access: true` is: `allowed_paths` is required, nested
delegation stays read-only, a write-enabled agent cannot delegate, and its
tool calls go through the same approval. An unknown `agent_type` is rejected
with the list of roles. Roles are built in; there are no user-defined roles.

Context inheritance is explicit in `spawn_agent`:

- `minimal` (default): task plus child system/project instructions.
- `evidence`: task plus caller-selected `evidence`.
- `recent`: selected evidence plus the last three complete parent user turns.
- `fork`: parent conversation and optional selected evidence.

Child policy/project instructions are rebuilt independently. Parent-local
system reminders and incomplete native assistant/tool groups are excluded.
Inherited histories are independent snapshots and are compacted to the child's
resolved model budget.

The parent receives a bounded structured completion containing status, summary,
truncation and workspace handoff, at most 8192 bytes after JSON encoding. It does
not receive the entire child transcript. Unknown verification or token facts
are not fabricated; inspect the child's real tool results and handoff when
more evidence is required.

Agent metadata, relationships, mailbox messages, histories, performance facts
and final summaries are atomically saved in the owning session's `agents.json`.
Resume validates relationships and marks previously queued/running work
`interrupted`; it does not restart processes. A followup reuses the preserved
history. Persisted terminal summaries remain inspectable and waitable.

Operational events include `subagent.spawn`, `subagent.spawn.finish`,
`subagent.queue.finish`, `subagent.model.finish`, `subagent.summary` and existing
start/finish events. In `context.request_composition`, `available_agent_tools`
counts the advertised tools that carry the delegation capability; those defined
as built-ins are also part of `available_builtin_tools`. Metadata exposes request/round/tool counts, model/tool
microseconds, context bytes and provider usage when supplied. Usage shown for a
round is the last provider request's reported usage; continuation counts remain
explicit. [Local Codex research](codex-multi-agent-research.md) records the
architectural sources and adaptations.

Children may send queue-only messages to root ID `0`. Root messages are
attributed evidence and drain before a model request, after announced tool
results are complete. Root history is flushed before acknowledging the mailbox
snapshot. Status notices redraw the UI without injecting asynchronous system
messages into native tool batches.
