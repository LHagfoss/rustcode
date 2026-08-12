# Status in Input Bar Design

## Goal

Keep the live interaction state visually attached to the prompt input.

## Design

- Render `Auto-Confirm`, `Context`, `Tps`, optional quota, and `Ctrl+P commands` together in the input box's bottom border.
- Remove the duplicate right-side status cluster from the one-row footer.
- Keep the footer's activity trail, activity label, and interrupt hint unchanged.
- Preserve existing context-token fallback, cached-token, quota, and streaming TPS behavior.

## Scope and verification

The change is limited to `src/ui/mod.rs` and its focused UI tests. Keyboard behavior and input-box title content remain unchanged. Verify with the focused UI tests, `cargo check --tests`, and `cargo test`.

