# Wakeup Turn Lock Release Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ensure a wakeup turn releases the `AppState` mutex before synchronous native-tool schema/filesystem work, then reaches provider startup or the existing bounded watchdog recovery path.

**Architecture:** Add a small snapshot/compute/commit boundary around native-tool schema selection. `PromptCache` snapshots session, MCP generation, user-message count, policy, and sticky names under a short lock; schema selection runs without the lock; the selected names are committed only when the snapshot still matches. Both context preparation and final provider request assembly use the same helper. A test-only schema gate pauses exactly between context diagnostics and provider startup so a concurrent `AppState` lock acquisition and provider request are observable.

**Tech Stack:** Rust, Tokio, `parking_lot`/async mutexes already used by the application, serde JSON, existing local HTTP streaming test utilities, Cargo tests.

**Spec:** `docs/superpowers/specs/2026-09-22-wakeup-turn-lock-release-design.md`

## Global Constraints

- Work only in the isolated worktree `/private/tmp/rustcode-issue-1279-final`; leave the primary checkout unchanged.
- Preserve session, MCP-generation, policy, user-message, queue, cancellation, provider, and existing watchdog behavior.
- Do not hold the `AppState` mutex while native schema computation, MCP collection, workspace source-file inspection, or an unbounded await runs.
- Commit sticky schema names only when the active session and schema-generation snapshot still match; stale computations must not overwrite newer cache state.
- The regression must exercise a real queued wakeup/orchestrator and prove both lock release and provider startup; a unit test of a helper alone is insufficient.
- Do not change the watchdog timeout or queue lease semantics.

## Review Focus

- A schema computation finishing after the active session changes must not update the new session's sticky selection; test in Task 2.
- A schema computation finishing after the MCP generation changes must not update the old selection; test in Task 2 using the existing generation hook or an equivalent generation mismatch.
- The wakeup path must release `AppState` while the schema gate is paused and then send the provider request; test in Task 1 and rerun in Task 3.
- The ordinary non-wakeup request path must continue to select tools and complete a streaming response; test in Task 3 alongside the focused wakeup test.
- Cancellation and queue cleanup must remain owned by the existing orchestrator/lease guards; run the existing queue/cancellation tests in Task 4.

---

### Task 1: Add the deterministic lock-release regression seam and red test

**Files:**
- Modify: `src/tools/schema.rs` — add a `#[cfg(test)]` gate that can pause a marked schema computation and expose entered/release state.
- Modify: `src/tools/mod.rs` — re-export the test-only gate type and installer to crate tests.
- Modify: `src/network/tests.rs` — add a real queued wakeup regression using the local streaming provider server and the schema gate.

**Interfaces:**
- Produces `NativeSchemaTestGate` with `is_entered()`, `release()`, and a drop guard that clears the process-global test hook.
- The schema seam pauses only when the request messages contain the test marker and only on the configured call number, so existing tests do not block.
- The regression starts an orchestrator with one `__task_wakeup__:` queue item, an API-native model profile, and a local streaming provider. It installs the gate for the third schema computation (the first two are in context preparation; the third is after `turn.context_diagnostics`).

- [ ] **Step 1: Add the test-only gate before changing production lock ownership.**

  Add a process-global `Arc` gate in `src/tools/schema.rs` under `#[cfg(test)]`. The gate state must use atomics plus a standard mutex/condition variable, not an async mutex, because the schema function is synchronous. `pause_on_call` counts only marked invocations. The gate sets `entered` before waiting, wakes the test through `is_entered()`, and `release()` wakes the blocked computation. The `Drop` implementation must release and remove only its own installed gate.

  Call the gate immediately before `tool_schema_phase(messages, workspace_root)` inside `native_tools_schema_for_context_with_sticky_at`. This keeps the pre-fix call under the existing `AppState` lock and makes the lock contention deterministic.

- [ ] **Step 2: Write the failing wakeup test.**

  Add a Tokio multi-thread test named `background_wakeup_releases_state_during_native_schema_selection_and_starts_provider_request`. Use a unique marker in the queued wakeup's user prompt, install `pause_on_call = 3`, configure a local API-native model profile, and enqueue exactly one wakeup item. Start the existing orchestrator/queue path rather than calling `stream_request` directly.

  Once `gate.is_entered()` becomes true, assert that a separate task can acquire `state.lock()` within 100 ms. Always call `gate.release()` after that assertion attempt. Then assert that the local provider observed an HTTP request and that the orchestrator finishes within the existing test timeout with the queue drained. The pre-fix assertion must fail by timing out on the `AppState` lock while the gate is held; it must not be satisfied by a watchdog-only recovery.

  Use the existing local HTTP test helpers and response format. The server must signal request receipt, return a short valid SSE response ending in `[DONE]`, and be shut down by the test after the orchestrator completes.

- [ ] **Step 3: Run only the new test and verify the expected red result.**

  Run:

  ```bash
  cargo test background_wakeup_releases_state_during_native_schema_selection_and_starts_provider_request -- --test-threads=1
  ```

  Expected: the test compiles and fails at the 100 ms lock-acquisition assertion because the current implementation computes the third schema selection while holding `AppState`. If it fails to compile or passes, correct the seam/test until the failure is specifically the lock-release acceptance assertion.

- [ ] **Step 4: Commit the red regression seam and test.**

  ```bash
  git add src/tools/schema.rs src/tools/mod.rs src/network/tests.rs
  git commit -m "test: reproduce wakeup schema lock stall"
  ```

### Task 2: Implement snapshot/compute/commit with stale-result protection

**Files:**
- Modify: `src/app/state/models.rs` — add the native schema snapshot type, selection revision, snapshot method, and guarded commit method on `PromptCache`.
- Modify: `src/network.rs` — add the shared async schema-selection helper that locks only for snapshot/commit.
- Modify: `src/app/state/models.rs` tests or the nearest existing state-model test module — add session and MCP-generation stale-commit tests.

**Interfaces:**
- Produces `NativeToolSchemaSnapshot` containing `generation`, `policy`, `session_id`, `user_message_count`, `selection_revision`, and cloned `sticky_names`.
- Produces `PromptCache::native_tool_schema_snapshot(&mut self, policy, messages, session_id) -> NativeToolSchemaSnapshot`.
- Produces `PromptCache::commit_native_tool_schema_selection(&mut self, snapshot, selected_names) -> bool`.
- Produces `network::prepare_native_tool_schemas(state, policy, messages, workspace_root) -> (Vec<Value>, McpSchemaSelectionStats)`.

- [ ] **Step 1: Write the stale-session and stale-generation tests.**

  Test that a snapshot from session `old-session` returns `false` when committed after a new snapshot for `new-session`, and that the newer selection remains intact. Test the same behavior after the global MCP generation changes using the repository's existing MCP generation increment/reset test hook; restore the generation state before returning so tests remain independent.

- [ ] **Step 2: Run the cache tests and verify they fail for the missing snapshot/commit API.**

  Run the exact new cache test filter with one test thread. Expected: a compile failure naming the not-yet-defined snapshot/commit methods, confirming the test is exercising the planned production API rather than passing against the old cache behavior.

- [ ] **Step 3: Add snapshot/commit state and methods.**

  Add `mcp_selection_revision: u64` to `PromptCache`, increment it with wrapping arithmetic on every snapshot, and preserve the current invalidation rules for generation, policy, session, and increasing user-message count. Snapshotting may count roles and clone names while holding the lock, but must not call schema selection, MCP collection, filesystem traversal, or an await.

  The commit method must return `false` unless all of these still match: current global MCP generation, prompt-cache generation, policy, session ID, user-message count, and selection revision. On a match, replace `mcp_selected_names` and return `true`; on mismatch, leave the cache untouched and return `false`.

- [ ] **Step 4: Add the shared async compute helper.**

  Implement `prepare_native_tool_schemas` in `src/network.rs` as:

  ```rust
  pub(crate) async fn prepare_native_tool_schemas(
      state: &Arc<Mutex<AppState>>,
      policy: ToolSchemaPolicy,
      messages: &[Value],
      workspace_root: Option<&Path>,
  ) -> (Vec<Value>, McpSchemaSelectionStats)
  ```

  Snapshot under `state.lock().await`, release the guard, call `native_tools_schema_for_context_with_sticky_at` with the snapshot's policy and sticky names, then reacquire the lock and conditionally commit the selected names. Return the computed result even when commit is rejected, because the current request still owns that already-computed provider payload.

- [ ] **Step 5: Run the cache tests and the existing schema tests.**

  Expected: session and generation stale commits pass; schema-selection behavior remains unchanged; the Task 1 wakeup test remains red because no call site uses the new helper yet.

- [ ] **Step 6: Commit the snapshot/compute/commit implementation.**

  ```bash
  git add src/app/state/models.rs src/network.rs
  git commit -m "fix: compute native schemas outside app state lock"
  ```

### Task 3: Route context preparation and provider startup through the helper

**Files:**
- Modify: `src/network.rs` — replace both native schema projections in `prepare_turn_request_with_checkpoint_and_prefix_cache` with the shared helper.
- Modify: `src/network/stream_request.rs` — release `AppState` before final native schema computation and move textual MCP surface calculation outside the lock as well.
- Modify: `src/network/tests.rs` — add/assert the ordinary streamed provider completion if the new regression helper needs a separate non-wakeup case.

**Interfaces:**
- Consumes `prepare_native_tool_schemas` from Task 2 at every existing native-schema call site.
- Does not change `ToolSchemaPolicy`, message trimming, request composition, provider selection, queue lease, cancellation, or watchdog control flow.

- [ ] **Step 1: Refactor the first context-preparation projection.**

  Read `active_context_budget()` under a short lock, then call `prepare_native_tool_schemas` after the lock is dropped. Preserve the current message order and the returned `native_tool_schemas` value.

- [ ] **Step 2: Refactor the post-trim projection.**

  Replace the second direct `PromptCache::native_tool_schemas` call with `prepare_native_tool_schemas`, preserving the current policy, trimmed messages, workspace root, and preflight-budget diagnostics ordering.

- [ ] **Step 3: Refactor final provider request composition.**

  In `stream_request`, snapshot only cheap protocol, agent-mode, and workspace values under `AppState`; compute `textual_tool_surface` after releasing the lock; and call `prepare_native_tool_schemas` for the API-native path. Keep the current MCP selection event and tool-surface values intact.

- [ ] **Step 4: Run the focused regression and confirm green.**

  ```bash
  cargo test background_wakeup_releases_state_during_native_schema_selection_and_starts_provider_request -- --test-threads=1
  cargo test background_wakeup_releases_state_during_native_schema_selection_and_starts_provider_request -- --test-threads=1
  cargo test background_wakeup_releases_state_during_native_schema_selection_and_starts_provider_request -- --test-threads=1
  ```

  Expected: every run passes the lock-acquisition assertion, observes `provider.request_start`/the provider request, and drains the wakeup turn without watchdog recovery.

- [ ] **Step 5: Run adjacent behavior tests.**

  Run the existing background-wakeup, queue/orchestrator, cancellation, native-tool-schema, and stall-watchdog filters. Expected: all pass with no new watchdog recovery events in the successful wakeup regression.

- [ ] **Step 6: Commit the call-site refactor and green regression.**

  ```bash
  git add src/network.rs src/network/stream_request.rs src/network/tests.rs
  git commit -m "fix: release state lock before wakeup schema selection"
  ```

### Task 4: Full verification and delivery gate

**Files:**
- Inspect only: all changed files and `git diff`.
- Create only after acceptance is proven: the GitHub PR from `fix/issue-1279-final` to `main`.

- [ ] **Step 1: Format and diff-check.**

  ```bash
  cargo fmt --all -- --check
  git diff --check
  ```

- [ ] **Step 2: Compile all tests.**

  ```bash
  cargo check --tests
  ```

- [ ] **Step 3: Run the full suite.**

  ```bash
  cargo test
  ```

- [ ] **Step 4: Repeat the focused regression after the full suite.**

  Run the deterministic wakeup test at least five times with one test thread and retain the outputs. Expected: five passes, each proving lock acquisition and provider request observation.

- [ ] **Step 5: Review the final diff against the spec.**

  Confirm the primary checkout is still on its original clean `main`, the worktree contains only the scoped implementation/spec/plan commits, no watchdog or queue policy changed, and the regression does not pass through recovery instead of provider startup.

- [ ] **Step 6: Push and open the PR only after all acceptance checks pass.**

  ```bash
  git push -u origin fix/issue-1279-final
  gh pr create --repo LHagfoss/rustcode --base main --head fix/issue-1279-final \
    --title "Fix wakeup turn stall during native schema preparation" \
    --body "Closes #1279\n\nRoot cause: synchronous native schema/MCP/filesystem preparation ran while holding AppState across the post-diagnostics provider startup boundary. The fix snapshots cache metadata, computes schemas without the mutex, and conditionally commits sticky selections. The regression pauses the exact wakeup schema step, proves another task acquires AppState, then observes provider startup and normal queue drain."
  ```

  Do not merge the PR. If any acceptance check fails or the test does not prove provider startup, do not push or open a PR; report the concrete blocker instead.
