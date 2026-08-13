# Codex-Style Native-Scrollback Chat

## Goal

Keep rustcode's current keyboard-first visual language while making completed
conversation history ordinary terminal scrollback. The terminal, rather than a
window-height chat viewport, owns scrolling, selection, and copying.

## Scope

- Keep Ratatui for the live composer, current streaming tail, queue preview,
  and inline keyboard interactions.
- Insert completed and stable transcript rows above the live viewport into
  native terminal scrollback.
- Preserve existing message/tool visual styling for both committed rows and
  the live tail.
- Replace full-screen/modal picker behavior with transient keyboard-driven
  panels immediately above the composer.
- Show `Working...` as the sole active model-status label.
- Remove transcript mouse capture, hover states, copy badges, and application
  transcript scrolling.
- Fix the normal interactive theme to the current default; remove runtime
  theme switching so previously committed terminal rows never need repainting.
- Preserve agent execution, session persistence/resume, queued prompts,
  commands, tool approvals, questions, ACP, and `--prompt` behavior unless a
  requirement below explicitly changes their presentation.

## Non-goals

- Repaint or recolor already committed terminal scrollback after a resize or
  configuration change.
- Preserve mouse-driven text selection, hover styling, copy controls, or a
  full-screen picker.
- Replace Ratatui with a plain stdin/stdout REPL.

## Architecture

### Inline viewport

The running UI owns only a compact inline viewport at the bottom of the
terminal. Its contents are, from top to bottom:

1. the mutable tail of an assistant response or current tool activity;
2. `Working...` while a model turn is active;
3. a queued-prompt preview when a later prompt is waiting; and
4. the composer or the active inline keyboard interaction.

The viewport height is based on this live content and capped at a small,
explicit maximum. It is never sized to the whole terminal window and never
contains the full conversation history.

### Native scrollback transcript

Completed user messages, assistant messages, tool results, and visible system
notices are rendered by the existing message renderer into wrapped terminal
rows, then inserted immediately above the inline viewport. The terminal owns
those rows after insertion. Native wheel/trackpad scrolling, selection,
copying, and terminal search therefore work without application state.

On resume, persisted visible history is rendered once through the same
committer before the empty live composer is drawn.

### Streaming lifecycle

An assistant stream is split into a stable prefix and a mutable tail. Once a
newline-terminated row can no longer change, it is committed to scrollback
with the same styling as a completed assistant message. The live viewport
retains only the unfinished tail plus the composer area. At turn completion,
the remaining tail and final message decoration are committed exactly once.

Tool activity follows the same boundary: dynamic state remains in the live
area; completed tool output becomes a terminal-history block. This prevents
duplicate text while retaining the current-looking active chat block.

### Keyboard interactions

The composer remains the default key target. Queued input remains visible
above it and retains the existing FIFO behavior. Tool approval, agent
questions, and simple pickers occupy a transient panel directly above the
composer and receive keys first; confirming or cancelling them prints their
result into the transcript and restores normal input. No mouse interaction or
full-screen overlay is required.

### Fixed visual configuration

The existing default palette becomes the interactive-chat palette. The
runtime theme picker and theme-changing path are removed from this mode. A
terminal resize affects only future wrapping and the live area; committed
history keeps its original wrapping and colors, matching ordinary terminal
output.

## State and interfaces

- `AppState.history` remains the durable model/session transcript and is not
  discarded when a block is committed to terminal scrollback.
- A new transcript-commit boundary records which history entries and stream
  rows have been inserted, preventing repeats across redraws.
- The existing full-history `render_conversation` path is replaced by
  renderers that can produce either one committed message/tool block or the
  mutable live tail.
- `Terminal::insert_before` or an equivalent scroll-region writer performs
  history insertion above the current inline viewport and preserves the
  composer cursor position.

## Error handling

If a terminal history insertion fails, do not advance the commit boundary.
Request a redraw and retain the uncommitted content in the live area so a
later draw can retry without data loss or duplication. On terminal resize,
recompute only the live viewport; prior terminal scrollback is intentionally
not reconstructed.

## Verification

- Unit-test the stable-prefix splitter so it commits only newline-terminated
  rows and leaves the mutable tail intact.
- Unit-test commit-boundary behavior so redraws cannot duplicate transcript
  rows and completion commits the tail once.
- Unit-test live-area layout: it contains only the current tail, `Working...`,
  queue preview, and composer/inline interaction—not prior history.
- Unit-test that no Thinking/Responding label is rendered and that mouse,
  hover/copy, theme-switching, and application transcript-scroll paths are
  absent from normal interactive chat.
- Run `cargo check --tests` and `cargo test` before handoff.
