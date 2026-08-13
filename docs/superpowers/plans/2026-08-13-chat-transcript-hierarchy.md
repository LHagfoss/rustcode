# Chat Transcript Hierarchy Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make user and assistant turns visually distinct, preserve natural Markdown reflow, and show detailed live Working status.

**Architecture:** `render_assistant_message` continues to own code-panel extraction but sends untouched prose blocks to the Markdown renderer, then prefixes resulting lines with a speaker gutter. `render_committed_history_block` handles the user gutter/body style, while `render_live_tail` reuses the existing activity status component.

**Tech Stack:** Rust 2024, Ratatui `Line`/`Span`, existing Markdown renderer and UI tests.

## Global Constraints

- Preserve native terminal scrollback and the compact inline viewport.
- Do not change tool, queue, picker, theme, or resize behavior.
- Verify with `cargo check --tests` and `cargo test`.

---

### Task 1: Render transcript hierarchy

**Files:**
- Modify: `src/ui/mod.rs:397-719,2116-2156,2160-2235`
- Modify: `src/ui/tests.rs`

- [ ] **Step 1: Write failing tests**

Add tests that assert a soft-wrapped assistant paragraph begins with `• ` and continuation rows begin with `  `, a committed user body does not have `BOLD`, and a streaming live tail contains the formatted Working line.

- [ ] **Step 2: Run tests to verify RED**

Run: `cargo test ui::tests::assistant_messages_use_a_gutter_after_soft_reflow && cargo test ui::tests::committed_user_messages_keep_regular_body_text && cargo test ui::tests::live_tail_uses_formatted_working_status`

Expected: FAIL because the current renderer pre-wraps prose, makes user bodies bold, and emits a bare `Working...` row.

- [ ] **Step 3: Implement the smallest rendering changes**

Remove normal-prose word wrapping from `render_assistant_message`; retain only code-fence detection. Prefix Markdown output with `• ` on the first nonblank row and `  ` for continuation rows. Change the user body modifier to `Modifier::empty()`. Replace the bare live status with `activity_status_line(state, false)`.

- [ ] **Step 4: Run focused tests to verify GREEN**

Run the three focused test commands from Step 2. Expected: PASS.

- [ ] **Step 5: Run project verification**

Run: `cargo check --tests && cargo test && git diff --check`

- [ ] **Step 6: Commit**

```bash
git add src/ui/mod.rs src/ui/tests.rs docs/superpowers
git commit -m "feat(ui): improve chat transcript hierarchy"
```
