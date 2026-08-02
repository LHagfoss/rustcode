# Agent Harness Context Safety Design

## Goal

Make Rustcode behave more like a production coding-agent harness under large or exploratory model responses: bound tool output before it enters the transcript, preserve recovery paths, and stop malformed or oversized protocol traffic without wasting the entire turn budget.

## Scope

This first sub-project covers the context gateway only. It does not change Discord RPC, the model provider abstraction, the UI renderer, or the turn state machine beyond the metadata needed to account for bounded tool results.

The later sub-projects are intentionally separate:

1. proactive context pressure and compaction;
2. protocol-error and oversized-batch recovery;
3. progress summaries and command-scope guardrails.

## Design

Every tool result passes through one bounded-output boundary before it is appended to history or sent back to the model. The boundary applies byte and line limits, keeps useful head and tail content, and records whether the result was truncated. The full result may be written to the existing artifact store, but the artifact path is metadata only and the full artifact must never be inserted into the model transcript.

`view_file` remains exact for small targeted reads and applies a hard line ceiling to explicit and implicit ranges. Its response must report the actual returned range and give a deterministic follow-up range for omitted content. Byte offsets and line numbers must not produce contradictory metadata.

Command output keeps the tail on failures because compiler diagnostics commonly appear there. Successful output remains compact. Search and symbol tools use the same shared output boundary.

The context gateway exposes testable pure helpers for truncation and accounting. Tests cover byte limits, line limits, head/tail retention, artifact recovery, exact follow-up reads, and the invariant that bounded content—not the original output—is what reaches history.

## Safety invariants

- No single tool result can exceed the configured transcript byte/line limits.
- A failed command retains its diagnostic tail.
- A truncated result is explicitly marked and recoverable.
- Repeating a bounded read cannot grow the context with the same full payload.
- Existing edit idempotency, real diffs, cancellation, and turn budgets remain unchanged.
- Existing user changes and unrelated formatting are not modified.

## Verification

The feature is accepted only after focused tests, the full Rust test suite, clippy with warnings denied, and `git diff --check` pass. Repository-wide rustfmt baseline drift is documented; `cargo fmt` must not be run as a whole-repository mutation.
