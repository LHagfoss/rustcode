# Agent Loop Progress Signals Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Record loop signals structurally and prevent output-stagnation alone from forcing recovery in a model round that also made meaningful progress.

**Architecture:** Keep the existing output detector and thresholds. Add a round-progress input to the loop-recovery arbiter, build a structured event from the detector and recovery decision, and emit that event from the existing post-batch handler. Pure decision/event helpers stay local to `src/network/turn/tools.rs` so deterministic unit tests can cover both signal and policy.

**Tech Stack:** Rust, `serde_json`, existing `operational_event` logger, in-file unit tests.

**Spec:** `docs/superpowers/specs/2026-09-23-agent-loop-progress-signals-design.md`

## Global Constraints

- Keep the existing repeated-output warning visible.
- Do not change repetition thresholds or the cross-turn reasoning detector.
- Do not suppress independent recovery signals: churn, failed-mutation replanning, repeated successful verification, infrastructure failure, and the hard turn budgets retain their existing behavior.

## Review Focus

- A repeated-output abort with meaningful progress in the same round: output-only recovery is suppressed and the raw signal remains visible.
- Repeated outputs without meaningful progress: recovery still fires at the current threshold.
- An independent evidence recovery in a mixed-progress round: that independent recovery still fires.
- A requested completion with stale output signals: no loop recovery is injected after completion is requested.
- Result order in a mixed batch: meaningful progress suppresses output-only recovery regardless of which result was processed first.

---

### Task 1: Reopen the existing evidence-linked tracking issue

**Files:**
- Update: GitHub issue #979, linked to this spec and the implementation PR

**Interfaces:**
- Consumes: accepted spec, session evidence, and the related closed issue search.
- Produces: a reopened tracking issue whose update distinguishes the remaining output-abort path from the earlier evidence-recovery case.

- [ ] **Step 1: Reopen #979 and comment before implementation**

The closed #979 issue already covers mixed batches with fresh evidence, so do not create a duplicate. Save this focused update as `/tmp/rustcode-issue-979-update.md`:

```markdown
Reopening to track a remaining mixed-batch recovery path found during the 2026-09-23 review.

The current batch handler clears a provisional no-information evidence recovery when another result in the same batch makes meaningful progress. A separate output-stagnation abort still flows directly into `should_apply_loop_recovery`, so a mixed batch may still force recovery based only on identical output even after meaningful progress. This is the remaining case for the same mixed-batch problem tracked here; no new issue is needed.

This is a code-reviewed risk, not a reproduced failure in the latest inspected session `01a0cd48510d-7000-99a9-0a21-0a21c8d40034`. That session recorded zero reasoning-loop detections and zero evidence recoveries; its only no-progress result occurred during a user-cancelled Discord-versus-Teams routing mistake.

## Acceptance criteria

- Emit `turn.loop_signal` with raw output-stagnation status, same-round progress, independent evidence signal, and the recovery decision.
- Keep repeated-output warnings and current all-repeated escalation thresholds.
- Suppress recovery caused solely by output stagnation if the same round made meaningful progress, regardless of result order.
- Preserve independent churn, failed-mutation, repeated-verification, infrastructure, and hard-budget guards.
- Add deterministic unit coverage for mixed-progress and all-repeated rounds.
```

Then run:

```bash
gh issue reopen 979
gh issue comment 979 --body-file /tmp/rustcode-issue-979-update.md
```

Reference #979 in the PR description.

### Task 2: Define the recovery decision and event payload

**Files:**
- Modify: `src/network/turn/tools.rs:35-42`
- Test: `src/network/turn/tools.rs` unit tests around `should_apply_loop_recovery` (including existing call sites near lines 2478 and 3166)

**Interfaces:**
- Consumes: `loop_detect::LoopStatus`, `round_had_meaningful`, optional evidence-recovery reason.
- Produces: an expanded `should_apply_loop_recovery` policy and a pure `loop_signal_event` payload builder returning `serde_json::Value`.

- [ ] **Step 1: Add failing recovery-decision, order-independence, and event tests**

```rust
#[test]
fn output_abort_does_not_recover_after_meaningful_progress() {
    assert!(!should_apply_loop_recovery(false, true, true, false));
}

#[test]
fn independent_recovery_still_fires_after_meaningful_progress() {
    assert!(should_apply_loop_recovery(false, true, true, true));
}

#[test]
fn output_abort_still_recovers_without_meaningful_progress() {
    assert!(should_apply_loop_recovery(false, true, false, false));
}

#[test]
fn mixed_batch_progress_suppresses_output_only_recovery_in_both_orders() {
    for batch in [
        [(true, false), (false, true)],
        [(false, true), (true, false)],
    ] {
        let mut output_abort = false;
        let mut round_had_meaningful = false;
        for (is_abort, is_meaningful) in batch {
            output_abort |= is_abort;
            round_had_meaningful |= is_meaningful;
        }
        assert!(!should_apply_loop_recovery(
            false,
            output_abort,
            round_had_meaningful,
            false,
        ));
    }
}

#[test]
fn loop_signal_event_preserves_the_raw_abort_and_suppression() {
    let event = loop_signal_event(
        &loop_detect::LoopStatus::Abort(4),
        true,
        None,
        false,
        Some("same_round_meaningful_progress"),
    );
    assert_eq!(event["output_stagnation"]["status"], "abort");
    assert_eq!(event["output_stagnation"]["repeats"], 4);
    assert_eq!(event["round_had_meaningful"], true);
    assert_eq!(event["recovery_taken"], false);
    assert_eq!(event["recovery_suppressed_reason"], "same_round_meaningful_progress");
}
```

- [ ] **Step 2: Run the focused tests and verify the new cases fail to compile or fail their assertions**

Run: `cargo test --lib output_abort_does_not_recover_after_meaningful_progress`

Expected: FAIL because the existing recovery helper has no round-progress parameter and currently accepts output abort unconditionally.

- [ ] **Step 3: Implement the pure recovery predicate and event payload builder**

```rust
fn should_apply_loop_recovery(
    completion_requested: bool,
    output_abort: bool,
    round_had_meaningful: bool,
    has_evidence_recovery: bool,
) -> bool {
    !completion_requested
        && (has_evidence_recovery || (output_abort && !round_had_meaningful))
}
```

Add `loop_signal_event` beside the predicate. It should accept the raw `LoopStatus`, `round_had_meaningful`, optional evidence reason, whether recovery was selected, and an optional suppression reason; return an object with fields `output_stagnation: {status, repeats}`, `round_had_meaningful`, `evidence_recovery`, `recovery_taken`, and `recovery_suppressed_reason`. Encode `Ok`, `Warning(n)`, and `Abort(n)` as `ok`, `warning`, and `abort`, preserving `n` in `repeats`.

- [ ] **Step 4: Run focused tests and verify the policy tests pass**

Run: `cargo test --lib output_abort_does_not_recover_after_meaningful_progress`

Expected: PASS.

- [ ] **Step 5: Add and run event-payload tests**

Add companion tests using the pure payload builder. For an all-repeated abort, assert `recovery_taken` is true and `recovery_suppressed_reason` is null. For a mixed-progress `ProgressReason::Churn`, assert the event retains the churn reason separately from output stagnation and reports recovery taken.

Run: `cargo test --lib loop_signal_event`

Expected: PASS with all payload fields asserted.

- [ ] **Step 6: Commit the recovery decision and payload tests**

```bash
git add src/network/turn/tools.rs
git commit -m "test(agent): cover loop signal recovery policy"
```

### Task 3: Apply the policy at the end of each tool batch

**Files:**
- Modify: `src/network/turn/tools.rs:1975-2003`
- Test: `src/network/turn/tools.rs` recovery tests

**Interfaces:**
- Consumes: `stagnation`, `round_had_meaningful`, `evidence_recovery`, `completed` from the existing batch handler.
- Produces: one `turn.loop_signal` operational event per completed tool batch and the correctly selected recovery notice.

- [ ] **Step 1: Pass round progress into the recovery arbiter and emit `turn.loop_signal`**

At the existing decision point, compute output-only suppression explicitly, preserve the current warning path, update both production call sites and every existing unit-test call site for `should_apply_loop_recovery(completed, output_abort, round_had_meaningful, evidence_recovery.is_some())`, build the payload with `loop_signal_event`, and pass it to `crate::logger::operational_event("turn.loop_signal", payload)` exactly once per batch. Set `recovery_suppressed_reason` to `same_round_meaningful_progress` only when an output abort was present, meaningful progress occurred, completion was not requested, and no independent evidence-recovery signal will fire.

- [ ] **Step 2: Run focused tests and verify both mixed-progress and all-repeated paths**

Run: `cargo test --lib mixed_batch_progress_suppresses_output_only_recovery_in_both_orders`

Expected: PASS; also run `cargo test --lib should_apply_loop_recovery` and `cargo test --lib loop_signal_event` to verify completion, evidence-recovery, and payload behavior.

- [ ] **Step 3: Review the diff for preserved independent guards**

Inspect the final diff around failed-mutation replanning, repeated verification, evidence-recovery invalidation, infrastructure stops, and round budgets. Confirm none of those paths or thresholds changed.

- [ ] **Step 4: Commit the batch-decision integration**

```bash
git add src/network/turn/tools.rs
git commit -m "fix(agent): respect progress in loop recovery"
```

### Task 4: Verify the branch against the project workflow

**Files:**
- Review: `src/network/turn/tools.rs`
- Verify: workspace Cargo targets

**Interfaces:**
- Consumes: the completed loop-signal instrumentation and unit coverage.
- Produces: a verified branch with no unrelated changes.

- [ ] **Step 1: Run formatting and inspect the final diff**

Run: `cargo fmt --check`

Expected: exit code 0 and no formatting changes required.

- [ ] **Step 2: Run the required compile check**

Run: `cargo check --tests`

Expected: exit code 0.

- [ ] **Step 3: Run the full RustCode test suite**

Run: `cargo test`

Expected: exit code 0.

- [ ] **Step 4: Compare the 2026-09-23 session baseline**

Check the inspected session summaries and new events. Confirm earlier sessions had zero reasoning-loop recoveries, then document that `turn.loop_signal` now reports each batch without retroactively treating those sessions as a reproduced loop failure.
