# Runtime and workspace architecture

RustCode is a Rust workspace with a root application crate and small domain
crates. The extracted crates keep frequently edited functionality from forcing
unrelated heavyweight dependencies to rebuild.

## Repository layout

```text
rustcode/
├── core/       Domain libraries. No terminal UI, no frontend assumptions.
│   ├── rustcode-core/         stable shared types and path helpers
│   ├── rustcode-session/      session persistence primitives
│   ├── rustcode-tool-protocol/ tool-call protocol envelopes
│   ├── rustcode-tools/        filesystem tool implementations
│   ├── rustcode-command/      cross-platform command execution
│   ├── rustcode-lifecycle/    turn lifecycle and stop-state types
│   ├── rustcode-tasks/        background task state and event delivery
│   └── rustcode-loop-detect/  semantic loop and progress guards
├── engine/     Sources of the root `rustcode` crate: agent loop, tools,
│               config, session state, ACP, daemon, and the terminal
│               frontend (behind the `tui` feature).
└── desktop/    `rustcode-app`, the native GPUI shell.
```

The root `Cargo.toml` is both the workspace manifest and the `rustcode`
package manifest; the engine lives in `rustcode/engine/src` and is declared
with `[lib] path` / `[[bin]] path`.

The engine exposes two feature sets. `tui` (on by default) builds the
terminal frontend — ratatui, crossterm, syntect, pulldown-cmark. Frontends
that never draw a frame build with `default-features = false`; CI checks
`cargo check --no-default-features` so that path cannot rot. Nothing under
`rustcode/core` may reference ratatui or crossterm. See `docs/frontends.md`
for the frontend contract and `docs/mobile.md` for the remote-only mobile
decision.

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

The root `rustcode` crate owns integration concerns: model/network turns,
ACP, configuration, media/audio tools, the terminal UI state, and adapters
between the smaller crates.

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

Changes under `rustcode/` and CI helper scripts trigger the required Linux test
and lint jobs, plus advisory macOS/Windows portability checks. The required
test and lint jobs run in parallel so merges do not wait for their combined
compile time. Release artifacts are built for:

- Linux x86_64
- macOS Apple Silicon (ARM64)
- Windows x86_64

Intel macOS is intentionally not part of the release matrix.

The `Build` workflow runs for version tags and manual dispatches only. A push
to `main` does not rebuild all release targets; the tag build is the single
source of published artifacts. Use manual dispatch on a branch when a release
binary needs validation without creating a release.

The release script performs a lightweight local preflight, opens a release PR,
waits only for required PR checks, and then tags the merged commit. Use
`--full-verify` when a complete local test run is desired before opening the
release PR.

Use [`scripts/bench-build-boundaries.md`](../scripts/bench-build-boundaries.md)
to measure clean, warm, and focused-edit Cargo rebuild costs without cleaning a
shared target directory.
