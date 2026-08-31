# Runtime and workspace architecture

RustCode is a Rust workspace with a root application crate and small domain
crates. The extracted crates keep frequently edited functionality from forcing
unrelated heavyweight dependencies to rebuild.

## Workspace crates

| Crate | Responsibility |
| --- | --- |
| `rustcode-core` | Stable shared domain types and executable-path helpers |
| `rustcode-session` | Session persistence primitives |
| `rustcode-tool-protocol` | Tool-call protocol parsing and envelopes |
| `rustcode-tools` | Filesystem-oriented tool implementations |
| `rustcode-command` | Cross-platform command execution and bounded output |
| `rustcode-lifecycle` | Turn lifecycle and stop-state types |
| `rustcode-tasks` | Session-aware background task state and event delivery |
| `rustcode-loop-detect` | Semantic loop, failure, progress, and reasoning guards |

The root `rustcode` crate owns integration concerns: TUI state, model/network
turns, ACP, configuration, media/audio tools, and adapters between the smaller
crates.

## Command and task flow

```text
model tool call
    -> tool protocol envelope
    -> network tool execution
    -> root tool dispatcher
    -> rustcode-command (foreground)
       or rustcode-tasks + rustcode-command (background)
    -> session-scoped event consumer
    -> history + UI/headless/ACP continuation
```

The task manager is process-scoped, but all listing, status, cancellation, and
event routing enforce session ownership. Consumers subscribe before work can
spawn so fast commands cannot complete before an observer exists.

Interactive, headless, and ACP consumers have separate adapters:

- The interactive runtime preserves inactive-session subscriptions until their
  terminal events have been drained.
- Headless execution tracks only tasks created by its current turn.
- ACP uses a server-owned router with bounded per-session delivery, durable
  persistence, provider call-ID correlation, and ordered terminal updates.

## Concurrency rules

- OS process termination is performed outside the task-state mutex.
- A `Terminating` state retains a racing process completion until cancellation
  commits its outcome.
- Terminal transitions are idempotent and publish once.
- Terminal IDs are retained in a bounded ledger for race classification.
- Event publication is synchronized with quiescence checks so a consumer cannot
  prune a subscription while its terminal event is still in flight.
- Slow task subscribers are disconnected rather than allowed to block workers.

## Build and CI boundaries

Changes under `crates/` and CI helper scripts trigger the required Linux test
job and macOS/Windows portability checks. Release artifacts are built for:

- Linux x86_64
- macOS Apple Silicon (ARM64)
- Windows x86_64

Intel macOS is intentionally not part of the release matrix.

Use [`scripts/bench-build-boundaries.md`](../scripts/bench-build-boundaries.md)
to measure clean, warm, and focused-edit Cargo rebuild costs without cleaning a
shared target directory.
