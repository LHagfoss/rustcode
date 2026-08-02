# Manual Compaction Lifecycle Fix Design

## Goal

Make detached `/compact` requests share the active session cancellation lifecycle
and prevent results from an old session from writing notices into a newly active
session.

## Design

`network::compaction::force_compact` gains an
`Option<&CancellationToken>` parameter. It forwards that value unchanged through
`force_compact_internal` and `generate_summary` to the existing
`await_summary_request` cancellation select. Manual `/compact` clones the current
active token before spawning its detached task and passes `Some(&token)`.
Callers that do not participate in a session lifecycle pass `None`, preserving
their current timeout, mutation, and error behavior.

When a detached compaction cannot merge or report its result, stale handling
compares the captured and live session IDs before touching history. A session
mismatch emits only an operational debug log and leaves the newly active history
unchanged. If the session is unchanged but its history conflicts with the captured
snapshot, the existing in-chat stale notice remains acceptable. Normal success
and failure notices remain unchanged when the operation still belongs to the
active session.

## Tests

- Exercise `force_compact` against a pending summary response and prove cancelling
  its supplied token interrupts the request promptly without mutating history.
- Exercise `/compact` with the active token and prove cancelling that token ends
  the detached summary operation.
- Prove stale reporting does not mutate history after a session mismatch.
- Preserve coverage for a same-session history conflict notice and successful
  prefix-preserving merge behavior.

## Constraints and Verification

- Do not modify or stage `Cargo.lock` or `images/rustcode-logo.png`.
- Do not run whole-repository `cargo fmt`.
- Run focused tests, `cargo test --locked`,
  `cargo clippy --all-targets --all-features -- -D warnings`, and
  `git diff --check` before committing the implementation.
