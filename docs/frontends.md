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
(enforced by `scripts/check-frontend-seam.sh`). It drives `controller` for
turns, config, skills, background tasks, and shared domain types. Its render
layer renders from `controller::RenderState`, a per-frame owned projection of
a live session (`controller::render_state`), in place of the `AppState`
snapshot bridge it used to read. Anything that needed an `AppState` method —
whether the turn is interruptible, whether an overlay owns the screen, the
active model profile / context window / tool protocol, the unexpired transient
notice — is resolved into the view by the engine, so a frontend never
re-derives engine policy. The TUI's event loop (`rustcode/tui/src/runtime`) is
out of scope by design: it moves with the frontend and drives turns directly.
The update prompt renders a binary-local `env!("CARGO_PKG_VERSION")` helper
plus the shared `rustcode_core::update` leaf, and render tests spawn
background tasks through `controller::spawn_background_task` — neither path
names engine internals. Treat `controller` as the stable seam for anything
new.

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
check is passive (`rustcode/tui/src/terminal_probe.rs`): it reads terminal
attributes and environment variables without sending queries or reading stdin,
so it cannot delay the UI or consume a user's first keypress. Cursor-report,
color, and keyboard support are estimates until a later phase negotiates those
protocols. Fullscreen remains opt-in; no default flip is planned here (#1450).

### Terminal matrix (#1450, code-verified 2026-09-30)

Source of truth is `terminal_probe::infer` plus the `terminal_probe` and
`terminal_runtime` unit tests and the `fullscreen_inline_fallback` golden.
Entries below describe implemented behavior, not hardware lab results — record
new terminal-specific bugs against #1450 instead of assuming one terminal
generalizes.

| Environment | Fullscreen | Mouse / selection / copy-paste | Fallback |
| --- | --- | --- | --- |
| macOS Terminal (`xterm-*`, direct TTY) | Alternate screen when requested; standard keyboard input | Wheel scroll, drag selection with edge scrolling, composer click-to-place; drag keeps highlight, copy only on explicit action (#1492); composer selection/copy/replace (#1493) | Inline when not requested; alternate screen released on exit/panic/Ctrl-Z suspension/handoff, scrollback preserved |
| Ghostty (`TERM_PROGRAM=ghostty`) | Alternate screen when requested; keyboard enhancement inferred | Same mouse/selection/clipboard path as above; clipboard reports confirmed vs. terminal-sent feedback in the footer | Same release guarantees as above |
| Kitty (`kitty`, `KITTY_WINDOW_ID`) | Alternate screen when requested; keyboard enhancement inferred | Same mouse/selection/clipboard path as above | Same release guarantees as above |
| WezTerm (`TERM_PROGRAM=wezterm`) | Alternate screen when requested; keyboard enhancement inferred | Same mouse/selection/clipboard path as above | Same release guarantees as above |
| tmux / screen (`TMUX`, `TMUX_PANE`, `screen*`) | Inline fallback (no takeover) | Mouse/selection/clipboard still handled by the inline surface where the multiplexer forwards events; no fullscreen dependency | Passive probe forces inline; `fullscreen_inline_fallback` golden covers it |
| SSH (`SSH_TTY` / `SSH_CONNECTION` / `SSH_CLIENT`) | Inline fallback (remote forwarding unverified) | Same inline interaction path; no remote-specific protocol assumed | Passive probe forces inline |
| Non-TTY, `TERM=dumb` / `unknown` / `emacs`, legacy `vt100` | Inline fallback | Keyboard-only path; no alternate-screen or cursor-report claims | `TerminalCapabilities::unsupported()` |

Follow-up interaction issues #1492 (drag keeps highlight, explicit copy),
#1493 (composer selection/copy/replace), #1494 (adaptive activity spacing), and
#1495 (truthful Preparing/queued/running tool status) are all closed; their
behavior is covered by TUI render/interaction tests. Palette contrast and
reduced-motion remain covered by the render goldens
(`rustcode/tui/src/ui/fixtures/render_snapshot_*.txt`) and the
reduced-motion activity-label tests. Headless/serve behavior is unchanged.

Fullscreen releases the alternate screen on normal exit, panic, Ctrl-Z
suspension, and the terminal runtime's external-command handoff. The main
screen scrollback is never cleared as part of fullscreen exit.
