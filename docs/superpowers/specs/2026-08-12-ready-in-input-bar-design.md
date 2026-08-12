# Ready in Input Bar Design

## Goal

Make the bordered prompt the single compact status surface.

## Design

- Render the Ready/activity animation and label as a left-aligned bottom title inside the input border.
- Render Auto-Confirm, Context, Tps, optional quota, and Ctrl+P commands as a right-aligned bottom title in the same row.
- Add one-column end padding and use two spaces between status items.
- Remove the separate footer row and retain existing activity details and interrupt hints.

## Verification

Preserve keyboard behavior and existing status calculations. Verify with focused UI tests, `cargo check --tests`, and `cargo test`.

