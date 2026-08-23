# Render Snapshots Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Render terminal frames from an immutable, revision-checked snapshot so the shared `AppState` mutex is held only while capturing and publishing state.

**Architecture:** Make `History` copy-on-write so a render snapshot can share stable conversation storage. Add a UI-owned `RenderSnapshot` containing the immutable values read by the renderer and a render revision; convert render functions to read that snapshot. The runtime captures a snapshot, renders after releasing the mutex, then publishes only current-frame bookkeeping if the revision still matches.

**Tech Stack:** Rust, Tokio, ratatui, `Arc`, existing `History` revision markers, existing `TranscriptState` projection.

**Spec:** `docs/superpowers/specs/2026-08-23-render-snapshots-design.md`

## Global Constraints

- Preserve current rendered output and event behavior.
- Keep network/provider code outside this change.
- Do not hold `AppState` across height calculation, widget layout, markdown/tool rendering, or terminal drawing.
- Reject stale frame bookkeeping publication when a newer render revision exists.
- Verify each task with focused tests, then `cargo check --tests` and `cargo test` before integration.

---

### Task 1: Share history snapshots with copy-on-write storage

**Files:**
- Modify: `src/app/state.rs:388-520`
- Test: `src/app/state.rs` history tests near the existing revision tests

**Interfaces:**
- Produces `History::snapshot(&self) -> History` with O(1) shared storage until mutation.
- Preserves existing `History` indexing, mutation, serialization, revision, and iterator interfaces.

- [ ] **Step 1: Write the failing test**

Add a test that captures `let snapshot = history.snapshot()`, mutates the live history, and asserts the snapshot still contains the original messages and revision while the live history contains the mutation.

- [ ] **Step 2: Run the focused test to verify it fails**

Run: `cargo test history_snapshot_is_stable_after_live_mutation`

Expected: FAIL because `History::snapshot` does not exist.

- [ ] **Step 3: Implement the minimal copy-on-write storage**

Store messages behind `Arc<Vec<ChatMessage>>`. Add `snapshot` that clones the `Arc` and preserves revisions. Before every mutable access, call `Arc::make_mut`; keep `drain` returning a standard vector drain after making the storage unique. Make `into_vec` use `Arc::try_unwrap` and clone only when another snapshot still exists.

- [ ] **Step 4: Run focused and existing history tests**

Run: `cargo test history_snapshot_is_stable_after_live_mutation history_revision_tracks_structural_and_in_place_mutations history_clone_keeps_snapshot_revision_without_sharing_storage`

Expected: PASS.

- [ ] **Step 5: Run the compiler gate**

Run: `cargo check --tests`

Expected: PASS with no errors.

- [ ] **Step 6: Commit**

```bash
git add src/app/state.rs
git commit -m "perf(state): share history snapshots"
```

### Task 2: Define the immutable UI render snapshot

**Files:**
- Create: `src/ui/render_snapshot.rs`
- Modify: `src/ui/mod.rs:1-60`
- Modify: `src/app/state.rs` near `AppState` methods
- Test: `src/ui/render_snapshot.rs`

**Interfaces:**
- Produces `pub(crate) struct RenderSnapshot` with immutable accessors for the values currently read by `desired_height`, `render_with_transcript`, modal rendering, composer rendering, live-tail rendering, and selected-subagent rendering.
- Produces `AppState::render_snapshot(&self) -> RenderSnapshot` and `RenderSnapshot::revision() -> u64`.
- Produces `AppState::publish_render_metrics(revision, height, input_area) -> bool`.

- [ ] **Step 1: Write the failing snapshot tests**

Test that a snapshot reports the same input, status, history, current response, modal state, and selected-subagent view as the source state. Test that publishing metrics succeeds for the captured revision and returns false after `request_redraw` or another state revision has advanced.

- [ ] **Step 2: Run the focused tests to verify they fail**

Run: `cargo test render_snapshot_captures_ui_state render_metrics_reject_stale_revision`

Expected: FAIL because `RenderSnapshot`, `render_snapshot`, and `publish_render_metrics` do not exist.

- [ ] **Step 3: Implement the snapshot projection**

Add the immutable projection in `src/ui/render_snapshot.rs`. Use the shared `History::snapshot` for root history and clone only small transient values. Include the active subagent history and metadata required by the existing selected-subagent renderer. Add a monotonic `render_revision` to `AppState`, increment it whenever render-visible state changes through the existing state update paths, and make metric publication revision-checked.

- [ ] **Step 4: Run focused tests and compile**

Run: `cargo test render_snapshot_captures_ui_state render_metrics_reject_stale_revision && cargo check --tests`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/app/state.rs src/ui/render_snapshot.rs src/ui/mod.rs
git commit -m "perf(ui): add immutable render snapshots"
```

### Task 3: Render exclusively from the snapshot

**Files:**
- Modify: `src/ui/mod.rs` render helpers and public compatibility wrappers
- Modify: `src/ui/composer.rs` render signature
- Modify: `src/ui/modals.rs` render signatures
- Modify: `src/ui/tests.rs`

**Interfaces:**
- `desired_height`, `render_with_transcript`, `render_live_tail_with_transcript`, modal render functions, composer render, and helper functions consume `&RenderSnapshot`.
- Existing unit-test wrappers may accept `&mut AppState`, construct a snapshot, render from it, and apply metrics.

- [ ] **Step 1: Write the render-equivalence regression test**

Render representative idle, streaming, approval, question, picker, and selected-subagent states through the snapshot path and compare the test backend buffer/text with the existing compatibility wrapper output.

- [ ] **Step 2: Run the regression test before conversion**

Run: `cargo test render_snapshot_preserves_existing_ui_output`

Expected: FAIL because the snapshot render entry point does not exist.

- [ ] **Step 3: Convert render-only functions to immutable input**

Replace `&mut AppState` with `&RenderSnapshot` wherever rendering does not mutate state. Move `conversation_content_height` and `input_text_area` updates to the wrapper/runtime publication path. Keep event handlers and modal actions unchanged.

- [ ] **Step 4: Run the focused UI suite**

Run: `cargo test ui::tests`

Expected: PASS with unchanged rendered assertions.

- [ ] **Step 5: Run the compiler gate**

Run: `cargo check --tests`

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/ui/mod.rs src/ui/composer.rs src/ui/modals.rs src/ui/tests.rs
git commit -m "refactor(ui): render from immutable state"
```

### Task 4: Release the state lock during runtime rendering

**Files:**
- Modify: `src/app/runtime.rs:740-790`
- Test: `src/app/runtime.rs` or `src/ui/tests.rs` runtime-focused tests

**Interfaces:**
- Runtime captures `RenderSnapshot` and revision under `app_state.lock().await`, drops the guard before `desired_height` and terminal drawing, then publishes metrics through the revision-checked method.

- [ ] **Step 1: Write the stale-frame publication test**

Capture a snapshot, advance the live state revision, attempt to publish the captured frame metrics, and assert that the newer state’s metrics remain unchanged.

- [ ] **Step 2: Run the focused test to verify it fails**

Run: `cargo test stale_render_metrics_cannot_overwrite_new_state`

Expected: FAIL until runtime publication is revision-checked.

- [ ] **Step 3: Change the draw loop ownership boundary**

Capture the snapshot and terminal dimensions while locked, release the lock, calculate height and call `draw_height` using the snapshot, then reacquire briefly to publish metrics. Keep terminal-local `TranscriptState` outside the lock as it is today.

- [ ] **Step 4: Run runtime/UI tests and full verification**

Run: `cargo test stale_render_metrics_cannot_overwrite_new_state && cargo check --tests && cargo test`

Expected: PASS with all tests passing.

- [ ] **Step 5: Run diff hygiene checks**

Run: `git diff --check`

Expected: no output and exit code 0.

- [ ] **Step 6: Commit**

```bash
git add src/app/runtime.rs src/app/state.rs src/ui
git commit -m "perf(ui): release state lock while rendering"
```
