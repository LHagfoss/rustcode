# Manual Compaction Lifecycle Fix Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make manual `/compact` cancellable by the active session token and prevent old-session compaction tasks from mutating a newly active session's history.

**Architecture:** Thread an optional cancellation-token reference through the existing manual compaction API into the already cancellation-aware summary request. Centralize stale-notice handling behind a session-identity guard so both detached success and failure paths log old-session results without touching current chat history.

**Tech Stack:** Rust, Tokio, `tokio_util::sync::CancellationToken`, Reqwest, built-in Rust tests.

## Global Constraints

- Preserve `Cargo.lock` and `images/rustcode-logo.png` exactly and never stage them.
- Do not run whole-repository `cargo fmt`.
- Work on the current `feature/bounded-context-compaction` branch.
- Commit implementation separately after all requested verification passes.

---

### Task 1: Propagate Manual Compaction Cancellation

**Files:**
- Modify: `src/network/compaction.rs:291-358`
- Modify: `src/app/actions.rs:126-192`
- Test: `src/network/compaction.rs:560-933`
- Test: `src/app/actions.rs:1575-1755`

**Interfaces:**
- Consumes: the active `&mut CancellationToken` supplied to `handle_enter`.
- Produces: `force_compact(client, url, model, history, cancel_token: Option<&CancellationToken>) -> Result<(usize, usize), String>`.

- [ ] **Step 1: Add failing cancellation tests**

Add a pending-response TCP helper and a `force_compact` test that starts a real request, cancels the supplied token, requires completion within one second, and verifies the original history remains unchanged. Add an action-level test that invokes `/compact`, waits until its request is accepted, invokes `/cancel` using the same active token slot, and requires the detached operation to produce its existing same-session failure notice within one second.

Use this server shape in each test module so the real Reqwest request remains
pending until cancellation:

```rust
async fn pending_response_server() -> (String, tokio::sync::oneshot::Receiver<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test server");
    let address = listener.local_addr().expect("test server address");
    let (accepted_tx, accepted_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let (socket, _) = listener.accept().await.expect("accept request");
        accepted_tx.send(()).ok();
        std::future::pending::<()>().await;
        drop(socket);
    });
    (format!("http://{address}"), accepted_rx)
}
```

The compaction-level test spawns `force_compact` with `Some(&task_token)`,
waits for `accepted_rx`, cancels the original token, and uses
`tokio::time::timeout(Duration::from_secs(1), task)` to require the existing
`Err("Failed to generate summary.")` result. The action-level test sets eight
messages and the pending URL on `AppState`, invokes `/compact`, then `/cancel`,
and uses a one-second timeout around a yield loop that observes the existing
`History compaction failed:` notice.

- [ ] **Step 2: Run the focused tests and verify RED**

Run:

```bash
cargo test --locked manual_compaction_cancellation -- --nocapture
```

Expected: failure because `force_compact` cannot yet receive the token and the detached `/compact` request does not observe `/cancel`.

- [ ] **Step 3: Implement minimal token propagation**

Change `force_compact` to accept `Option<&CancellationToken>` and pass it directly to `force_compact_internal`. Before dropping the app-state lock and spawning `/compact`, clone the current active token:

```rust
let compaction_cancel_token = cancel_token.clone();
```

Inside the detached task, pass `Some(&compaction_cancel_token)` to `force_compact`. Update non-session callers and existing tests to pass `None`, preserving their behavior.

- [ ] **Step 4: Run the focused tests and verify GREEN**

Run:

```bash
cargo test --locked manual_compaction_cancellation -- --nocapture
```

Expected: both cancellation tests pass and history is not replaced by a cancelled summary.

### Task 2: Guard Stale Notices by Session Identity

**Files:**
- Modify: `src/app/actions.rs:153-190`
- Modify: `src/app/actions.rs:844-879`
- Test: `src/app/actions.rs:1634-1734`

**Interfaces:**
- Consumes: live session ID, captured session ID, and mutable live history.
- Produces: `report_stale_compaction(live_session_id, captured_session_id, history)`, which logs on a session mismatch and appends the existing notice only for a same-session history conflict.

- [ ] **Step 1: Add failing mismatch and same-session tests**

Change the stale-report helper tests to assert these literal outcomes:

```rust
// Different session: exact history equality before and after.
assert_eq!(live_history, expected);

// Same session conflict: existing history is preserved and one stale notice is appended.
assert_eq!(live_history.len(), expected.len() + 1);
assert!(live_history.last().unwrap().content.contains("discarded as stale"));
```

- [ ] **Step 2: Run the focused tests and verify RED**

Run:

```bash
cargo test --locked manual_compaction_stale_report -- --nocapture
```

Expected: the mismatch test fails because the current helper always appends to live history.

- [ ] **Step 3: Implement the guarded stale reporter**

Pass live and captured session IDs from both detached success and failure branches. Return after an operational `dbg_log!` when the IDs differ; otherwise append the existing stale notice unchanged. Do not alter normal success or failure notices.

- [ ] **Step 4: Run focused compaction and action tests**

Run:

```bash
cargo test --locked compaction -- --nocapture
```

Expected: all matching compaction tests pass.

### Task 3: Verify, Review, and Commit

**Files:**
- Verify: `src/app/actions.rs`
- Verify: `src/network/compaction.rs`
- Preserve: `Cargo.lock`
- Preserve: `images/rustcode-logo.png`

**Interfaces:**
- Consumes: completed implementation and tests.
- Produces: one verified conventional implementation commit.

- [ ] **Step 1: Format only touched Rust files**

Run:

```bash
rustfmt --edition 2024 src/app/actions.rs src/network/compaction.rs
git status --short -- Cargo.lock images/rustcode-logo.png
```

Confirm the protected paths retain their original pre-task status.

- [ ] **Step 2: Run requested verification**

Run:

```bash
cargo test --locked
cargo clippy --all-targets --all-features -- -D warnings
git diff --check
```

Expected: every command exits zero.

- [ ] **Step 3: Review the final diff and protected files**

Confirm the diff contains only the lifecycle fix, focused tests, and this plan; verify the protected-file hashes still match their starting values and neither protected path is staged.

- [ ] **Step 4: Commit implementation separately**

Stage only the plan and intended Rust files, then commit:

```bash
git commit -m "fix: bind manual compaction to session lifecycle"
```
