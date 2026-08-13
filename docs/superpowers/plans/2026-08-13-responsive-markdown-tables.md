# Responsive Markdown Tables Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Render cramped multi-column Markdown tables as readable key/value records.

**Architecture:** Keep parsing and normal-width grid rendering in `src/ui/markdown.rs`. Add a width-driven fallback inside its existing table flush path that turns each body row into header-labelled fields and reuses the paragraph word wrapper for values.

**Tech Stack:** Rust, pulldown-cmark, ratatui, existing Markdown renderer tests.

## Global Constraints

- Preserve existing wide-table grid output.
- Change only multi-column tables that cannot provide a readable width per column.
- Keep values word-wrapped and preserve the current Markdown cache interface.
- Verify with `cargo check --tests` and `cargo test`.

---

### Task 1: Add a failing narrow-table test

**Files:**
- Modify: `src/ui/markdown.rs`

**Interfaces:**
- Consumes: `render_markdown`.
- Produces: a test showing a two-column table rendered at 28 columns as labelled fields rather than box-drawing rows.

- [ ] **Step 1: Write the failing test**

Render a table with `Field` and `Value` headers at width 28. Assert a line
contains `Name: rustcode`, a long value wraps, and no line contains `┌`.

- [ ] **Step 2: Verify it fails**

Run: `cargo test narrow_tables_render_as_key_value_records`

Expected: FAIL because the current renderer always emits a bordered grid.

### Task 2: Add responsive record fallback

**Files:**
- Modify: `src/ui/markdown.rs`
- Test: `src/ui/markdown.rs`

**Interfaces:**
- Consumes: parsed `(cells, is_header)` table rows, available width, and `push_wrapped`.
- Produces: a narrow-table record renderer called by the existing table flush closure.

- [ ] **Step 1: Detect a cramped multi-column grid**

Use `cols * 14 + 4 > width` as the readable-grid threshold. Require a header
and at least two columns before selecting the record layout.

- [ ] **Step 2: Render body rows as fields**

For every body cell, create muted bold `<header>:` text and normal value text,
then call `push_wrapped` at the available width. Add a blank row between body
records only when another body record follows.

- [ ] **Step 3: Verify the focused test passes**

Run: `cargo test narrow_tables_render_as_key_value_records`

Expected: PASS while the existing normal-width table test remains green.

### Task 3: Verify and commit

**Files:**
- Modify: `docs/superpowers/specs/2026-08-13-responsive-markdown-tables-design.md`
- Modify: `docs/superpowers/plans/2026-08-13-responsive-markdown-tables.md`

- [ ] **Step 1: Run verification**

Run: `cargo check --tests && cargo test`

Expected: both commands exit 0.

- [ ] **Step 2: Commit the scoped change**

Run: `git add src/ui/markdown.rs && git add -f docs/superpowers/specs/2026-08-13-responsive-markdown-tables-design.md docs/superpowers/plans/2026-08-13-responsive-markdown-tables.md && git commit -m "feat(ui): adapt narrow markdown tables"`

Expected: one focused commit.
