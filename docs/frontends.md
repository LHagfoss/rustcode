# Frontends

One core, one UI per platform. All feature and agent-loop work happens in the
`rustcode` library (`rustcode/engine/src` plus `rustcode/core/*`); each
frontend carries only its own rendering and input code.

## Current frontends

| Frontend | Location | How it links core |
| --- | --- | --- |
| Terminal UI | in-tree, behind the `tui` feature (default on) | same crate |
| Native desktop (GPUI, macOS) | `rustcode/desktop` (`rustcode-app`) | `rustcode = { path = "../..", default-features = false }` |
| ACP | in-tree (`engine/src/acp.rs`) | same crate |
| Daemon / headless | in-tree (`engine/src/daemon.rs`, CLI flags) | same crate |
| Mobile | remote only — see `docs/mobile.md` | JSON protocol, never links Rust |

## The seam

`rustcode::controller` (`engine/src/controller/`) is the UI-neutral contract
for controlling and observing a session: `InteractiveController`,
`ControllerHandle`, `ControllerEvent` / `ControllerUpdate`,
`ControllerSnapshot`, `Command`. New frontends drive this; `rustcode/desktop`
is the reference implementation.

The terminal UI predates the contract and still drives internals directly
(`app::runtime`, `network::ui_adapter`). Converging it onto `controller` is
the prerequisite for extracting it into its own crate — see issue #1430.
Until then, treat `controller` as the stable seam and the TUI's direct
internals use as legacy.

## Rules

- Nothing under `rustcode/core/` may reference `ratatui` or `crossterm`.
  Non-TUI frontends build core with `default-features = false`; CI checks
  `cargo check --locked --no-default-features` so that path cannot rot.
- Dependency direction is strictly frontend → core. `rustcode` must never
  depend on a frontend crate, not even optionally: Cargo rejects the cycle
  (`error: cyclic package dependency`) regardless of feature flags.
- Adding a frontend: add a package that depends on `rustcode` with
  `default-features = false` (unless it embeds the terminal UI), drive
  `controller`, and add its paths to `scripts/ci-relevant-changes.sh` so CI
  triggers on it.
