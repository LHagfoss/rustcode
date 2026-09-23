# Agent loop progress signals

## Goal

Make repeated-tool recovery easier to understand and less likely to cut off a
turn that made useful progress in the same model round. Preserve RustCode's
existing protections against endless retries and unsafe repeated mutations.

## Evidence and motivation

The current tool-result handler in `src/network/turn/tools.rs` already tracks
whether a round produced meaningful progress. That progress clears the
consecutive no-progress streak and can invalidate a provisional no-information
recovery. However, the handler independently sets `output_abort` whenever the
output stagnation detector reaches `Abort`, and `should_apply_loop_recovery`
allows that signal alone to trigger recovery. A batch can therefore contain a
repeated result and meaningful new information, yet still force a recovery
because of the repeated-output signal.

This is a code-reviewed risk, not a failure reproduced in the session most
recently inspected (`01a0cd48510d-7000-99a9-0a21-0a21c8d40034`). That session
completed its first two Teams requests, then the user cancelled a request
because RustCode assumed `JMRanked` meant Discord. The cancelled turn had one
no-progress result, no recovery, and no loop detection. The evidence supports
better diagnostics and a focused regression test; it does not support changing
loop thresholds globally.

## Design

1. Keep the existing repeated-output warning visible.
2. At the end of each tool batch, emit one structured `turn.loop_signal`
   operational event with the raw output-stagnation status, whether the round
   made meaningful progress, any independent evidence-recovery signal, and
   whether recovery was taken or suppressed (including the reason).
3. Suppress recovery escalation caused solely by output stagnation when the
   same round also produced meaningful progress. Keep the raw signal in the
   event and retain the warning so the repeated result remains visible.
4. Do not suppress independent recovery signals: churn, failed-mutation
   replanning, repeated successful verification, infrastructure failure, and
   the hard turn budgets retain their existing behavior.
5. Do not change repetition thresholds or the cross-turn reasoning detector.

## Behavior examples

- A batch containing an old repeated result and a distinct meaningful result
  records the output signal and progress. It warns about the repeated result
  but does not force output-only recovery.
- A round with only repeated output continues to warn and escalate at the
  existing threshold.
- A meaningful result does not bypass an independent failure, churn, mutation,
  or budget guard.

## Acceptance criteria

- Add deterministic unit coverage for `output_abort` with and without
  meaningful progress, including an independent evidence-recovery signal.
- Add coverage for the structured event fields and the taken/suppressed
  decision.
- Confirm all-repeated batches still escalate at the existing threshold.
- Confirm mixed repeated/fresh batches do not escalate on output stagnation
  alone, regardless of result order.
- Preserve failed-mutation replanning, repeated-verification handling,
  cross-turn recovery, infrastructure stops, and hard turn limits.
- No user-facing wording is removed; repeated-output warnings remain.

## Scope

Expected files are `src/network/turn/tools.rs` and its focused tests. This
design does not include live queue steering, tool scheduling concurrency,
permission policy changes, UI changes, or global loop-threshold adjustments.

## Rollout and evidence

Compare `turn.loop_signal` events and existing `turn.summary` metrics against
the inspected sessions from 2026-09-23. Those sessions had no reasoning-loop
recoveries; use them as a zero-recovery baseline, not as proof of a reproduced
failure. When a future session shows a mixed batch, verify that the raw signal
is recorded and that only output-only recovery is suppressed.
