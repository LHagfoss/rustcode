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

## Optional fullscreen terminal mode (Phase 0)

The terminal UI stays inline by default. Set `fullscreen = true` in the
configuration file or pass `--fullscreen` to opt in. The startup capability
check is passive: it reads terminal attributes and environment variables
without sending queries or reading stdin, so it cannot delay the UI or
consume a user's first keypress. Cursor-report, color, and keyboard support
are estimates until a later phase negotiates those protocols.

| Environment | Phase 0 behavior |
| --- | --- |
| macOS Terminal (`xterm-*`) | Alternate screen when requested; standard keyboard input |
| Ghostty, Kitty, WezTerm | Alternate screen when requested; keyboard enhancement inferred where identified |
| tmux, screen | Inline fallback |
| SSH session | Inline fallback |
| Non-TTY, `TERM=dumb`, unknown terminal | Inline fallback |

Fullscreen releases the alternate screen on normal exit, panic, Ctrl-Z
suspension, and the terminal runtime's external-command handoff. The main
screen scrollback is never cleared as part of fullscreen exit.
