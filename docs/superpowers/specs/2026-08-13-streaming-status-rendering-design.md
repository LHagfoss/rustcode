# Streaming Status Rendering Design

## Goal

Make assistant output remain visibly progressive across multiple lines, separate the live status from the composer with one blank row, and make the `Working` shimmer visibly travel across the word.

## Streaming transcript

Responses that begin with a reasoning block stay mutable until finalization so their compact thought presentation can be normalized once. While such a response is mutable, the live renderer will render the complete current response instead of only its unfinished final row. The renderer will continue hiding raw reasoning behind the existing compact `Thought` preview and will progressively show every answer line after the reasoning block closes.

Ordinary responses will keep the existing scrollback behavior: newline-complete rows are committed permanently and only the unfinished suffix remains mutable. Finalization will continue emitting each response exactly once.

## Working status spacing

The live conversation tail will append exactly one empty `Line` after the active status row. This gives `Working` one row of padding before the composer without changing spacing inside committed assistant messages.

## Shimmer animation

RustCode will retain its existing Ratatui span-based animation and active-response redraw cadence. The shimmer will use Codex's core motion parameters: a monotonic process-local clock, a two-second sweep, ten virtual character positions of padding on each side, and a cosine highlight band with a half-width of five positions.

The gradient will blend RustCode's themed muted and text colors so it remains visible in both dark and light themes. Animation math will be separated from the real clock so tests can inspect deterministic frames. No new dependency or terminal-palette subsystem will be introduced.

## Verification

Regression tests will prove that:

- a reasoning-prefixed stream displays both an earlier completed answer line and the current unfinished line before finalization;
- the active `Working` row is followed by one blank line;
- deterministic shimmer frames move the brightest span across `Working` and do not style the whole word with one unchanged foreground color.

Focused UI tests will run before the required `cargo check --tests` and full `cargo test` gates.
