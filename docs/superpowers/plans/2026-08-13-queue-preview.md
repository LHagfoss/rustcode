# Queue Preview Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Render up to three queued user prompts directly above the composer without exposing internal wakeups.

**Architecture:** Add small UI-only helpers in `src/ui/mod.rs` to select visible user prompts, calculate dynamic preview height, and render its header and rows. The queue data and key handling remain in `AppState`; the renderer only filters and presents it.

**Tech Stack:** Rust, ratatui, existing RustCode UI unit tests.

## Global Constraints

- Preserve queue ordering and the existing Up-arrow edit-last behavior.
- Exclude `__task_wakeup__:` entries from queue count and rendering.
- Display no more than three one-line prompt previews, newest closest to the composer.
- Verify with `cargo check --tests` and `cargo test`.

---

### Task 1: Add the failing queue-preview integration test

**Files:**
- Modify: `src/ui/tests.rs`

**Interfaces:**
- Consumes: `render`, `AppState`, and ratatui's `TestBackend`.
- Produces: a rendered-screen regression test for visible queue behavior.

- [ ] **Step 1: Write the failing test**

Set `pending_queue` to one internal wakeup and four user prompts. Render an
80-column test terminal. Assert the buffer contains `queued (4) · ↑ edit last`
and the three newest prompts, while excluding the wakeup and oldest prompt.

- [ ] **Step 2: Verify it fails**

Run: `cargo test queue_preview_shows_recent_user_prompts_without_wakeups`

Expected: FAIL because the current renderer displays only the raw last queue
entry and includes wakeups.

### Task 2: Render the compact dynamic queue preview

**Files:**
- Modify: `src/ui/mod.rs`
- Test: `src/ui/tests.rs`

**Interfaces:**
- Consumes: `AppState::pending_queue`.
- Produces: `queued_user_prompts`, `queue_preview_height`, and queue preview
lines used by `render`.

- [ ] **Step 1: Select visible user prompts**

Filter internal wakeups, retain at most the three newest user prompts, then
restore queue order for display.

- [ ] **Step 2: Make layout height match visible rows**

Reserve zero rows for wakeups-only queues; otherwise reserve one header row
plus one row per visible prompt. Use that calculation in both composer layouts.

- [ ] **Step 3: Render header and one-line rows**

Use `queued (<full user count>) · ↑ edit last` as the muted header and `  ›`
as each primary-colored row prefix. Truncate preview text to fit its row.

- [ ] **Step 4: Verify the focused test passes**

Run: `cargo test queue_preview_shows_recent_user_prompts_without_wakeups`

Expected: PASS.

### Task 3: Verify and commit

**Files:**
- Modify: `docs/superpowers/specs/2026-08-13-queue-preview-design.md`
- Modify: `docs/superpowers/plans/2026-08-13-queue-preview.md`

- [ ] **Step 1: Run verification**

Run: `cargo check --tests && cargo test`

Expected: both commands exit 0.

- [ ] **Step 2: Commit the scoped change**

Run: `git add src/ui/mod.rs src/ui/tests.rs && git add -f docs/superpowers/specs/2026-08-13-queue-preview-design.md docs/superpowers/plans/2026-08-13-queue-preview.md && git commit -m "feat(ui): compact queued prompts"`

Expected: one focused commit.
