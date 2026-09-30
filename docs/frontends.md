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

The experimental `rustcode serve` command is the TCP transport for remote
frontends. It drives `rustcode::controller` and is documented in
[`mobile.md`](mobile.md).

## The seam

`rustcode::controller` (`engine/src/controller/`) is the UI-neutral contract
for controlling and observing a session: `InteractiveController`,
`ControllerHandle`, `ControllerEvent` / `ControllerUpdate`,
`ControllerSnapshot`, `Command`. New frontends drive this; `rustcode/desktop`
is the reference implementation.

The terminal UI lives in its own crate and follows the same seam
(enforced by `scripts/check-frontend-seam.sh`). Its render layer renders from
`controller::RenderState`, a per-frame owned projection of a live session
(`controller::render_state`), in place of the `AppState` snapshot bridge it
used to read. Anything that needed an `AppState` method — whether the turn is
interruptible, whether an overlay owns the screen, the active model profile /
context window / tool protocol, the unexpired transient notice — is resolved
into the view by the engine, so a frontend never re-derives engine policy. The
TUI's event loop (`rustcode/tui/src/runtime`) is out of scope by design: it
moves with the frontend and drives turns directly.

`controller` also re-exports the shared domain types a frontend renders —
session status, chat history and tool records, live tool calls, subagents,
pending confirmations, approval/question answers, and `UiRect`. Prefer
extending the view over reaching past `controller`.

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
