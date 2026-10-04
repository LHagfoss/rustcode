# Inspectable agent threads

`/delegate` authorizes delegation for the next task. `spawn_agent` returns
immediately with a stable session-local numeric ID. Children retain separate
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

`wait_agent` accepts `id` and optional `timeout_ms` (default 60000, range
1–300000). It wakes for completion or agent mailbox activity; a timeout leaves
the child available. Waiting children yield their execution slot so a parent
can join a nested child even with concurrency one. Self/ancestor waits are
rejected. `cancel_agent` cancels a child and its descendants; parent cancellation
propagates through child cancellation tokens. Admitted runners finish process
and blocking cleanup before publishing terminal completion. Session switches
signal cancellation; terminal shutdown waits for cleanup.

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
start/finish events. Metadata exposes request/round/tool counts, model/tool
microseconds, context bytes and provider usage when supplied. Usage shown for a
round is the last provider request's reported usage; continuation counts remain
explicit. [Local Codex research](codex-multi-agent-research.md) records the
architectural sources and adaptations.

Children may send queue-only messages to root ID `0`. Root messages are
attributed evidence and drain before a model request, after announced tool
results are complete. Root history is flushed before acknowledging the mailbox
snapshot. Status notices redraw the UI without injecting asynchronous system
messages into native tool batches.
