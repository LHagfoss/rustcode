# Task 4 Report: Release the State Lock While Rendering

Implemented on `perf/render-snapshots` after the completed Task 3 commits.

## Scope

- Added `stale_render_metrics_cannot_overwrite_new_state`, covering captured revisions and preserving newer metrics when a stale frame publishes.
- Changed the runtime draw boundary to capture `RenderSnapshot`, terminal dimensions, title/progress inputs, and clear-screen state under the `AppState` lock.
- Released the lock before scrollback projection, desired-height calculation, title/progress terminal writes, and terminal drawing.
- Reacquired the lock after drawing only to publish frame metrics through `publish_render_metrics`, preserving the revision check.
- Converted runtime scrollback and live rendering calls to the snapshot-only UI APIs while preserving terminal-local transcript, title, progress, scrollback, and event handling behavior.
- Fixed finalized assistant scrollback handling so a `None` stream remainder renders the snapshot history block, while `Some("")` still emits only its separator. Added focused coverage for both cases.

## Verification

- `cargo test stale_render_metrics_cannot_overwrite_new_state`: passed.
- `cargo test app::runtime::tests`: 12 passed.
- `cargo test ui::tests`: 133 passed.
- `cargo check --tests`: passed.
- `cargo test`: 944 passed.
- `git diff --check`: passed.

The stale-metrics test passed before the runtime boundary change because Task 2 had already implemented revision-checked publication; the runtime refactor was then verified with the same regression and full suites.

## Concerns

- Existing dead-code warnings remain for `app::transcript::TranscriptState::history_len`, `AppState::transcript`, and two `AppState` methods; the former two became unused after runtime rendering moved to `RenderSnapshot`.
- Pre-existing untracked `.worktrees/` was left untouched.
