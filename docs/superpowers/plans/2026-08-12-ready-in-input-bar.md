# Ready in Input Bar Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Put the Ready animation and compact live status on the input border's bottom row.

**Architecture:** Reuse the existing activity classification/animation and status formatter. Add independent left/right bottom titles to the ratatui Block and remove the now-redundant footer layout row.

**Tech Stack:** Rust, ratatui, Cargo unit tests.

## Global Constraints

- Preserve activity details, interrupt hints, context fallback, quota, TPS, and keyboard behavior.
- Use one-column end padding and two-space status separators.

---

### Task 1: Regression test

**Files:**
- Modify: `src/ui/tests.rs`

- [x] Update the input status expectation to two-space separators.
- [x] Assert the idle activity label is `Ready`.
- [x] Run the focused test and observe the expected missing-helper/old-spacing failure.

### Task 2: Implement the combined input bottom row

**Files:**
- Modify: `src/ui/mod.rs`

- [x] Extract the activity line from the footer renderer.
- [x] Add left/right bottom titles to the input Block with explicit padding.
- [x] Remove the separate footer row and tighten interrupt/status spacing.
- [x] Run the focused UI suite; 30 tests pass.

### Task 3: Full verification and integration

**Files:**
- No additional files.

- [ ] Run `cargo check --tests` and `cargo test`.
- [ ] Commit, push, create/merge the PR into `main`, and pull `main`.

