# Frontends

One core, one UI per platform. All feature and agent-loop work happens in the
`rustcode` library (`rustcode/engine/src` plus `rustcode/core/*`); each
frontend carries only its own rendering and input code.

## Current frontends

| Frontend | Location | How it links core |
| --- | --- | --- |
| Terminal UI | `rustcode/tui` (`rustcode-tui`, owns the `rustcode` binary) | `rustcode = { path = "../.." }` |
| Native desktop (GPUI, macOS) | `rustcode/desktop` (`rustcode-app`) | `rustcode = { path = "../.." }` |
| ACP | in-tree (`engine/src/acp.rs`, driven by the CLI) | same crate |
| Daemon / headless | in-tree (`engine/src/daemon.rs`, CLI flags) | same crate |
| Mobile | remote only — see `docs/mobile.md` | JSON protocol, never links Rust |

## The seam

`rustcode::controller` (`engine/src/controller/`) is the UI-neutral contract
for controlling and observing a session: `InteractiveController`,
`ControllerHandle`, `ControllerEvent` / `ControllerUpdate`,
`ControllerSnapshot`, `Command`. New frontends drive this; `rustcode/desktop`
is the reference implementation.

The terminal UI lives in its own crate but still drives core internals
directly (turn control, `network::ui_adapter`) alongside `controller`.
Converging the render layer onto `controller` (issue #1431, enforced by
`scripts/check-frontend-seam.sh`) keeps shrinking that surface; the TUI's
event loop moves with the frontend by design. Treat `controller` as the
stable seam for anything new.

## Rules

- Nothing under `rustcode/core/`, `rustcode/engine/`, or
  `rustcode/desktop/` may reference `ratatui` or `crossterm`; the terminal
  stack lives only in `rustcode/tui`.
- Dependency direction is strictly frontend → core. `rustcode` must never
  depend on a frontend crate, not even optionally: Cargo rejects the cycle
  (`error: cyclic package dependency`) regardless of feature flags.
- Adding a frontend: add a package that depends on `rustcode`, drive
  `controller`, and add its paths to `scripts/ci-relevant-changes.sh` so CI
  triggers on it.
