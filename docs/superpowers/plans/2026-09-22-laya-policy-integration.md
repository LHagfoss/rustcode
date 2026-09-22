# Laya Policy Integration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an opt-in, self-hosted Laya-MLX advisory layer that can relax only narrowly eligible shell confirmations and read-only repetition recovery while preserving RustCode's local safety authority.

**Architecture:** Rust owns configuration, deterministic command classification, policy composition, caching, repetition accounting, and fallback behavior. A persistent Python JSONL sidecar owns Laya-MLX model loading and inference. `off` bypasses the sidecar entirely, `shadow` records advisory results without changing behavior, and `relaxed` applies only high-confidence read-only decisions that pass Rust-side eligibility checks.

**Tech Stack:** Rust, Tokio, Clap, Serde/TOML, existing RustCode authorization and loop-detection modules, Python 3.11+ with pinned `laya-mlx` and local MLX checkpoint on Apple Silicon.

**Spec:** `docs/superpowers/specs/2026-09-22-laya-advisory-policy-design.md`

## Global Constraints

- The feature is `off` by default and disabled mode must preserve current RustCode behavior exactly.
- No cloud API, API key, telemetry service, or runtime model download may be introduced.
- The first implementation uses a persistent Python `laya-mlx` sidecar; it does not bind MLX directly from Rust.
- Laya may never downgrade local `Deny`, Plan-mode denial, explicit destructive confirmation, or known mutation/process/network hazards.
- Laya failures, uncertainty, low confidence, malformed responses, timeouts, and unavailable dependencies fall back to the existing local policy.
- Relaxed repetition assistance is limited to one read-only recovery credit and never resets detectors, ledgers, budgets, compiler gates, or completion gates.
- Raw commands, environment values, tokens, file contents, and command output must not enter ordinary Laya logs.
- The sidecar protocol is versioned JSONL over stdin/stdout; stdout is protocol-only and diagnostics use stderr.
- Required verification is `cargo check --tests` followed by `cargo test` in the feature worktree.

## Review Focus

- A command with shell metacharacters that looks read-only to a model must still require confirmation; pin this to the shell eligibility tests in Task 3.
- A high-confidence Laya `read_only` result must not bypass Plan mode, explicit destructive commands, or known mutation; pin this to monotonic policy tests in Task 3.
- A sidecar timeout or malformed response must never allow execution and must not cause repeated restart storms; pin this to the fake-sidecar tests in Task 2.
- A read-only repetition credit must expire after one recovery round and cannot be used by a mutation; pin this to the repetition tests in Task 4.
- `off` mode must not spawn Python or change the existing authorization path; pin this to config/runtime tests in Tasks 1 and 5.

---

### Task 1: Add Laya configuration, CLI controls, and core advisory types

**Files:**
- Create: `src/laya.rs`
- Modify: `src/config.rs` (`AppConfig`, `TomlConfig`, defaults, load/apply/save paths)
- Modify: `src/cli.rs` (`Commands`, new `LayaCommand`, `LayaMode` parsing)
- Modify: `src/lib.rs` (early `laya` command dispatch)
- Modify: `src/app/state.rs` (process-lived runtime handle initialized from config)
- Test: inline tests in `src/laya.rs`, `src/cli.rs`, and `src/config.rs`

**Interfaces:**
- Produces `crate::laya::LayaMode`, `LayaConfig`, `AdvisoryKind`, `AdvisoryRequest`, `AdvisoryDecision`, `AdvisoryError`, and `LayaRuntime` for later tasks.
- `LayaMode` has `Off`, `Shadow`, and `Relaxed` variants and is serde-compatible with `off`, `shadow`, and `relaxed`.
- `LayaConfig` contains `mode`, optional `python`, optional `adapter`, optional `model`, `timeout_ms`, `min_confidence`, and `max_extra_read_only_recoveries` with the spec defaults.
- `LayaRuntime::new(config: LayaConfig) -> Self` is lazy and does not spawn a process.
- `LayaRuntime::evaluate(&self, request: AdvisoryRequest) -> Result<AdvisoryDecision, AdvisoryError>` is async-safe and is a no-op/fallback when mode is `Off`.
- `rustcode laya status`, `rustcode laya enable --mode shadow|relaxed`, and `rustcode laya disable` are parsed by Clap and handled before normal interactive startup.

- [ ] **Step 1: Write failing CLI and config tests.**

  Add tests asserting that the four command forms parse, invalid modes are rejected, `LayaConfig::default()` is off with `timeout_ms = 150`, confidence `0.98`, and one extra recovery, and an old TOML without `[laya]` loads with the same defaults.

- [ ] **Step 2: Run the focused tests and verify they fail.**

  Run `cargo test laya --lib`.

  Expected: compilation/test failure because the new command, config section, and types do not exist.

- [ ] **Step 3: Implement the data types and config overlay.**

  Add a serde-defaulted `LayaConfig` field to `AppConfig`, mirror it in `TomlConfig`, apply it in both global and project config overlays, serialize it in `save_config_to`, and keep invalid values fail-closed to `off`. Add `#[path = "laya.rs"] pub(crate) mod laya;` in the crate module declarations used by `src/lib.rs`.

  Add the following policy-facing shapes in `src/laya.rs`:

  ```rust
  pub enum AdvisoryKind { ShellPolicy, Repetition }

  pub struct AdvisoryRequest {
      pub id: String,
      pub kind: AdvisoryKind,
      pub input: serde_json::Value,
      pub deadline: std::time::Duration,
  }

  pub struct AdvisoryDecision {
      pub label: String,
      pub confidence: f32,
      pub effects: Vec<String>,
      pub rationale_code: Option<String>,
  }
  ```

  Keep sidecar transport details private to the module so policy callers depend only on these bounded types.

- [ ] **Step 4: Implement CLI state changes and status output.**

  Add a `LayaCommand` subcommand with `Status`, `Enable { mode }`, and `Disable`. Load the workspace config, mutate only `config.laya.mode` for enable/disable, call `save_entire_config`, and print a status report containing mode, platform, Python executable, adapter/model presence, and availability category. Never install packages or download a model.

  Make `status` use a pure diagnostic function that can report missing Python, unsupported architecture, missing adapter, and missing model without starting a model process.

- [ ] **Step 5: Initialize the process-lived runtime without spawning it.**

  Add the lazy runtime handle to `AppState` and initialize it from the loaded config in `AppState::new`. Preserve all existing constructors and test fixtures by using the same `LayaRuntime::new` default path. Ensure the runtime is not created or queried in `off` mode beyond holding its disabled configuration.

- [ ] **Step 6: Run focused tests and commit the task.**

  Run `cargo test laya --lib` and `cargo test cli::tests --lib`.

  Commit with `feat: add opt-in laya configuration and controls`.

### Task 2: Implement the persistent JSONL sidecar client and Laya-MLX adapter

**Files:**
- Modify: `src/laya.rs` (process lifecycle, framing, validation, status diagnostics)
- Create: `scripts/laya_sidecar.py`
- Create: `scripts/laya_sidecar_protocol.md`
- Test: inline Rust tests in `src/laya.rs` using a fake sidecar executable/script

**Interfaces:**
- Consumes `LayaConfig`, `AdvisoryRequest`, and `AdvisoryDecision` from Task 1.
- Produces `LayaRuntime::evaluate`, `LayaRuntime::status`, and stable `AdvisoryError` categories for policy callers.
- The sidecar accepts one JSON request per line after one readiness line and returns one correlated JSON response per request.

- [ ] **Step 1: Write failing transport tests.**

  Add fake-sidecar tests for readiness, request ID correlation, successful shell and repetition responses, malformed JSON, unknown protocol version, missing confidence, timeout, process exit, oversized response, and a second request after a recoverable process failure. Assert that errors are categorized and no error result is treated as an allow.

- [ ] **Step 2: Run the focused transport tests and verify they fail.**

  Run `cargo test laya::tests --lib`.

  Expected: failure because the sidecar process client is not implemented.

- [ ] **Step 3: Implement bounded JSONL process management.**

  Use Tokio `Command` with piped stdin/stdout/stderr. Spawn lazily from `LayaRuntime`, read and validate the readiness line, assign caller-owned IDs, write one bounded request line, await the matching response with `tokio::time::timeout`, and reject unknown versions, duplicate IDs, non-finite confidence, unsupported labels, and overlong lines. Keep at most one request in flight. On failure, mark the runtime unavailable for the current turn and allow one rate-limited restart on a later request.

  Keep `mode = off` as an early return before any `Command` call. Classify failures as `unavailable`, `invalid_request`, `timeout`, `malformed_response`, `model_error`, or `process_exit`.

- [ ] **Step 4: Add the minimal pinned Python adapter.**

  Implement `scripts/laya_sidecar.py` with no network/download behavior: parse startup arguments for the local model path, import `laya_mlx`, load the checkpoint once, emit a readiness object, validate each bounded request, call the package's local prediction API, normalize the result to `label`, `confidence`, `effects`, and `rationale_code`, and write only JSON responses to stdout. Send import/model/inference diagnostics to stderr and exit nonzero on startup failure.

  Document Python `>=3.11`, the pinned `laya-mlx` revision/version, compatible MLX range, checkpoint revision, Apple Silicon requirement, and a sample config in `scripts/laya_sidecar_protocol.md`.

- [ ] **Step 5: Implement status diagnostics without inference.**

  Check architecture, Python executable availability, adapter readability, model path readability, and configured protocol version. Return structured categories that the CLI can render without exposing full paths beyond the user-configured path.

- [ ] **Step 6: Run transport and static checks, then commit.**

  Run `cargo test laya::tests --lib`, `python3 -m py_compile scripts/laya_sidecar.py` when the installed interpreter supports it, and `git diff --check`.

  Commit with `feat: add persistent laya sidecar client`.

### Task 3: Integrate monotonic shell policy and cached assessments

**Files:**
- Modify: `src/tools/exec/policy.rs` (expose deterministic classification facts)
- Modify: `src/tools/mod.rs` (effective authorization helper and classification bridge)
- Modify: `src/network/policy.rs` (Laya-aware interactive approval)
- Modify: `src/network/turn/tools.rs` (one assessment per call/batch and scheduler use)
- Modify: `src/network/tool_exec.rs` (execution-time enforcement despite batch bypass)
- Modify: `src/network/turn/context.rs` (turn-scoped assessment cache)
- Test: `src/tools/tests.rs`, `src/network/tests.rs`, and inline policy tests

**Interfaces:**
- Consumes `LayaRuntime`, `LayaMode`, and `AdvisoryDecision` from Tasks 1–2.
- Produces `ShellAssessment` and `EffectiveAuthorization` used consistently by scheduling, interactive confirmation, and execution.
- `ShellAssessment` contains the local classification, eligibility, optional advisory decision, and effective authorization; it is immutable once cached for a call ID/signature.

- [ ] **Step 1: Write failing classification and monotonicity tests.**

  Cover `git status`, `ls`, and `rg` as local read-only candidates; `rm`, destructive Git, redirection, command substitution, backgrounding, `sudo`, pipelines/mixed lists, network commands, Plan mode, unknown commands, and explicit mutations as non-relaxable. Add a fake high-confidence `read_only` advisory and assert that only an eligible unclassified command changes from confirmation to allow; assert all protected cases remain confirmation or denial.

- [ ] **Step 2: Run the focused policy tests and verify they fail.**

  Run `cargo test shell --lib` and `cargo test authorize --lib`.

  Expected: failures for the new assessment and relaxed-mode cases.

- [ ] **Step 3: Expose local shell facts without weakening existing policy.**

  Add a policy classification enum/helper in `src/tools/exec/policy.rs` that reuses `command_confirmation_scope` and records redirection, backgrounding, privilege, known destructive, network, mixed-list, and unclassified facts. Do not change the existing `command_requires_confirmation` result in this step.

- [ ] **Step 4: Implement the effective shell assessment.**

  In `src/tools/mod.rs`, compose the local authorization with the advisory result. Call Laya only for eligible `run_command` calls in shadow/relaxed modes, require exact `read_only` plus `confidence >= min_confidence`, reject any mutation/process/network effect, and allow a relaxed downgrade only for a local `RequireConfirmation` caused solely by an otherwise-unclassified safe candidate. Never downgrade `Deny`, Plan denial, explicit destructive confirmation, or a known hazard. Treat missing/failed/low-confidence results as the unchanged local decision.

- [ ] **Step 5: Cache and reuse the assessment across the turn.**

  Add a turn-scoped cache keyed by call ID when present, otherwise by a stable hash of tool name and normalized arguments. During `selected_tool_call_indices` calculation, compute the assessment once and use its effective read-only status for scheduling. Pass the same cached assessment into `InteractivePolicy::should_approve` and `confirm_and_execute_for_call`; remove the implicit assumption that `bypass_confirm = true` means the execution path can skip the effective policy.

- [ ] **Step 6: Enforce execution-time safety.**

  In `confirm_and_execute_for_call`, keep the existing authorization call as a hard floor and add the cached assessment check immediately before execution. A missing assessment, a stale cache key, or a stricter current local decision must require the existing confirmation/denial result. The batch executor may bypass the UI only after the approved effective assessment is present.

- [ ] **Step 7: Run policy and execution tests, then commit.**

  Run `cargo test shell --lib`, `cargo test authorize --lib`, and the relevant `cargo test network::tests --lib` filters.

  Commit with `feat: apply laya advisory shell policy`.

### Task 4: Add bounded read-only repetition assistance

**Files:**
- Modify: `src/network/turn/context.rs` (turn-scoped recovery credit)
- Modify: `src/network/turn/tools.rs` (advisory request and recovery decision)
- Modify: `src/network/loop_detect.rs` only where the existing public classification boundary requires it
- Modify: `src/network/tool_exec.rs` if the cached read-only assessment is needed for result metadata
- Test: `src/network/loop_detect/tests.rs` and `src/network/tests.rs`

**Interfaces:**
- Consumes `AdvisoryKind::Repetition`, the cached shell/read-only assessment, and `LayaConfig::max_extra_read_only_recoveries`.
- Produces a `RecoveryAdvisory` with `NovelEvidence`, `ConfirmatoryEvidence`, `NoNewInformation`, or `Unknown`, plus one turn-scoped consumable credit.

- [ ] **Step 1: Write failing repetition tests.**

  Assert that shadow mode never consumes a credit, relaxed mode grants at most one credit for high-confidence `novel_evidence` or `confirmatory_evidence`, the credit expires after one recovery round, a second repeat still stops, mutation/mixed/network/process calls receive no credit, and detector/ledger/budget counters are not reset.

- [ ] **Step 2: Run the focused repetition tests and verify they fail.**

  Run `cargo test loop_detect --lib` and `cargo test repetition --lib`.

  Expected: failures because no Laya recovery advisory or credit exists.

- [ ] **Step 3: Add the turn-scoped credit state.**

  Add `laya_read_only_recoveries_used` and an optional pending advisory/credit marker to `RecoveryState` or the narrowest existing turn state. Initialize it to zero, do not serialize it into segment checkpoints, and clear it when a recovery round completes or a mutation is observed.

- [ ] **Step 4: Build the repetition request from redacted progress facts.**

  At the existing pre/post recovery decision points, send only tool kind, normalized arguments, recovery reason, and bounded progress fingerprints. Gate requests on relaxed mode, baseline `crate::tools::is_read_only_call`, no mixed batch, and no mutation/process/network effect. Shadow mode may evaluate and log the hypothetical result but cannot alter state.

- [ ] **Step 5: Apply one monotonic recovery credit.**

  Consume one credit only for `novel_evidence` or `confirmatory_evidence` above the configured confidence threshold. Let the existing loop detector, progress ledger, hard read-only recovery cap, compiler gates, and completion gates continue to run. Do not reset any detector or ledger state. Ensure the credit expires immediately after the single permitted round.

- [ ] **Step 6: Normalize the canonical read-only check.**

  Replace the post-result uses of `loop_detect::is_read_only(&name)` in the affected turn path with `crate::tools::is_read_only_call(call)` when the full call is available, preserving the existing name-only fallback only for tool results that have no call object. Add an alias/structured-call regression test.

- [ ] **Step 7: Run repetition tests and commit.**

  Run `cargo test loop_detect --lib`, `cargo test repetition --lib`, and the affected `cargo test network::tests --lib` filters.

  Commit with `feat: add bounded laya repetition advisory`.

### Task 5: Add off-mode integration coverage, diagnostics, and final verification

**Files:**
- Modify: `src/laya.rs`, `src/cli.rs`, `src/config.rs`, and existing network/tool tests as needed for coverage
- Modify: `README.md` or the project configuration documentation location established by the repository conventions
- Create: a small fixture/fake sidecar under `tests/fixtures/` only if the Rust tests cannot use an inline temporary script

**Interfaces:**
- Consumes the complete configuration, sidecar, shell, and repetition interfaces from Tasks 1–4.
- Produces documented user setup, deterministic off-mode behavior, and repository-level verification evidence.

- [ ] **Step 1: Write the off-mode integration test.**

  Configure a temporary runtime with `mode = off` and an adapter path that would fail if spawned. Exercise the policy entry point and assert that it returns the pre-Laya local decision without creating a child process or advisory event.

- [ ] **Step 2: Add status/setup documentation tests or snapshot assertions.**

  Assert that status output names the missing Python/adapter/model categories and that enabling a mode does not claim installation. Document the Apple Silicon/Python 3.11+ prerequisites, pinned package/model requirement, sample TOML, and disable command.

- [ ] **Step 3: Run the full required verification.**

  Run:

  ```text
  cargo fmt -- --check
  cargo check --tests
  cargo test
  git diff --check
  ```

  Expected: all commands exit successfully; tests include the existing suite plus the new Laya cases, and the default/off path remains green.

- [ ] **Step 4: Review the complete branch for safety and scope.**

  Inspect `git diff main...HEAD`, confirm no dependency or model download was added, confirm raw command text is absent from structured Laya logs, and manually verify every relaxed path has a local eligibility check and a strict fallback.

- [ ] **Step 5: Commit final documentation/tests and report evidence.**

  Commit with `test: verify laya fallback and integration behavior`, then report the exact test commands and results before requesting code review.

## Plan Self-Review

### Spec coverage

- User modes, configuration, CLI, and disabled compatibility are covered by Task 1 and Task 5.
- Persistent sidecar lifecycle, versioned JSONL, pinning, bounded input, and status diagnostics are covered by Task 2.
- Shell classification, monotonic policy, cache reuse, batch execution enforcement, and failure fallback are covered by Task 3.
- One-credit read-only repetition behavior, no resets, expiry, and canonical classification are covered by Task 4.
- Shadow observability and replay-friendly redacted metadata are covered by Tasks 2–5 through the runtime diagnostics and policy event hooks.
- Jev is intentionally not implemented as a runtime backend because the approved scope is self-hosted Laya-MLX; the abstraction leaves room for a later backend.

### Placeholder and consistency check

The plan contains no unresolved placeholders. The names used by later tasks are defined in Task 1 or Task 2, and the only policy decision that can relax behavior is the `read_only` shell path described in Task 3. Every task ends with focused tests and a commit.

### Review-focus coverage

The five high-risk inputs in the header are pinned to tests in Tasks 2–5: hazardous shell syntax, protected policy classes, transport failure, one-shot repetition credit, and disabled mode process isolation.
