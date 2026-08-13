# Markdown Hanging Indents Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Preserve list and blockquote visual context when Markdown wraps to a narrow terminal width.

**Architecture:** Extend `src/ui/markdown.rs`'s existing span word wrapper with an optional continuation prefix. The Markdown event loop records the active list item's continuation indentation and combines it with the current quote depth before flushing text.

**Tech Stack:** Rust, pulldown-cmark, ratatui, existing Markdown unit tests.

## Global Constraints

- Preserve existing normal-paragraph and wide-content output.
- Reuse the existing word wrapper; do not replace the Markdown parser.
- Keep quote gutters on every wrapped quote line and align list continuation text under its marker.
- Verify with `cargo check --tests` and `cargo test`.

---

### Task 1: Add failing hanging-indent renderer tests

**Files:**
- Modify: `src/ui/markdown.rs`

**Interfaces:**
- Consumes: `render_markdown` with narrow widths.
- Produces: failing tests for wrapped unordered lists and blockquotes.

- [ ] **Step 1: Write a list test**

Render one long unordered item at width 24. Assert the first line begins `• `,
the continuation line begins two spaces, and it does not begin with a second
bullet.

- [ ] **Step 2: Verify it fails**

Run: `cargo test wrapped_list_items_keep_a_hanging_indent`

Expected: FAIL because current continuation lines restart at column zero.

- [ ] **Step 3: Write a blockquote test**

Render a long quote at width 24. Assert every nonblank line begins `│ `.

- [ ] **Step 4: Verify it fails**

Run: `cargo test wrapped_blockquotes_keep_their_gutter`

Expected: FAIL because the current quote gutter appears only on the first line.

### Task 2: Seed wrapped continuation prefixes

**Files:**
- Modify: `src/ui/markdown.rs`
- Test: `src/ui/markdown.rs`

**Interfaces:**
- Consumes: paragraph spans, quote depth, and active list marker width.
- Produces: an internal wrapper accepting an optional continuation `Span`.

- [ ] **Step 1: Add a continuation-aware word wrapper**

Create a helper that starts each output line after the first with a supplied
prefix span, counting its display width against the terminal width.

- [ ] **Step 2: Track the active list continuation**

At `Tag::Item`, record the quote gutter plus spaces matching the list marker.
At item end, flush the item and clear this state.

- [ ] **Step 3: Use quote gutter when no list item is active**

When flushing a standalone blockquote, pass its repeated `│ ` gutter as the
continuation prefix.

- [ ] **Step 4: Verify focused tests pass**

Run: `cargo test wrapped_`

Expected: both new tests pass.

### Task 3: Verify and commit

**Files:**
- Modify: `docs/superpowers/specs/2026-08-13-markdown-hanging-indents-design.md`
- Modify: `docs/superpowers/plans/2026-08-13-markdown-hanging-indents.md`

- [ ] **Step 1: Run verification**

Run: `cargo check --tests && cargo test`

Expected: both commands exit 0.

- [ ] **Step 2: Commit the scoped change**

Run: `git add src/ui/markdown.rs && git add -f docs/superpowers/specs/2026-08-13-markdown-hanging-indents-design.md docs/superpowers/plans/2026-08-13-markdown-hanging-indents.md && git commit -m "feat(ui): align wrapped markdown blocks"`

Expected: one focused commit.
