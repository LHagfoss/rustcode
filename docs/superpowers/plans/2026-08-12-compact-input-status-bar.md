# Compact Input Status Bar Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the terminal input/status area compact while keeping command access, auto-confirm state, context usage, and token rate visible.

**Architecture:** Keep rendering in `src/ui/mod.rs`; extract the status-bar data assembly into small pure helpers so the display policy can be tested without terminal interaction. The vertical layout will use the existing input and footer areas directly adjacent to one another.

**Tech Stack:** Rust, ratatui, Cargo unit tests.

## Global Constraints

- Preserve all existing keyboard behavior; change displayed hints and spacing only.
- Show `Context: <tokens> (<percent>%)` and `Tps: <rate>` even before the first message.
- Preserve cached-token and quota details when available.
- Keep the input hint limited to `Ctrl+P commands`.
- Stage only explicit feature/spec paths; do not use broad Git staging commands.

---

### Task 1: Add focused status formatting tests

**Files:**
- Modify: `src/ui/mod.rs` (test module or test-facing pure helpers near the footer renderer)
- Test: `src/ui/mod.rs` unit tests via the existing `#[cfg(test)]` module, if present

**Interfaces:**
- Consumes: the existing `AppState`, token usage, context-window, quota, and stream tracker state.
- Produces: failing regression tests for the always-visible context/TPS status and compact hint policy.

- [ ] **Step 1: Write the failing tests**

Add tests that assert:

```rust
assert_eq!(format_context_info(0, 0, None), "Context: 0 (0%)");
assert_eq!(format_tps_info(0.0), "Tps: 0.0");
assert_eq!(input_footer_hint_text(), "Ctrl+P commands");
```

Also add a layout-policy assertion that the render constraints contain no spacer row between the input height and the one-row footer.

- [ ] **Step 2: Run the focused tests to verify they fail**

Run:

```bash
cargo test ui::tests --lib
```

Expected: compilation failure because the new pure helpers/layout policy do not exist yet.

### Task 2: Implement compact input and status-bar rendering

**Files:**
- Modify: `src/ui/mod.rs:722-997,1095-1112,2490-2512`

**Interfaces:**
- Consumes: existing `AppState` values and `render_input`/`render_footer` call flow.
- Produces: compact input hint, always-visible status data, and adjacent input/footer layout.

- [ ] **Step 1: Add minimal pure formatting helpers**

Implement helpers for context and TPS text, preserving the current token formatting (`N`, `N.K`) and percentage calculation. Make the idle TPS helper return `Tps: 0.0`.

- [ ] **Step 2: Run the focused tests**

Run:

```bash
cargo test ui::tests --lib
```

Expected: the helper tests pass; any remaining failure must identify a layout or rendering assertion.

- [ ] **Step 3: Replace the input-box hint line**

Change `footer_hints` to render only:

```text
Ctrl+P commands
```

Keep the existing styling and right alignment.

- [ ] **Step 4: Move status content into the footer bar**

Remove the empty-history branch that displays `tab agents`. Build the right-side spans consistently from current/last token usage or the existing character fallback, then append `Context`, `Tps`, quota when available, and `ctrl+p commands`. Render `Auto-Confirm` as part of this same bar rather than in a separate centered column.

- [ ] **Step 5: Remove the spacer row**

Delete the extra `Constraint::Length(1)` between `input_height` and the footer constraint, and update footer indexing only as needed. The input area and one-row footer must be adjacent.

- [ ] **Step 6: Run focused tests and inspect the diff**

Run:

```bash
cargo test ui::tests --lib
git diff --check
git diff -- src/ui/mod.rs
```

Expected: focused tests pass, no whitespace errors, and the diff contains only the requested UI changes plus tests.

### Task 3: Verify the full Rust change

**Files:**
- No additional files.

- [ ] **Step 1: Run the required compile/test gates**

Run:

```bash
cargo check --tests
cargo test
```

Expected: both commands exit successfully with no test failures.

- [ ] **Step 2: Review repository state**

Run:

```bash
git status --short --branch
git diff --stat
```

Confirm only the implementation and focused tests remain unstaged for the feature commit; the already-committed design and plan docs are expected history.

