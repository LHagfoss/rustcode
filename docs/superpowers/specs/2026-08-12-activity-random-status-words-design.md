# Activity Random Status Words Design

## Goal

Make the compact input-bar activity status feel more expressive while preserving its existing animation and interrupt affordance.

## Design

- Rename the idle activity label from `Ready` to `Idle`.
- During streaming, replace `Working · Responding` with the historical RustCode status phrases:
  `Thinking...`, `Analyzing code...`, `Consulting the oracle...`, `Brewing coffee...`,
  `Refactoring reality...`, `Checking documentation...`, `Optimizing loops...`,
  `Debugging the universe...`, `Synthesizing solutions...`, and `Querying knowledge base...`.
- Select one phrase from the elapsed generation time, changing every three seconds; this keeps rendering deterministic and avoids introducing new mutable UI state.
- Preserve elapsed seconds and the existing `esc interrupt` hint for active work.
- Add one trailing space to the left activity title inside the input border so the label has visual padding before the border line.
- Keep tool-running, queued, action-required, and question/confirmation details unchanged.

## Scope and verification

The change is limited to activity classification/rendering and focused tests. Verify phrase selection, idle labeling, preserved activity details, and padding-related output with focused Rust tests, then run `cargo check --tests` and `cargo test`.

