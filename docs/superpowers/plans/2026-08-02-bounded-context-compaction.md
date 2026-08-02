# Bounded Context Compaction Plan

## Goal

Make automatic and manual history compaction finite, cancellable, and safe to
fall back from when the summarizer provider is unavailable.

## Scope

- Add a cancellation-aware, finite timeout around summary requests.
- Bound generated summary text before it is written into history.
- Preserve local pruning when AI summarization fails or is cancelled.
- Keep the existing provider abstraction, compaction format, and Discord RPC
  behavior unchanged.

## Verification

- Test timeout/cancellation and invalid or oversized summaries.
- Run `cargo test --locked`, Clippy with warnings denied, and `git diff --check`.
- Do not run whole-repository formatting; the repository has known baseline
  rustfmt drift.
- Integrate as one feature branch and PR, preserving the user-owned
  `Cargo.lock` and `images/rustcode-logo.png` changes.
