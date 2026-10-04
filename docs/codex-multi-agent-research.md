# Local Codex multi-agent research

The implementation was informed by the local checkout at `/Users/lagos/code/codex`,
inspected during the performance/context epic. This is architectural inspiration,
not copied implementation code.

Codex separates durable agent identity from loaded runtimes and running turns.
`codex-rs/core/src/agent/registry.rs` owns stable IDs and canonical paths;
`agent/control/runtime.rs` shares a registry and limits across an agent tree and
uses a weak thread-manager reference to avoid ownership cycles. RustCode retains
its numeric wire IDs, session-owned agent records and existing supervisor.

`agent/control/spawn.rs` reserves capacity, creates a child, persists the spawn
edge and admits initial input before committing. `agent/control/spawn_guard.rs`
cleans up abandoned spawn attempts, including ordering persisted Open/Closed
edge writes. RustCode must preserve the same ownership invariant using its own
session storage and cancellation tokens.

`agent/control/delivery.rs`, `agent/control/api.rs` and
`session/input_queue.rs` distinguish queue-only messages from followups that
start an idle turn. Mailbox delivery is FIFO and takes place at safe turn
boundaries. `tools/handlers/multi_agents_v2/wait.rs` subscribes before checking
pending activity and wakes on mailbox activity, steering or deadline.

`agent/status.rs` derives lifecycle state from events and treats interruption as
resumable. `agent/control/completion.rs` routes captured terminal results to the
parent without retaining child turn contexts. `agent/control/watch.rs` subscribes
before obtaining the initial status snapshot and coalesces later updates.

Fork handling in `agent/control/spawn.rs` explicitly filters inherited rollout
items, preserves full-history prompt baselines, drops parent-local usage and
authorization state, and rewrites child instructions. RustCode uses explicit
inheritance and its existing provider-native message representation, preserving
assistant/tool pairs and child history isolation.

`agent/control/execution.rs` separates running capacity from identity count;
`agent/control/residency.rs` separates loaded runtime capacity and idle eviction.
`agent/control/spawn.rs` restores metadata without automatically reopening every
V2 runtime and validates recorded parent/environment ownership on reload.
RustCode does not need Codex's guardian authorization, encrypted communication,
hosted control backends or runtime eviction system to implement inspectable
session-local children.

TUI presentation in `tui/src/app/agent_navigation.rs` keeps first-seen spawn
order rather than sorting IDs. `tui/src/app/agent_status_feed.rs` deduplicates
activity by item ID and bounds previews; `tui/src/multi_agents.rs` separates
presentation from orchestration. RustCode keeps separate child transcripts and
restores the previous transcript's scroll position on navigation.
