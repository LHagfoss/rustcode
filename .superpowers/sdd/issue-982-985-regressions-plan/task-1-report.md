# Task 1 — Issue #982: Batch Validation Attribution

## Implementation

Schema validation now runs per retained tool call before execution. The turn handler executes only calls with no per-call validation error, then interleaves typed `Validation` failures back into the original result order. This preserves positional provider call-ID pairing: the malformed call receives its own error and a valid sibling receives its execution result rather than the malformed call's message.

Batch-wide validation remains in place after filtering per-call failures, so duplicate-call, mutation-limit, and control-plane constraints continue to reject the batch as before.

## Files

- `src/tools/mod.rs`: extracted single-call validation and added ordered per-call error collection.
- `src/network/turn/tools.rs`: filters invalid calls from execution and merges validation failures into their original positions.
- `src/network/tests.rs`: regression test for an invalid `call_invalid` and valid `call_valid` batch; only the invalid call receives the schema error.

## Tests and results

- `cargo test mixed_batch_validation_errors_are_isolated_to_the_failing_call_id`: passed.
- `cargo check --tests`: passed (existing warnings only).
- `cargo test`: 1,102 passed; 1 failed: `ui::tests::status_panels_render_minimal_inline` at `src/ui/tests.rs:1982`. The assertion expected `" ✕ YOLO mode disabled "` and received `" YOLO mode disabled "`.

## TDD evidence

The regression test was added before `validation_errors_by_call` existed. The focused test initially failed to compile with `E0425` because that function was absent. After the minimal per-call validation and result-interleaving implementation, the focused test passed.

## Self-review

The change is limited to per-call schema validation attribution and preserves the existing batch validator for shared constraints. Result order is retained before history persistence, where provider call IDs are attached positionally. `git diff --check` was clean.

## Concerns

The full suite has one failure in a pre-existing, protected user edit to `src/ui/tests.rs`; it was not modified or staged. Existing compiler warnings remain unrelated to this task.
