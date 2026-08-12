# Conditional Tool-Result Transcript Spacing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Preserve model history across `/new` and `/clear`, hide only the cleared UI range, and add one conditional blank row before the next assistant transcript block.

**Architecture:** Keep spacing decisions and display-boundary handling in the existing TUI renderer. Add a transient `AppState` display boundary for `/clear`, preserve `history` in `start_new_session`, and include the boundary in the render cache key. Cover the history semantics and spacing predicate with focused unit tests.

**Tech Stack:** Rust, ratatui `Line`, existing `ChatMessage` history model, Cargo tests.

## Global Constraints

- Keep the change scoped to transcript spacing.
- Preserve existing compact spacing between consecutive tool results.
- Preserve existing user-turn separator behavior.
- Do not add dependencies or refactor unrelated rendering.

---

### Task 1: Add conditional tool-result spacing

**Files:**
- Modify: `src/ui/mod.rs` near `tool_result_follows` and the tool-result branch in `render_conversation`
- Test: `src/ui/mod.rs` test module

**Interfaces:**
- Consumes: `&[ChatMessage]` history and the current tool-result index.
- Produces: a private predicate used by `render_conversation` to decide whether to append one blank `Line`.

- [ ] **Step 1: Write the failing test**

Add a unit test for the predicate covering direct assistant, hidden-notice-plus-assistant, user, and consecutive-tool transitions.

- [ ] **Step 2: Run the focused test and verify it fails**

Run: `cargo test ui::tests::tool_result_spacing_targets_next_assistant -- --exact`

Expected: FAIL because the predicate does not exist yet.

- [ ] **Step 3: Implement the minimal predicate and renderer branch**

Skip hidden system notices after the tool result. Append `Line::from("")` only when the next visible message has role `assistant`; leave the existing user-only blank condition intact.

- [ ] **Step 4: Run the focused test and verify it passes**

Run: `cargo test ui::tests::tool_result_spacing_targets_next_assistant -- --exact`

Expected: PASS.

- [ ] **Step 5: Run the required project checks**

Run: `cargo check --tests` and `cargo test`

Expected: both commands pass.

- [ ] **Step 6: Commit the implementation**

```bash
git add src/ui/mod.rs docs/superpowers/specs/2026-08-12-tool-result-assistant-spacing-design.md docs/superpowers/plans/2026-08-12-tool-result-assistant-spacing.md
git commit -m "fix(ui): space tool results before assistant blocks"
```

### Task 2: Preserve history while clearing or starting chats

**Files:**
- Modify: `src/app/state.rs` to store the transient transcript display boundary
- Modify: `src/app/actions.rs` to preserve history in `/new`, hide it in `/clear`, and restore full display on `/resume`
- Modify: `src/main.rs` to apply the same `/clear` and `/new` behavior from the command picker
- Modify: `src/ui/mod.rs` to render only the visible history range and invalidate the cache when it changes
- Test: `src/app/actions.rs` tests for `/clear` and `/new` history behavior

**Interfaces:**
- Consumes: `AppState.history` and command handlers.
- Produces: `AppState.history_display_start`, a transient index into the retained history used only by the TUI.

- [ ] **Step 1: Write regression tests**

Verify `/clear` retains history and sets the display boundary, while `/new` retains history and resets the boundary to zero.

- [ ] **Step 2: Run focused tests to verify they fail**

Run: `cargo test app::actions::tests::clear_and_new_preserve_history -- --exact`

Expected: FAIL because the display boundary and preservation behavior do not exist yet.

- [ ] **Step 3: Implement the transient display boundary**

Add `history_display_start: usize` initialized to zero. Set it to `history.len()` for `/clear`; keep history unchanged and reset it to zero in `start_new_session` and `load_session_into`. Render history and streaming content only when their index is at or after the boundary, and include the boundary in `ChatKey`.

- [ ] **Step 4: Run focused tests to verify they pass**

Run: `cargo test app::actions::tests::clear_and_new_preserve_history -- --exact`

Expected: PASS.

- [ ] **Step 5: Run the required project checks**

Run: `cargo check --tests` and `cargo test`

Expected: both commands pass.

- [ ] **Step 6: Commit the implementation**

```bash
git add src/app/state.rs src/app/actions.rs src/main.rs src/ui/mod.rs src/ui/tests.rs docs/superpowers/specs/2026-08-12-tool-result-assistant-spacing-design.md docs/superpowers/plans/2026-08-12-tool-result-assistant-spacing.md
git commit -m "fix(ui): preserve transcript across clear and new"
```
