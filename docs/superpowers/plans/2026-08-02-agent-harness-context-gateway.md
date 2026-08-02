# Agent Harness Context Gateway Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ensure every tool result is bounded before entering model context while preserving actionable recovery information.

**Architecture:** Keep the existing tool implementations, but route their returned strings through the shared output boundary in `src/network/output.rs`. Make `view_file` enforce its line window at the source, and keep artifact storage separate from transcript content. Add regression tests at the pure helper and orchestration boundaries.

**Tech Stack:** Rust 2024, Tokio, serde_json, existing Rustcode tool registry and test harness.

## Global Constraints

- Do not modify Discord RPC behavior.
- Preserve `Cargo.lock` and `images/rustcode-logo.png` as user-owned changes.
- Do not run whole-repository `cargo fmt`; use `cargo fmt --check` or format only explicitly touched hunks.
- Do not remove or weaken existing turn, cancellation, edit-idempotency, or loop-safety limits.
- Do not put full artifacts into chat history or provider requests.
- Each task must be independently tested and committed on this feature branch.

---

### Task 1: Establish a bounded-output test contract

**Files:**
- Modify: `src/network/output.rs`
- Test: `src/network/output.rs` test module

**Interfaces:**
- Preserve `truncate_tool_output(name: &str, result: String) -> String`.
- Add only small pure helpers if needed so byte/line limits can be tested without network calls.

- [ ] **Step 1: Add failing tests** for a result over the byte limit and a result over the line limit. Assert that the returned string contains a truncation marker, keeps the first meaningful line, keeps the last meaningful line, and is smaller than the input.
- [ ] **Step 2: Add a failure-output test** proving the tail containing a compiler error survives truncation.
- [ ] **Step 3: Add an artifact test** proving the saved artifact is byte-identical and that only its path—not its full contents—is present in the returned bounded string.
- [ ] **Step 4: Run the focused tests** with `cargo test network::output::tests` and confirm the new tests fail for the missing behavior.
- [ ] **Step 5: Implement the smallest helper changes** needed to satisfy the tests without changing existing limits or public tool behavior.
- [ ] **Step 6: Run `cargo test network::output::tests`** and confirm all focused tests pass.
- [ ] **Step 7: Commit** with `git add src/network/output.rs && git commit -m "test: define bounded tool output contract"`.

### Task 2: Enforce bounded file reads and honest ranges

**Files:**
- Modify: `src/tools/filesystem.rs` in `view_file_tool`
- Test: `src/tools/filesystem.rs` test module

**Interfaces:**
- Preserve `view_file_tool(args: &serde_json::Value) -> Result<String, String>`.
- Keep `start_line`, `end_line`, and `content_offset` compatibility.

- [ ] **Step 1: Add a failing explicit-range test** requesting more than the maximum window and assert the result does not contain lines beyond the cap.
- [ ] **Step 2: Add a metadata test** asserting the header reports the actual returned range and the truncation marker gives the exact next `start_line`.
- [ ] **Step 3: Add a byte-offset test** proving line numbering and follow-up reads remain coherent when `content_offset` is supplied.
- [ ] **Step 4: Run the focused filesystem tests** with `cargo test tools::filesystem::tests` and confirm the new tests fail or expose the current behavior.
- [ ] **Step 5: Implement the hard ceiling** for both omitted and explicit `end_line` values. Do not silently describe a capped result as the complete requested range.
- [ ] **Step 6: Run the focused filesystem tests** and confirm all pass.
- [ ] **Step 7: Commit** with `git add src/tools/filesystem.rs && git commit -m "fix(tools): bound explicit file reads"`.

### Task 3: Verify the orchestration boundary

**Files:**
- Modify: `src/network.rs` only where tool results are appended to history or returned to the model
- Test: `src/network.rs` test module

**Interfaces:**
- Preserve `ToolResult` metadata and existing `full_output_artifact` behavior.
- Preserve native and text-protocol tool execution paths.

- [ ] **Step 1: Add a regression test** around the result-to-history path using an oversized synthetic tool result. Assert that the stored message contains the bounded result and not the original full payload.
- [ ] **Step 2: Add a duplicate-boundary test** proving the same result is not truncated twice into nested markers or expanded again through a second path.
- [ ] **Step 3: Run the focused network tests** and confirm the regression tests fail or identify the current path precisely.
- [ ] **Step 4: Route only the unbounded path through the shared gateway**, avoiding duplicate truncation where metadata already contains a bounded result.
- [ ] **Step 5: Run focused network tests and the full `cargo test` suite**.
- [ ] **Step 6: Run `cargo clippy --all-targets --all-features -- -D warnings` and `git diff --check`**.
- [ ] **Step 7: Commit** with `git add src/network.rs && git commit -m "fix(harness): bound tool results before context insertion"`.

### Task 4: Review and integrate the feature

**Files:**
- Review: all commits on this branch against `main`

- [ ] **Step 1: Inspect** `git diff --stat main...HEAD`, `git diff --check`, and the changed-file list. Confirm no Discord or unrelated formatting changes exist.
- [ ] **Step 2: Run** `cargo test`, `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo fmt --check`; record the known baseline formatting failures without applying whole-repository formatting.
- [ ] **Step 3: Request a focused code review** for context bounds, recovery semantics, and accidental transcript growth.
- [ ] **Step 4: Resolve all critical and important findings** with tests before integration.
- [ ] **Step 5: Push this branch and open one PR against `main`**.
- [ ] **Step 6: Merge the PR only after verification**, then checkout `main` and pull `origin/main` before starting the next feature.
