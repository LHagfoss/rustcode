# Edit Diff Rendering After Session Reload Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Render persisted Edit results through the normal diff renderer even when the original assistant tool-call message is unavailable.

**Architecture:** Keep the existing ephemeral diff fast path. In the transcript tool-result branch, resolve the tool name from persisted `ToolResultRecord` metadata or the content prefix, then reuse `cached_tool_result` so embedded diffs are highlighted consistently.

**Tech Stack:** Rust, Ratatui, existing UI unit tests.

## Global Constraints

- Preserve unrelated `Cargo.lock` changes.
- Do not persist the full ephemeral `ChatMessage.diff` field.
- Keep live-session rendering behavior unchanged.

### Task 1: Add the regression test

**Files:**
- Modify: `src/ui/mod.rs` test module

- [ ] **Step 1: Add a test fixture for a persisted Edit result without a preceding parseable call.** Assert that the rendered transcript contains the diff body and not only the generic result fallback.

- [ ] **Step 2: Run the focused UI test and confirm it fails because the renderer currently omits the result when the preceding call cannot be parsed.**

### Task 2: Resolve persisted tool metadata during rendering

**Files:**
- Modify: `src/ui/mod.rs` transcript rendering branch

- [ ] **Step 1: Resolve the tool name from `prev_tool_info`, then `msg.tool_result.tool_name`, then the `name: result` content prefix.**

- [ ] **Step 2: Use the resolved tool name with `cached_tool_result` when no ephemeral file preview or diff is available.**

- [ ] **Step 3: Run the focused test and full `cargo test`.**

### Task 3: Verify and publish

**Files:**
- Modify only the files from Tasks 1–2.

- [ ] **Step 1: Run `cargo check`, `cargo test`, `cargo clippy --all-targets -- -D warnings`, and `git diff --check`.**

- [ ] **Step 2: Review the diff and commit only the feature files.**

- [ ] **Step 3: Push the feature branch, open and merge a PR, then return to `main` and pull.**
