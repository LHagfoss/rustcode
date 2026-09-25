# Harness Checker Reliability Implementation Plan
> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prevent sandbox or checker-infrastructure failures from consuming the unchanged compiler-diagnostic budget while preserving real source-diagnostic protection.

**Architecture:** Replace the compiler checker's stringly success/failure boundary with a typed outcome, run checks with an isolated writable scratch environment, and count only source diagnostics toward the unchanged-diagnostics guard. Keep sandbox enforcement intact and pass the active workspace explicitly to command execution instead of weakening policies.

**Tech Stack:** Rust, Tokio/process execution, existing network/tool execution pipeline, Cargo tests, recorded RustCode operational events.

**Spec:** `docs/superpowers/specs/2026-09-25-native-settings-and-harness-reliability-design.md`

## Global Constraints

- Work on a fresh `fix/...` branch in a second isolated worktree created from current `origin/main` after the UI PR is merged.
- Preserve the sandbox boundary; do not grant broad filesystem access or disable sandboxing to make checks pass.
- Only repeatable source/compiler diagnostics may update the diagnostic fingerprint or consecutive-unchanged counter.
- Setup, workspace, permission, temp-directory, executable-not-found, cancellation, and other infrastructure failures must be visible as unverified checks and must not masquerade as clean checks.
- Keep the existing budget protection for genuinely unchanged source errors.
- Pass workspace context explicitly through the established execution pipeline; do not infer it from process-global current directory.
- Add regression tests derived from session `01a0d840854b-7000-9b08-10aa-10aa1ee60034` before changing behavior.
- Run `cargo check --tests` and `cargo test` before the branch is declared complete.

## Review Focus

- Classification boundaries between source diagnostics and infrastructure errors.
- No false “passed” state when a checker could not run.
- Temporary paths are unique, writable, cleaned up, and scoped to the checker process.
- Workspace propagation reaches macOS shell sandbox construction on every relevant call path.
- Budget/recovery event semantics and telemetry remain understandable.

---

## Task 1: Introduce typed compiler-check outcomes and diagnostic accounting

**Files:**
- Modify: `src/network/compiler.rs`
- Modify: `src/network/turn/tools.rs`
- Test: `src/network/tests.rs`

- [ ] Add failing regression tests for three outcomes: successful check, source diagnostics, and infrastructure/unverified failure. Assert only source diagnostics append the compiler marker/fingerprint and increment the unchanged-diagnostics streak.
- [ ] Introduce `CompilerCheckOutcome::{Passed, SourceDiagnostics { output, fingerprint }, UnverifiedInfrastructure { reason }}` (names may follow local conventions while preserving these semantics).
- [ ] Centralize classification so process exit status and normalized output are interpreted once; recognize the session's `PermissionDenied` tempdir failure as infrastructure, not a source diagnostic.
- [ ] Update turn accounting to clear or preserve counters according to existing successful/source behavior while leaving the source-diagnostic streak untouched by unverified infrastructure outcomes.
- [ ] Emit an explicit operational event/message for unverified checks with a recovery-oriented reason; never emit the normal source-diagnostics marker for that outcome.
- [ ] Run focused compiler/turn tests and `cargo check --tests`.
- [ ] Self-review cancellation, missing executable, empty output, nonzero exit, and fingerprint compatibility; commit the task.

## Task 2: Give compiler checks an isolated writable scratch environment

**Files:**
- Modify: `src/network/compiler.rs`
- Modify if required by existing helpers: `src/tools/exec/sandbox.rs`
- Test: `src/network/tests.rs`

- [ ] Add a failing test using a checker command that writes through `TMPDIR`/platform temp variables and prove it succeeds without writing artifacts into the project workspace.
- [ ] Create a unique temporary scratch directory per compiler-check invocation using the repository's existing temp helper or a small RAII helper.
- [ ] Set only the child checker's relevant temp/cache environment (`TMPDIR`, `TMP`, `TEMP`, and tool cache/home variables required by the configured checker) to paths within that scratch directory.
- [ ] Ensure the configured workspace remains the process working directory so project-relative check commands keep working, while transient downloads/cache files stay outside the project.
- [ ] Ensure cleanup occurs on success, source failure, infrastructure failure, and cancellation; cleanup failure may be logged but must not rewrite the checker outcome.
- [ ] Add the recorded `bunx biome check .` permission failure shape as a classifier/integration regression without depending on network access.
- [ ] Run focused compiler/sandbox tests and `cargo check --tests`.
- [ ] Self-review symlink/path escape risks, environment leakage, cross-platform variable handling, and concurrent checks; commit the task.

## Task 3: Propagate the active workspace into shell sandbox execution

**Files:**
- Modify: `src/network/tool_exec.rs`
- Modify: `src/tools/exec.rs`
- Modify: `src/tools/exec/sandbox.rs`
- Test: `src/network/tests.rs`
- Test: existing tests colocated under `src/tools/exec.rs` or `src/tools/exec/sandbox.rs`

- [ ] Add a failing regression test reproducing `macOS shell sandbox needs an active workspace` even though the session/tool request has a valid workspace.
- [ ] Trace the canonical workspace value from session/tool context to the command executor and pass it explicitly through the narrowest existing request/config structure.
- [ ] Make macOS sandbox profile construction consume that explicit validated workspace path; keep the existing error when no workspace is actually available.
- [ ] Verify mutation-triggered compiler checks and direct `run_command` calls receive the same active workspace without process-global state.
- [ ] Run focused tool execution/sandbox/network tests and `cargo check --tests`.
- [ ] Self-review path canonicalization, missing/deleted workspace behavior, non-macOS behavior, and every constructor/call site; commit the task.

## Task 4: Replay the originating failure and complete verification

**Files:**
- Modify as required by verified defects only: `src/network/compiler.rs`
- Modify as required by verified defects only: `src/network/tool_exec.rs`
- Modify as required by verified defects only: `src/tools/exec.rs`
- Modify as required by verified defects only: `src/tools/exec/sandbox.rs`
- Modify as required by verified defects only: `src/network/turn/tools.rs`
- Test: `src/network/tests.rs`

- [ ] Build a deterministic replay fixture from the originating sequence: four mutations, checker tempdir `PermissionDenied`, and a command requiring the active workspace.
- [ ] Assert the replay produces unverified-infrastructure events, zero unchanged-source-diagnostic budget increments, no generated `.hm`/`bunx-*` artifacts in the workspace, and successful workspace propagation.
- [ ] Retain or add a control test where four identical real source diagnostics still trigger the existing budget guard.
- [ ] Compare the replay's rounds, tool calls, recovery/budget events, and terminal status with the pre-fix session evidence; record the comparison in the implementation report.
- [ ] Run `cargo fmt --check`, `cargo check --tests`, and `cargo test` and record exact results.
- [ ] Review the whole branch against issue #1412 and the spec, remove incidental changes, and commit bounded integration fixes.
