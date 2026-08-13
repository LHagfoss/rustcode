# Tool Transcript Hierarchy Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Render visible completed tool calls with an action header, explicit result status, and indented output.

**Architecture:** Keep `src/ui/tool_result.rs` responsible for its tool-specific body lines. Add committed-history composition in `src/ui/mod.rs` that resolves an associated tool call, renders header and status lines, and indents the existing body. The history renderer remains the single point that commits this presentation to native scrollback.

**Tech Stack:** Rust, ratatui, existing RustCode transcript renderer and unit tests.

## Global Constraints

- Preserve native terminal scrollback and the current keyboard-only interaction model.
- Keep control-plane and high-verbosity tool results hidden.
- Use `ToolResultRecord` metadata when available and avoid changes to tool execution or persistence.
- Verify with `cargo check --tests` and `cargo test`.

---

### Task 1: Define transcript behavior with regression tests

**Files:**
- Modify: `src/ui/tests.rs`

**Interfaces:**
- Consumes: `render_committed_history_block`, `ChatMessage`, `ToolCallRef`, and `ToolResultRecord`.
- Produces: failing tests that require visible tool header, explicit status, and indented output.

- [ ] **Step 1: Write the failing success-result test**

Create an assistant message with a `run_command` tool call named `call-1` and `{"command":"cargo test"}` arguments. Follow it with a matching successful tool result with `exit_code: Some(0)` and output `504 passed`. Assert its committed block contains `• Bash · cargo test`, `└ ✓ exit 0`, and an output line beginning `    │ 504 passed`.

- [ ] **Step 2: Run the focused test to verify it fails**

Run: `cargo test ui::tests::committed_tool_result_shows_action_status_and_indented_output`

Expected: FAIL because the current committed-history renderer emits only the legacy result body.

- [ ] **Step 3: Write the failing failure-result test**

Create matching `run_command` call/result messages with `success: false`, `exit_code: Some(1)`, and command stderr. Assert the block contains `• Bash · cargo test`, `└ ✗ exit 1`, and indented output.

- [ ] **Step 4: Run the focused test to verify it fails**

Run: `cargo test ui::tests::committed_tool_result_shows_failure_status`

Expected: FAIL because no explicit header or status line currently exists.

### Task 2: Compose tool transcript blocks

**Files:**
- Modify: `src/ui/mod.rs`
- Test: `src/ui/tests.rs`

**Interfaces:**
- Consumes: `format_pi_tool_action`, history tool call records, `ToolResultRecord`, and `cached_tool_result` body lines.
- Produces: a helper used by `render_committed_history_block` that returns `Vec<Line<'static>>` for one visible tool result.

- [ ] **Step 1: Resolve the related action**

Search preceding assistant messages for the matching `tool_call_id`; parse its JSON arguments and pass them to `format_pi_tool_action`. If no structured link exists, use the nearest same-name tool call and omit unknown targets.

- [ ] **Step 2: Render header and status**

Render `• <Action> · <target>` with existing transcript colors. Render `  └ ✓ exit <code>` or `  └ ✗ exit <code>` from result metadata; use `completed`/`failed` when an exit code is not available. Parse an old command result's `exit code: N` only as a fallback.

- [ ] **Step 3: Indent the existing result body**

Prepend two spaces to each nonblank cached result body line, preserving the existing spans and their colors. Return no block in high verbosity or for the existing control-plane result types.

- [ ] **Step 4: Replace the direct committed-history body call**

Call the composition helper from the tool-result branch of `render_committed_history_block`, then retain its existing separating blank line only for visible blocks.

- [ ] **Step 5: Run focused tests to verify they pass**

Run: `cargo test ui::tests::committed_tool_result_`

Expected: PASS for the new success and failure transcript tests.

### Task 3: Verify and commit the focused change

**Files:**
- Modify: `docs/superpowers/specs/2026-08-13-tool-transcript-hierarchy-design.md`
- Modify: `docs/superpowers/plans/2026-08-13-tool-transcript-hierarchy.md`

- [ ] **Step 1: Run project verification**

Run: `cargo check --tests && cargo test`

Expected: both commands exit 0.

- [ ] **Step 2: Inspect the scoped diff**

Run: `git diff --check && git diff -- src/ui/mod.rs src/ui/tests.rs docs/superpowers`

Expected: only the planned transcript composition, tests, and documentation are present.

- [ ] **Step 3: Commit the focused change**

Run: `git add src/ui/mod.rs src/ui/tests.rs && git add -f docs/superpowers/specs/2026-08-13-tool-transcript-hierarchy-design.md docs/superpowers/plans/2026-08-13-tool-transcript-hierarchy.md && git commit -m "feat: clarify tool transcript results"`

Expected: one commit containing only this feature.
