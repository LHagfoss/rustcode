# Activity Random Status Words Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rename idle to `Idle` and replace active `Working · Responding` text with the historical rotating RustCode status phrases.

**Architecture:** Keep activity classification in `src/app/activity.rs`, changing only the idle label. Add a deterministic phrase selector in `src/ui/mod.rs` keyed by elapsed generation seconds; the input-bar activity renderer will use it only for streaming. Preserve the existing animation, elapsed time, tool/action details, and interrupt hint.

**Tech Stack:** Rust, ratatui, Cargo unit tests.

## Global Constraints

- Use the historical ten-phrase list exactly.
- Change phrases every three elapsed seconds.
- Show `Idle` for `AppStatus::Idle`.
- Preserve queued, running-tool, action-required, question/confirmation, elapsed-time, and `esc interrupt` behavior.
- Add one trailing space after the left activity item inside the input border.

---

### Task 1: Add failing regression tests

**Files:**
- Modify: `src/ui/tests.rs`
- Modify: `src/app/activity.rs` tests

**Interfaces:**
- Produces: tests for the deterministic phrase selector and idle label.

- [ ] **Step 1: Add tests**

Add assertions equivalent to:

```rust
assert_eq!(streaming_status_word(0), "Thinking...");
assert_eq!(streaming_status_word(2), "Thinking...");
assert_eq!(streaming_status_word(3), "Analyzing code...");
assert_eq!(streaming_status_word(27), "Querying knowledge base...");
assert_eq!(classify_activity(&AppStatus::Idle, &[]).label, "Idle");
```

Add an activity-line regression assertion that its final span/text has a trailing space after the status content.

- [ ] **Step 2: Run focused tests and verify red**

Run:

```bash
cargo test --bin rustcode ui::tests
cargo test --bin rustcode activity
```

Expected: failure because the selector is not defined, idle still reports `Ready`, and the activity line has no trailing padding.

### Task 2: Implement activity labels and padding

**Files:**
- Modify: `src/app/activity.rs:69-74`
- Modify: `src/ui/mod.rs:805-895`

**Interfaces:**
- Consumes: `AppState.generation_start_time`, `ActivityKind::Working`, and existing activity classification.
- Produces: `streaming_status_word(elapsed_secs: u64) -> &'static str` and the updated input-bar activity line.

- [ ] **Step 1: Add the historical phrase selector**

Implement:

```rust
const STREAMING_STATUS_WORDS: &[&str] = &[
    "Thinking...",
    "Analyzing code...",
    "Consulting the oracle...",
    "Brewing coffee...",
    "Refactoring reality...",
    "Checking documentation...",
    "Optimizing loops...",
    "Debugging the universe...",
    "Synthesizing solutions...",
    "Querying knowledge base...",
];

fn streaming_status_word(elapsed_secs: u64) -> &'static str {
    STREAMING_STATUS_WORDS[((elapsed_secs / 3) as usize) % STREAMING_STATUS_WORDS.len()]
}
```

- [ ] **Step 2: Rename the idle activity label**

Change the `AppStatus::Idle` snapshot label from `Ready` to `Idle` and update the matching color/title tests.

- [ ] **Step 3: Use the phrase for streaming**

In `activity_status_label`, return `streaming_status_word` for `ActivityKind::Working`, using `generation_start_time.elapsed().as_secs()` or zero if unavailable. Leave all non-streaming activity labels unchanged.

- [ ] **Step 4: Add right-side padding**

Append a raw single-space span after the activity/interrupt spans in `activity_status_line`, preserving all existing left padding and hint content.

- [ ] **Step 5: Run the focused tests**

Run:

```bash
cargo test --bin rustcode ui::tests
cargo test --bin rustcode activity
```

Expected: all focused activity/UI tests pass.

### Task 3: Full verification and integration

**Files:**
- No additional files.

- [ ] **Step 1: Run required gates**

Run:

```bash
cargo check --tests
cargo test
```

Expected: both exit successfully with zero failures.

- [ ] **Step 2: Inspect and commit**

Run:

```bash
git diff --check
git status --short --branch
```

Stage only explicit implementation/test/spec/plan paths and commit the fix.

- [ ] **Step 3: Publish and merge**

Push the fix branch, create a PR into `main` with `gh pr create`, merge it with `gh pr merge`, then checkout `main` and `git pull --ff-only`.

- [ ] **Step 4: Post-merge verification**

Run `cargo test` on synchronized `main` and confirm the worktree is clean.
