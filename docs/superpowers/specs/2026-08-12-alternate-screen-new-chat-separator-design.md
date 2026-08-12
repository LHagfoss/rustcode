# Alternate Screen and New Chat Separator Design

## Goal

Keep build and startup output from `cargo run` visible in the terminal's normal
scrollback while rustcode owns a clean full-screen UI, and make `/new` visibly
separate the retained previous transcript from the new-chat boundary.

## Design

Rustcode will enter the terminal's alternate screen before initializing the
Ratatui terminal. The existing full-screen clear and redraw behavior will remain
inside that alternate screen. On the normal shutdown path, rustcode will disable
its terminal features, leave the alternate screen, restore the cursor, and then
print the goodbye message. This leaves Cargo/rustc output in the primary screen
instead of overwriting it with the TUI.

The existing `✨ New chat started` system history marker will remain in the
transcript and model context. The UI renderer will recognize that marker and
render a single full-width separator with `✨ NEW CHAT` centered between line
segments. Other system messages will keep their current rendering. `/new` will
continue retaining the prior history; this change only improves its visual
boundary.

## Verification

- Add a focused renderer test proving the new-chat separator spans the complete
  content width and centers its label.
- Verify the alternate-screen startup/shutdown symbols compile through
  `cargo check --tests`.
- Run `cargo test` and inspect the final diff for unrelated changes.
