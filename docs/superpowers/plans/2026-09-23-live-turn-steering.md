# Live Turn Steering Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let users steer a safe active interactive turn at a finalized tool-result boundary, interrupt to apply pending steers immediately, and preserve normal FIFO follow-ups.

**Architecture:** Keep pending steering messages and draft submission mode in `AppState`, separate from `pending_queue`. An explicit regular-interactive-turn marker gates both Enter and Tab; it must be false for internal wakeups, headless/non-regular work, recovery requests, questions, confirmations, and finalized turns. Under the state mutex, the tool handler appends all result messages (including deferred and cancellation results) before atomically moving pending steers into history; the turn-end and Esc paths atomically move any still-pending steers to the FIFO head. Pending steers are tied to the active session so session switches cannot leak them.

**Tech Stack:** Rust, Tokio `Mutex<AppState>`, existing `ChatMessage`/`History`, Crossterm key events, Ratatui render snapshots, Rust unit tests.

**Spec:** [docs/superpowers/specs/2026-09-23-live-turn-steering-design.md](../specs/2026-09-23-live-turn-steering-design.md)

## Global Constraints

- Only a regular interactive agent turn may accept a steer.
- Never inject steering text while a tool approval or question is pending, during a non-regular turn, or after a final response.
- Pending steers must be handed to the model only after the current tool batch has finalized its results, so tool call/result pairing and history ordering remain valid.
- Existing cancel handling and queued-prompt preservation remain in force.
- Slash commands keep their current dispatch behavior and are not converted into steers by this change.
- Do not change model policy, tool permissions, loop thresholds, or queue watchdog timeouts in this project.
- Keep pending steers separate from `pending_queue`; preserve FIFO ordering and the existing queued-prompt editing behavior.
- Each accepted steer makes one atomic transition to active history or the FIFO queue; cancellation races must not lose or duplicate text.
- All pending-steer transitions must verify the session that accepted them before mutating history or a replacement session's queue.

## Review Focus

- A normal interactive model request is the only steerable request: test that the explicit marker is enabled for it and disabled for wakeups, headless/non-regular work, recovery requests, question/confirmation waits, and after finalization.
- A partially executed or deferred tool batch still has a single safe insertion boundary: test result/call pairing and verify all completed, cancelled, and deferred result messages precede accepted steers.
- Cancellation can race with batch completion, turn completion, or session replacement: test lock-order outcomes so each steer occurs exactly once in the active history or FIFO queue, and never crosses sessions.
- Tab and Enter overlap with completion, slash-command, and queue behavior: test completion priority, mode reset/defaults, slash dispatch preservation, and unchanged queue editing.
- A batch can contain multiple steers: test exact submission order, distinct user messages, preview order, and turn-end FIFO order ahead of existing follow-ups.

---

## File Structure

- `src/app/state.rs`: own pending steer text, the explicit steerability/session marker, draft mode, and small atomic transition helpers; clear session-scoped steer state on session replacement.
- `src/app/actions/enter.rs` and `src/app/actions.rs`: route plain-text Enter to pending steer or existing queue behavior; make Esc interrupt and promote pending steers atomically.
- `src/app/runtime/input.rs` and `src/ui/composer.rs`: preserve slash dispatch and completion handling while exposing Tab mode toggling only for a non-empty draft when no completion is available.
- `src/network/policy.rs`, `src/network/turn/queue.rs`, `src/network/turn/recovery.rs`, and `src/network/turn/finish.rs`: establish, constrain, and clear the explicit regular-turn marker and return unaccepted steers at the turn boundary.
- `src/network/turn/tools.rs`: after the whole batch has finalized and its result messages are appended, take and append pending steers atomically before the next provider request.
- `src/ui/render_snapshot.rs`, `src/ui/composer_render.rs`, and `src/ui/render.rs`: snapshot and show separate pending-steer and FIFO previews plus mode/application hints, including the updated layout height.
- Focused tests live beside existing code: `src/app/actions/tests.rs`, `src/app/state/*_tests.rs`, `src/app/composer.rs`, `src/network/turn/tools.rs`, `src/network/turn/queue.rs`, `src/network/policy.rs`, and `src/ui/*` tests/fixtures.

## Interfaces and Ordering Contract

Implement a state-owned representation (suggested names; retain project naming conventions if a nearby pattern is clearer):

```rust
pub(crate) struct PendingSteer {
    pub(crate) session_id: String,
    pub(crate) text: String,
}

pub(crate) enum DraftSubmitMode { Steer, Queue }
```

`AppState` owns `pending_steers: Vec<PendingSteer>`, `draft_submit_mode: DraftSubmitMode`, and an explicit `active_turn_steerable_session: Option<String>`. Add narrow state helpers with these contracts:

- `can_accept_steer(&self) -> bool`: true only when the marker matches `active_session_id`, status is `Streaming`, and neither tool confirmation nor question state is pending.
- `queue_steer(&mut self, text: String) -> bool`: append a distinct non-empty steer only if `can_accept_steer()`; the active session ID is captured with it.
- `take_steers_for_history(&mut self, session_id: &str) -> Vec<String>`: only when the marker and every pending item match `session_id`; atomically take the whole batch, or return empty without touching a different session.
- `promote_pending_steers_to_queue(&mut self, session_id: &str)`: prepend matching pending texts in submission order to `pending_queue`, then clear the pending list and matching steerability marker. Internal wakeups and existing follow-ups remain after the promoted messages.
- `clear_session_steering(&mut self)`: clear pending steers, steerability marker, and draft mode when the active session changes.

When an accepting path wins the state mutex, it owns the transition. Esc first means pending steers become queue entries and the tool handler takes none. Batch handoff first means the steers enter history and Esc finds none to promote. Finalization follows the same rule. Every consumer verifies the captured turn/session ID before using history or queue state.

## Tasks

### Task 1: Add session-scoped steer state and explicit eligibility

**Files:**
- Modify: `src/app/state.rs`
- Modify: `src/app/state/models.rs` only if the chosen small state types belong with other shared models
- Modify: `src/app/session_controller.rs`
- Test: `src/app/state/steering_tests.rs` (new focused child test module, wired from `state.rs`)
- Test: `src/app/session_controller.rs` existing unit tests

**Interfaces:**
- Produces the state fields and helpers from “Interfaces and Ordering Contract”.
- The turn setup in Task 3 sets/clears `active_turn_steerable_session`; Tasks 2, 4, 5, and 6 consume the state helpers.

- [x] **Step 1: Add failing state tests for gating and transitions.** Add tests that construct `AppState::new()`, set `active_turn_steerable_session` to its session, and assert: `Streaming` accepts ordinary non-empty steers; `Idle`, `Queued`, mismatched session marker, `AwaitingToolConfirmation`, pending tool confirmation, `AwaitingQuestion`, and a pending question reject them; empty or whitespace-only text is rejected; multiple accepted steers preserve order and remain separate entries. The core test should use the public state contract directly:

```rust
state.status = AppStatus::Streaming;
state.active_turn_steerable_session = Some(state.active_session_id.clone());
assert!(state.queue_steer("Use Teams".to_owned()));
assert!(state.queue_steer("Keep the same channel".to_owned()));
assert_eq!(state.pending_steers.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(),
           ["Use Teams", "Keep the same channel"]);
assert!(state.pending_queue.is_empty());
```
- [x] **Step 2: Run the focused state tests and confirm failure.** Run `cargo test steering_tests --lib`; expected: compile/test failure because the steering state and helpers do not exist.
- [x] **Step 3: Implement the state representation and atomic helpers.** Keep all eligibility decisions and pending-list mutation in `AppState`; make `promote_pending_steers_to_queue` use ordered prepend semantics (`pending_queue.splice(0..0, prompts)`) and reject a stale `session_id` without clearing or moving another session's values.
- [x] **Step 4: Add session transition cleanup tests and implementation.** Extend the session-controller tests: create pending steers, call each transition that changes `active_session_id` (fresh, resume, fork, archive/delete when they replace the active session), and assert no steer is visible or queued in the replacement session. Clear the marker and draft mode on the same state transition path that already drops other session-local UI state.
- [x] **Step 5: Run the focused tests.** Run `cargo test steering_tests --lib` and `cargo test session_controller --lib`; expected: PASS, with existing session transition tests still passing.

### Task 2: Route Enter and Tab through steer or queue modes

**Files:**
- Modify: `src/app/actions/enter.rs`
- Modify: `src/app/runtime/input.rs`
- Modify: `src/ui/composer.rs`
- Modify: `src/app/composer.rs` if queued-prompt pullback must reset mode
- Test: `src/app/actions/tests.rs`
- Test: `src/app/composer.rs`

**Interfaces:**
- Consumes: `AppState::can_accept_steer`, `queue_steer`, and `DraftSubmitMode` from Task 1.
- Produces: Enter submissions become pending steers in `Steer` mode, or append to `pending_queue` in `Queue` mode; when not steerable, retain current FIFO behavior. A new draft defaults to `Steer` only while the marker remains valid.

- [x] **Step 1: Add failing Enter tests.** Test a steerable streaming state with plain text in default mode: Enter clears the draft and records one separate steer without changing `pending_queue`. Set mode to `Queue`: Enter appends only to `pending_queue`. With each unsteerable state from Task 1, Enter keeps existing queue behavior. A slash command during a steerable turn still takes its existing command dispatch path and is not stored as a steer.
- [x] **Step 2: Run focused Enter tests and confirm failure.** Run `cargo test actions::tests --lib`; expected: new steering assertions fail against the current unconditional queue behavior.
- [x] **Step 3: Implement Enter routing after completion acceptance and slash-command classification.** Preserve `selected_file_completion` handling first and keep slash commands on their existing dispatch path. For non-command text, consult the explicit state helper and draft mode; queue normally when steering is unavailable. Clear the submitted draft and restore the next-draft default from the current marker.
- [x] **Step 4: Add failing Tab tests.** Test non-empty drafts toggle `Steer`/`Queue` while the marker is valid and no completion suggestion exists; empty drafts do not toggle; a valid file/command completion retains the current Tab completion action and does not change mode; Tab outside a steerable turn retains current behavior.
- [x] **Step 5: Implement the Tab mode action in the input/composer path.** Check completion eligibility using the same existing suggestion state and query used for Tab completion. Only consume Tab for mode toggling when there is no available suggestion, the draft is non-empty, and `can_accept_steer()` is true. Request redraw after changing mode.
- [x] **Step 6: Run focused tests.** Run `cargo test actions::tests --lib` and `cargo test composer --lib`; expected: PASS, including existing autocomplete and `↑ edit last queued` behavior.

### Task 3: Mark only regular interactive model requests as steerable

**Files:**
- Modify: `src/network/policy.rs`
- Modify: `src/network/turn/queue.rs`
- Modify: `src/network/turn/mod.rs`
- Modify: `src/network/turn/request.rs` or `src/network/turn/recovery.rs` at the narrow point where a recovery request is initiated
- Modify: `src/network/turn/finish.rs`
- Test: `src/network/policy.rs`
- Test: `src/network/turn/queue.rs`
- Test: `src/network/turn/recovery.rs`

**Interfaces:**
- Consumes: Task 1's explicit session marker and eligibility guard.
- Produces: a marker set only for a real, regular interactive queued user prompt, disabled for internal wakeups, headless/non-regular work, recovery continuation/recovery prompts, questions and confirmations, and final/stop paths.

- [x] **Step 1: Add failing marker lifecycle tests.** Test that an ordinary interactive non-wakeup prompt is marked with its session before the provider request; internal `__task_wakeup__:` prompts and headless policy runs are not. Test that recovery request entry clears the marker and that finalization clears it even on cancellation or failed completion.
- [x] **Step 2: Run focused tests and confirm failure.** RED was observed for missing capability/marker setup and for the finalization marker helper.
- [x] **Step 3: Add an explicit policy capability for interactive steer eligibility.** Add a `TurnPolicy` method defaulting to false and override it only for `InteractivePolicy`; do not infer steerability from `AppStatus::Streaming`, `!is_headless()`, or UI presence alone.
- [x] **Step 4: Set and clear the marker at request lifecycle boundaries.** In queue setup, set the marker for a non-wakeup regular prompt only when the policy capability allows it and the active session matches the claimed turn session. Clear it before internal wakeup/recovery requests, at finalization, and after session mismatch. Ensure the confirmation and question paths make `can_accept_steer()` false while awaiting user input and restore eligibility only when the same regular turn resumes.
- [x] **Step 5: Run focused tests.** Queue, recovery, and policy filters passed; see `.superpowers/sdd/2026-09-23-live-turn-steering/task-3-report.md` for exact counts, finalization-order regression, and full validation.

### Task 4: Append steers after finalized tool-batch results

**Files:**
- Modify: `src/network/turn/tools.rs`
- Test: `src/network/turn/tools.rs`
- Test: `src/network/turn/mod.rs` for cancellation result pairing if a small helper is extracted

**Interfaces:**
- Consumes: Task 1 `take_steers_for_history(session_id)` and Task 3 marker lifecycle.
- Produces: at the next completed tool-batch boundary, append all pending steers as distinct `ChatMessage::new("user", text)` messages after the full batch's result messages and before returning `Continue` for the next provider request.

- [x] **Step 1: Add failing batch-order tests.** Build a deterministic tool batch with multiple native calls where one result completes, one is deferred, and one is denied/cancelled or unselected. Queue two steers before the boundary. Assert every call has exactly one paired result (including typed cancellation/deferred closure), all result messages precede the two user messages, and the steer texts appear once in submission order. The transcript-order assertion should identify the last result index and the two steer message indices, for example:

```rust
assert!(history.iter().any(|message| message.tool_result.as_ref()
    .is_some_and(|result| result.call_id.as_deref() == Some("call-deferred"))));
let steer_positions = history.iter().enumerate()
    .filter(|(_, message)| message.role == "user"
        && ["Use Teams", "Keep the same channel"].contains(&message.content.as_str()))
    .map(|(index, _)| index)
    .collect::<Vec<_>>();
assert_eq!(steer_positions.len(), 2);
assert!(last_batch_result_index < steer_positions[0]);
assert!(steer_positions[0] < steer_positions[1]);
```
- [x] **Step 2: Add a failing cancellation race test.** Exercise the state transition with cancellation already set before the batch result lock: the cancellation path appends completed/cancelled/deferred results, takes no steers already promoted by Esc, and does not duplicate them. Exercise the inverse state-lock order: completed batch takes the steers, then Esc sees an empty pending list.
- [x] **Step 3: Run focused tool tests and confirm failure.** Run `cargo test turn::tools --lib`; expected: new ordering/race tests fail because pending steers are not yet handed off.
- [x] **Step 4: Insert handoff at the finalized append point.** In `handle_tool_response`, append sorted `result_messages`, append unanswered/deferred call results, and only then—while still holding the same `AppState` mutex—take matching-session pending steers and append each as an individual user message. In the cancellation branch, first append all `append_cancelled_batch_results` and deferred/unexecuted results, then take and append any still-pending matching-session steers only if Esc did not already promote them. Do not append between tool calls or before a tool result.
- [x] **Step 5: Persist the updated history and resume.** Save the session history after the result-plus-steer append and before returning `Continue`; retain the existing stop behavior for a cancelled batch and let queue fallback apply only to still-pending items.
- [x] **Step 6: Run focused tests.** Run `cargo test turn::tools --lib`; expected: PASS, including existing tool result pairing/cancellation tests.

### Task 5: Apply pending steers on Esc or turn end exactly once

**Files:**
- Modify: `src/app/actions.rs`
- Modify: `src/network/turn/queue.rs`
- Modify: `src/network/turn/finish.rs` only if final response ownership requires fallback before final transcript save
- Test: `src/app/actions/tests.rs`
- Test: `src/network/turn/queue.rs`

**Interfaces:**
- Consumes: Task 1 `promote_pending_steers_to_queue(session_id)` and Task 3 marker/session lifecycle.
- Produces: Esc with pending steers promotes them to the FIFO head before cancellation; any still-pending steers at the active turn boundary are promoted before ordinary follow-ups. The existing orchestrator lease still owns dequeueing.

- [x] **Step 1: Add failing Esc behavior tests.** Test multiple pending steers plus queued follow-ups: Esc cancels the token, promotes steers to queue positions 0..N in original order, clears pending preview, and preserves all follow-ups/wakeups after them. Test Esc with no pending steers leaves current queue/cancel behavior unchanged.
- [x] **Step 2: Add failing end-of-turn fallback tests.** Simulate a turn completing or cancelling before another tool-result boundary and assert the queue begins with the still-pending steers, then prior follow-ups. Test a session switch before old-turn unwind and assert old-session steers do not enter the replacement session queue.
- [x] **Step 3: Run focused tests and confirm failure.** Run `cargo test actions::tests --lib` and `cargo test turn::queue --lib`; expected: the new fallback assertions fail before implementation.
- [x] **Step 4: Implement Esc promotion under the existing state lock.** Capture the active session ID, call the state helper before cancelling/resetting the token and render projections, then continue existing Esc behavior. Promotion also clears the matching steerability marker immediately, so text submitted during cancellation unwind follows FIFO behavior. With no active matching session or no pending steers, preserve the old path.
- [x] **Step 5: Implement turn-end fallback under the queue orchestrator lock.** Immediately after `run_agent_turn_with_context` returns and before saving context or checking whether to continue/dequeue, verify the session is still `turn_session_id` and promote remaining steers to the FIFO head. This lock-order point ensures Esc, batch handoff, and end-of-turn fallback cannot all consume the same pending item.
- [x] **Step 6: Run focused tests.** Run `cargo test actions::tests --lib` and `cargo test turn::queue --lib`; expected: PASS, including lease/cancellation queue-preservation tests.

### Task 6: Render separate pending-steer and queued-follow-up previews

**Files:**
- Modify: `src/ui/render_snapshot.rs`
- Modify: `src/ui/composer_render.rs`
- Modify: `src/ui/render.rs`
- Modify: `src/ui/composer.rs` for mode-aware footer text if composer rendering owns that footer
- Test: `src/ui/composer_render.rs` or `src/ui/tests.rs`
- Test: `src/ui/render_snapshot.rs`
- Update: `src/ui/fixtures/render_snapshot_*.txt` only where existing fixture output intentionally changes

**Interfaces:**
- Consumes: Task 1 state fields and Task 2 draft mode.
- Produces: separate `Pending steers` and `Queued follow-ups` preview blocks; steer label explains “applies after next tool result”; Esc hint says “interrupt and apply now” only while interruptible; composer shows `Steer`/`Queue` mode and Tab hint; queue count, latest prompts, and `↑ edit last` remain.

- [x] **Step 1: Add failing snapshot/render tests.** Assert a snapshot contains ordered pending steer texts separately from user follow-ups, hides internal wakeups from the queue preview, and carries mode plus the steerability/interruption marker needed for hints. Test pending steers consume preview rows without suppressing the existing FIFO count.
- [x] **Step 2: Run focused render tests and confirm failure.** Run `cargo test ui::render_snapshot --lib` and `cargo test ui::tests --lib`; expected: steering snapshot and output assertions fail.
- [x] **Step 3: Extend the immutable render snapshot.** Copy pending steer texts, current draft mode, and a derived interruptible-steer flag; expose read-only accessors and keep the render layer from inspecting mutable `AppState` directly.
- [x] **Step 4: Render the two previews and composer mode hint.** Add pending-steer height to layout sizing and render a distinct label plus “applies after next tool result”. Render queue count/latest prompts/`↑ edit last queued` as before. Show Esc “interrupt and apply now” only when pending steers exist and the active marker is interruptible. Show Tab mode guidance only for the non-empty steerable draft case when no completion suggestion is active.
- [x] **Step 5: Refresh affected fixtures and tests.** Update only fixture rows whose rendered output changed because of the new intentional UI. Verify both the normal queue-only state and the combined steers-plus-follow-ups state.
- [x] **Step 6: Run focused render tests.** Run `cargo test ui::render_snapshot --lib` and `cargo test ui::tests --lib`; expected: PASS and fixture diffs are limited to the new UI.

### Task 7: Full integration and race regression pass

**Files:**
- Review: all files listed in Tasks 1–6
- Test: focused unit tests plus full crate test suite

**Interfaces:**
- Consumes: all earlier tasks; no new public interfaces.
- Produces: implementation matching every acceptance criterion in the spec.

- [x] **Step 1: Add any missing deterministic integration assertions.** Ensure the combined path tests (a) two steers handed off after a batch, (b) a steer queued by fallback before an existing follow-up, (c) Esc-vs-batch single-winner transition, and (d) session replacement preventing leakage. Use Tokio synchronization gates rather than sleeps for concurrency tests.
- [x] **Step 2: Run focused regression tests.** Run `cargo test steering --lib`, `cargo test turn::tools --lib`, `cargo test turn::queue --lib`, `cargo test actions::tests --lib`, and `cargo test ui::tests --lib`; expected: PASS.
- [x] **Step 3: Run required project validation.** Run `cargo check --tests` followed by `cargo test`; expected: both complete successfully.
- [x] **Step 4: Review scope and state transitions.** Confirm no model/tool permission/loop threshold/watchdog changes, every pending steer has exactly one transition, slash commands retain dispatch behavior, and no stale session can receive the previous session's steering text.

## Plan Self-Review

- **Spec coverage:** submission modes and slash commands (Task 2); regular-turn and unsafe-state gating (Tasks 1 and 3); post-batch ordered handoff and pairing (Task 4); Esc and turn-end FIFO fallback (Task 5); previews/completion affordance (Task 6); all stated deterministic race and validation criteria (Task 7).
- **Harness review findings:** the regular-turn steerability marker is explicit; handoff occurs after finalized batch results; result-plus-steer history mutation is atomic under the state mutex; pending items fall back at turn end in FIFO order; session identity is checked on every transition.
- **Type consistency:** all state mutations are owned by the Task 1 `AppState` helpers; tasks pass the session ID already captured by the queue orchestrator; history handoff returns ordered `Vec<String>` and creates one user message for each string.
- **Review focus coverage:** Task 3 covers request-class eligibility; Task 4 covers finalized batches and cancellation; Task 5 covers races and session replacement; Task 2 covers Tab/completion/slash/queue interactions; Tasks 4–6 cover multi-steer order, transcript distinction, and preview.
