# Status in Input Bar Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move the live status cluster from the footer into the input box bottom border.

**Architecture:** Reuse the existing context/TPS formatting and centralize state-to-text assembly in `format_input_status_text`. Keep `render_footer` responsible only for activity status and keep keyboard behavior unchanged.

**Tech Stack:** Rust, ratatui, Cargo unit tests.

## Global Constraints

- Render `Auto-Confirm`, `Context`, `Tps`, optional quota, and `Ctrl+P commands` in the input bottom border.
- Keep the activity trail and interrupt hint in the footer.
- Preserve context-token fallback, cached-token, quota, and streaming TPS behavior.

---

### Task 1: Regression coverage

**Files:**
- Modify: `src/ui/tests.rs`

- [x] Write the failing `input_bar_contains_live_status_and_command_hint` test.
- [x] Run `cargo test --bin rustcode ui::tests`; it failed because `format_input_status_text` was missing.

### Task 2: Input-bar status rendering

**Files:**
- Modify: `src/ui/mod.rs`

- [x] Add `context_usage` and `format_input_status_text`.
- [x] Use the formatted status in the input box bottom border.
- [x] Remove the duplicate right-side footer status cluster.
- [x] Run `cargo test --bin rustcode ui::tests`; 31 tests pass.

### Task 3: Verification and integration

**Files:**
- No additional files.

- [ ] Run `cargo check --tests` and `cargo test`.
- [ ] Commit, push, create/merge the PR into `main`, then pull `main`.

