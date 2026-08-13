# Streaming Status Rendering Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Keep multi-line reasoning-prefixed answers visible during streaming, add one row beneath `Working`, and make the status shimmer visibly traverse the word.

**Architecture:** Keep ordinary terminal scrollback unchanged. Add one scrollback policy helper that returns the entire mutable text only for streams already held from permanent scrollback, and use the existing live assistant renderer for compact reasoning plus progressive answer text. Refine the existing shimmer into a deterministic time-parameterized helper wrapped by a monotonic process clock.

**Tech Stack:** Rust, Ratatui, Cargo unit tests

## Global Constraints

- Preserve raw reasoning hiding and final transcript de-duplication.
- Preserve unrelated uncommitted source changes.
- Add no dependency and perform no unrelated refactor or formatting.
- Verify with `cargo check --tests` and `cargo test`.

---

### Task 1: Progressive reasoning-prefixed stream rendering

**Files:**
- Modify: `src/ui/scrollback.rs`
- Modify: `src/ui/mod.rs`
- Test: `src/ui/tests.rs`

**Interfaces:**
- Produces: `scrollback::mutable_stream_text(text: &str) -> String`
- Consumes: existing `stream_starts_with_thought`, `split_stable_rows`, and `render_assistant_message`

- [ ] **Step 1: Write the failing regression test**

Add a UI test whose `current_response` is `"<think>\nPlanning\n</think>\n\nFirst answer line\nSecond answer line"` and assert `render_live_tail` contains both answer lines before finalization.

- [ ] **Step 2: Verify the test fails for the reported symptom**

Run: `cargo test ui::tests::reasoning_prefixed_stream_keeps_completed_answer_lines_live -- --exact`

Expected: FAIL because only `Second answer line` is currently rendered.

- [ ] **Step 3: Implement the narrow stream policy**

Add:

```rust
pub(crate) fn mutable_stream_text(text: &str) -> String {
    if stream_starts_with_thought(text) {
        text.to_owned()
    } else {
        split_stable_rows(text).1
    }
}
```

Use this helper instead of directly taking `split_stable_rows(...).1` in `render_live_tail`.

- [ ] **Step 4: Verify the focused test passes**

Run: `cargo test ui::tests::reasoning_prefixed_stream_keeps_completed_answer_lines_live -- --exact`

Expected: PASS.

### Task 2: Working-row padding and Codex-style shimmer

**Files:**
- Modify: `src/ui/mod.rs`
- Test: `src/ui/tests.rs`

**Interfaces:**
- Produces: deterministic `shimmer_spans_at(text: &str, elapsed: Duration) -> Vec<Span<'static>>`
- Consumes: `COLOR_MUTED`, `COLOR_TEXT`, existing active-response redraw cadence

- [ ] **Step 1: Write failing status tests**

Add one test asserting the final line from a streaming `render_live_tail` is blank. Add one deterministic shimmer test at the midpoint of the sweep asserting `Working` contains more than one foreground color and that the brightest position differs from the base positions.

- [ ] **Step 2: Verify both tests fail for the current behavior**

Run the two exact UI tests with `cargo test`.

Expected: the padding test fails because `Working` is the final row; the shimmer test fails because the deterministic helper is absent.

- [ ] **Step 3: Implement status spacing and animation**

Append one empty `Line` after the active status row. Replace wall-clock shimmer math with a `OnceLock<Instant>` wrapper and a deterministic helper using a two-second period, ten-position padding, five-position cosine half-width, and themed muted-to-text RGB blending. Apply bold styling throughout the word so color motion remains the visible signal.

- [ ] **Step 4: Verify focused UI behavior**

Run the exact padding and shimmer tests, then run all `ui::tests`.

Expected: PASS.

### Task 3: Repository verification and delivery

**Files:**
- Verify only

**Interfaces:**
- Consumes: completed implementation and repository workflow
- Produces: verified commit and pull request merged into `main`

- [ ] **Step 1: Run required compile gate**

Run: `cargo check --tests`

Expected: exit 0.

- [ ] **Step 2: Run full test gate**

Run: `cargo test`

Expected: exit 0 with zero failed tests.

- [ ] **Step 3: Review and commit only task changes**

Inspect `git diff`, stage only the streaming, padding, shimmer, tests, design, and plan hunks, and commit without staging unrelated working-tree edits.

- [ ] **Step 4: Publish and merge**

Push `fix/streaming-status-shimmer`, open one PR into `main`, merge it, checkout `main`, and pull the merged result while preserving unrelated local modifications.
