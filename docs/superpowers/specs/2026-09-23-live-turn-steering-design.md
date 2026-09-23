# Live turn steering

## Goal

Let users redirect an active RustCode turn at the next tool-result boundary, or interrupt the turn and apply the correction immediately, while keeping ordinary follow-up prompts in the existing FIFO queue.

## Evidence and motivation

RustCode currently appends every normal submission to `AppState.pending_queue` and processes prompts one at a time in `src/network/turn/queue.rs`. It already previews queued prompts and supports pulling the last one back into the composer. It has no distinct input path for a correction to the active turn.

In session `01a0cd48510d-7000-99a9-0a21-0a21c8d40034`, the model interpreted `JMRanked` as a Discord destination, retried discovery for four rounds, and was cancelled before the user corrected it to Teams. RustCode then needed two more rounds to finish using the correction. The session recorded one no-progress tool result, zero loop recoveries, and zero reasoning-loop detections. Live steering would not fix the initial app guess, but it can make a correction reach the active task sooner.

The same session contains a watchdog notice that queued work had stalled without an active turn for five minutes. The logged first exchange itself took about 26 seconds, so this is a separate queue-reliability signal and must not be represented as a live-steering failure.

Codex provides the interaction pattern: distinct pending steers and FIFO follow-ups, with pending steers applied after the next tool result and an interrupt shortcut to apply them immediately. RustCode's existing queue preview/edit support should remain intact.

## Design

### Submission behavior

1. When no interactive agent turn is active, Enter starts a turn as it does today.
2. During a regular interactive agent turn that is safe to steer, Enter submits the draft as a pending steer. The steer is appended to the active conversation after the current tool batch has finalized all completed, cancelled, and deferred results, and before the next provider request.
3. A draft may be sent as an ordinary queued follow-up instead. While a steerable turn is active, Tab toggles the current non-empty draft between `Steer` and `Queue` modes when no completion suggestion is available. When a completion suggestion is available, Tab keeps its current completion behavior. The composer shows the current mode and the key hint. A new draft defaults to `Steer` while the turn remains steerable.
4. In states that cannot accept a steer (tool confirmation, interactive question, non-regular or recovery turn, or no active model turn), Enter keeps the current FIFO queue behavior.
5. Esc with one or more pending steers interrupts the active turn and immediately places those steers, in their original order, at the head of the existing FIFO queue. Without pending steers, Esc keeps its current cancel behavior.

### Handoff and ordering

- Store pending steers separately from `pending_queue`; do not overload FIFO follow-ups or internal background wakeups.
- At the next tool-batch boundary, atomically take the pending steers, append them as user messages after every completed, cancelled, and deferred result in that batch, and clear the pending preview only when the turn has accepted them.
- If the turn finishes or is cancelled before another tool-result boundary, move unaccepted steers to the front of `pending_queue` before any later follow-up. The next orchestrated turn then applies the correction first.
- Give each accepted steer a single state transition from pending to active-history or queued. A cancellation/tool-result race must never lose or duplicate text.
- If several steers are pending at one boundary, preserve submission order and expose each message distinctly in the transcript.

### UI behavior

- Show `Pending steers` separately from `Queued follow-ups` above the composer.
- For steers, state `applies after next tool result` and show Esc as `interrupt and apply now` while the current turn can be interrupted.
- Keep the current FIFO count and latest prompts preview, plus `↑ edit last queued` behavior.
- Keep Tab completion whenever a completion suggestion is available. In steerable turns without an available suggestion, Tab toggles the current draft mode and the preview reflects `Steer` or `Queue`.

## Safety boundaries

- Only a regular interactive agent turn may accept a steer. Never inject steering text while a tool approval or question is pending, during a non-regular turn, or after a final response.
- Pending steers must be handed to the model only after the current tool batch has finalized its results, so tool call/result pairing and history ordering remain valid.
- Existing cancel handling and queued-prompt preservation remain in force.
- Slash commands keep their current dispatch behavior and are not converted into steers by this change.
- Do not change model policy, tool permissions, loop thresholds, or queue watchdog timeouts in this project.

## Acceptance criteria

- Submitting ordinary text during a steerable turn creates a separately visible pending steer and does not append it to the FIFO follow-up queue.
- At the next completed tool-batch boundary, pending steers enter model history in submission order, after all matching tool results and before the next provider request.
- If the current turn finishes before another result boundary, pending steers run before existing FIFO follow-ups.
- Esc interrupts an active turn and applies pending steers exactly once; Esc without a steer keeps existing behavior.
- A tool confirmation, pending question, non-regular turn, or no-active-turn state does not accept a steer.
- Tab toggles `Steer`/`Queue` mode only when the draft is non-empty and no completion suggestion is available; ordinary completion remains unchanged.
- UI previews distinguish pending steers and FIFO follow-ups, state when steers apply, and preserve existing queue editing.
- Deterministic tests cover multiple steers, order, end-of-turn handoff, interrupt races, unsteerable states, and completions.
- `cargo check --tests` and `cargo test` pass.

## Scope

Expected files include `src/app/state.rs`, `src/app/actions/enter.rs`, `src/app/actions.rs`, `src/app/composer.rs`, `src/app/runtime/input.rs`, `src/network/turn/tools.rs`, `src/network/turn/queue.rs`, `src/ui/render_snapshot.rs`, `src/ui/composer_render.rs`, and focused tests near those modules. Exact boundaries should follow existing turn/state ownership and be confirmed in the implementation plan.

This project does not fix destination/tool discovery, timestamp conversion, or the five-minute queue-stall watchdog. Those are separate observations from the same session.
