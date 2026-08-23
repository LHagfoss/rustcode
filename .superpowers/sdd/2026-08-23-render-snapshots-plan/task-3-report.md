# Task 3 report: render exclusively from the snapshot

## Status

Implemented and verified on branch `perf/render-snapshots`, based on the approved Task 1/2 commits through `a114343`.

## Scope

- Added `render_snapshot_preserves_existing_ui_output`, comparing the immutable snapshot render against the compatibility render for idle, streaming, approval, question, picker, and selected-subagent states.
- Converted render-only helpers in `src/ui/mod.rs` to consume `&RenderSnapshot`, including live-tail rendering, layout sizing, input/composer rendering, status/footer rendering, committed-history projections, and modal support helpers.
- Converted composer rendering and modal render functions to immutable snapshot accessors.
- Extended `RenderSnapshot` with the remaining render-visible values needed by picker, context, completion, status, and selected-subagent rendering.
- Kept event/action handlers on `AppState`.
- Kept runtime code and runtime lock ownership unchanged.
- Preserved existing unit-test and runtime call compatibility with thin `AppState` wrappers that construct a snapshot and publish render metrics where needed.
- No `AppState` clone was introduced.

## Verification

- Regression test was first run before conversion and failed at the expected missing snapshot render signature.
- `cargo test ui::tests`: 132 passed, 0 failed.
- `cargo check --tests`: passed.
- `cargo test`: 941 passed, 0 failed.
- `git diff --check`: passed.

The compiler reports existing-style dead-code warnings for `AppState::active_history` and `AppState::auto_confirm_status_text`; they are no longer used by render paths because the snapshot owns those projections.

## Files

- `src/ui/mod.rs`
- `src/ui/composer.rs`
- `src/ui/modals.rs`
- `src/ui/render_snapshot.rs`
- `src/ui/tests.rs`

The pre-existing untracked `.worktrees/` directory was left untouched and is not part of the task changes.
