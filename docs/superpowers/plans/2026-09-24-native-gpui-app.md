# Native GPUI App Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a Cargo-built macOS GPUI app that runs RustCode's interactive agent in process while preserving the terminal executable.

**Architecture:** A public, UI-neutral controller in the existing `rustcode` library owns one workspace/session and delegates turns to the existing queue orchestrator. A new `rustcode-app` crate owns GPUI entities and a Tokio runtime, sending typed commands to the controller and rendering its session-scoped updates. GPUI Kit supplies native controls and chat presentation.

**Tech Stack:** Rust 2024, Tokio, existing RustCode library, `gpui-kit = "=0.6.6"` (its matched GPUI dependency set), macOS native windowing.

**Spec:** `docs/superpowers/specs/2026-09-24-native-gpui-app-design.md`

## Global Constraints

- Keep the original checkout on its original branch. All changes, commits, and integration occur in the isolated feature worktree.
- Keep `rustcode` and `rustcode-app` separately buildable with `cargo build --bin rustcode` and `cargo build -p rustcode-app`.
- The app uses the RustCode library directly: no ACP subprocess, Tauri, WebView, or separate GPUI version.
- First platform and deliverable: a macOS Cargo executable with a native window. No app bundle is required.
- Start empty; offer launch CWD, folder selection, and resumable sessions. One workspace/session is active at a time.
- Use the existing session store and effective workspace model configuration. Automatic tool confirmation is the default, within existing policy limits.
- Preserve FIFO prompts, steering, single-flight orchestration, persistence, cancellation, and background-task completion. Do not expose Ratatui snapshots as the app API.
- Run `cargo check --tests` and `cargo test` as required by `AGENTS.md`; build both binaries and smoke-test the app on macOS.

## File Structure

- `src/controller/mod.rs`: public command handle, controller worker, lifecycle and error types.
- `src/controller/snapshot.rs`: immutable session/project/model/history projection; no GPUI or Ratatui types.
- `src/controller/events.rs`: session-generation-tagged update projection from agent events.
- `src/app/actions/submit.rs`: shared plain-prompt enqueue/steer operation; TUI-only slash commands stay in `enter.rs`.
- `src/app/runtime/orchestration.rs`: call the shared observed queue launcher; retain terminal event loop.
- `src/app/session_controller.rs`, `src/config/session.rs`: reuse existing saved-session operations; expose only controller DTOs publicly.
- `crates/rustcode-app/Cargo.toml`, `src/main.rs`: native executable entry and GPUI Kit initialization.
- `crates/rustcode-app/src/backend.rs`: Tokio runtime, controller handle, and GPUI-facing update subscription.
- `crates/rustcode-app/src/view.rs`: app screen, Kit composer/transcript, commands and dialogs.
- `crates/rustcode-app/src/projection.rs`: map controller transcript and tool rows into Kit chat elements.

## Review Focus

- An empty or whitespace-only prompt leaves queue/history untouched; Task 1 tests this.
- Switching projects or sessions during a stream cannot put old text in the new chat; Task 4 tests generation filtering.
- Canceling a folder picker leaves the existing project/session intact; Task 6 tests this.
- A legacy session with no recorded project resumes in the selected or launch directory; Task 4 tests this.
- A provider failure, question, or policy-denied tool must leave the GUI responsive; Tasks 3 and 7 test these paths.

## Baseline Evidence

On the refreshed `origin/main` base with only documentation changes, `cargo check --tests` passed. The default parallel `cargo test` run failed twice at `daemon::lifecycle::tests::aborted_foreground_task_releases_socket_and_registration` (`daemon ownership is busy`); that test passed alone, and `cargo test --quiet -- --test-threads=1` passed all 1,675 tests. Treat a recurrence as an existing parallel-test issue to investigate separately, not as proof of a GUI regression.

---

### Task 1: Shared plain-prompt submission and queue launch

**Files:** Modify `src/app/actions/enter.rs`, `src/app/runtime/orchestration.rs`, `src/app/actions/mod.rs`, `src/network/turn/queue.rs`; create `src/app/actions/submit.rs`; test in `src/app/actions/tests.rs` and `src/network/turn/queue.rs`.

**Interfaces:** Produce `submit_plain_prompt(state: &mut AppState, text: String) -> SubmitOutcome` where outcome distinguishes empty, steered, and queued. Produce one `spawn_observed_orchestrator(...)` helper that claims the existing lease, calls `process_queue_orchestrator_with_ui_events`, and observes task death. TUI Enter and runtime queue drain consume these helpers.

```rust
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SubmitOutcome { Empty, Steered, Queued }
assert_eq!(submit_plain_prompt(&mut state, "  ".into()), SubmitOutcome::Empty);
assert!(state.pending_queue.is_empty());
assert_eq!(submit_plain_prompt(&mut state, "first".into()), SubmitOutcome::Queued);
assert_eq!(state.pending_queue, ["first"]);
```

- [ ] **Step 1: Write failing tests.** In `src/app/actions/tests.rs`, assert `submit_plain_prompt` ignores `"  "`, queues `"first"` then `"second"` in order, and steers only when `can_accept_steer()` is true. In queue tests, use the existing local provider/event fixture to assert an Enter-started turn emits `PromptStarted`, `TextDelta`, and a terminal event.
- [ ] **Step 2: Run the focused tests and observe failure.** Run `cargo test submit_plain_prompt` and the new Enter event test by name; expect the missing helper/event-path assertion to fail.
- [ ] **Step 3: Implement minimal extraction.** Move the non-slash portion of `handle_enter` (currently near line 1050) into `submit.rs`; keep autocomplete, slash dispatch, input history, and draft clearing in Enter. Replace the direct no-event spawn near line 1080 and the separate spawn near `runtime/orchestration.rs:321` with one eventful observed launcher. Preserve `claim_orchestrator`, cancellation token, `InteractivePolicy`, and the queue's own FIFO/lease guard.
- [ ] **Step 4: Run focused tests and existing queue tests.** Confirm both submission paths publish the same events and existing steering/lease tests still pass.
- [ ] **Step 5: Commit.** `git add src/app src/network/turn/queue.rs && git commit -m "refactor: share interactive prompt submission"`.

### Task 2: Public UI-neutral controller contract

**Files:** Create `src/controller/mod.rs`, `src/controller/snapshot.rs`, `src/controller/events.rs`; modify `src/lib.rs`; test inside the new modules.

**Interfaces:** Export `rustcode::controller::{Command, ControllerEvent, ControllerHandle, ControllerSnapshot, ControllerError, ModelChoice, SessionChoice, TranscriptItem}`. `Command` includes `StartNew(PathBuf)`, `Resume { session_id, workspace }`, `Submit(String)`, `Cancel`, `SelectModel(String)`, `AnswerQuestion(String)`, `Approval(ApprovalChoice)`, and `Shutdown`. Each `ControllerEvent` carries `generation: u64` and either a full snapshot, a turn update, or an error. `ControllerSnapshot` contains workspace, session ID, model choices/selection, history projection, live response, queue/turn state, pending question, and pending approval.

```rust
pub enum Command {
    ListSessions,
    StartNew(std::path::PathBuf),
    Resume { session_id: String, workspace: std::path::PathBuf },
    Submit(String),
    Cancel,
    SelectModel(String),
    AnswerQuestion(String),
    Approval(ApprovalChoice),
    Shutdown,
}
pub enum ApprovalChoice { Approve, Deny }
pub enum ControllerError {
    NoActiveSession,
    InvalidWorkspace(String),
    Session(String),
    Model(String),
    Provider(String),
    ChannelClosed,
}
pub struct ModelChoice { pub id: String, pub label: String }
pub struct SessionChoice {
    pub id: String, pub title: String, pub when: String, pub message_count: usize,
}
pub struct TranscriptItem {
    pub role: String, pub content: String, pub tool_name: Option<String>,
}
pub struct QuestionPrompt {
    pub text: String, pub options: Vec<String>, pub multiple: bool,
}
pub struct ApprovalPrompt { pub tool_name: String, pub description: String }
pub struct ControllerSnapshot {
    pub generation: u64,
    pub workspace: Option<std::path::PathBuf>,
    pub session_id: Option<String>,
    pub sessions: Vec<SessionChoice>,
    pub models: Vec<ModelChoice>,
    pub selected_model: Option<String>,
    pub transcript: Vec<TranscriptItem>,
    pub live_response: String,
    pub queued_count: usize,
    pub turn_active: bool,
    pub pending_question: Option<QuestionPrompt>,
    pub pending_approval: Option<ApprovalPrompt>,
}
pub enum TurnUpdate {
    PromptStarted(String), TextDelta(String),
    ToolStarted { id: String, name: String },
    ToolFinished { id: String, content: String },
    TurnFinished, Cancelled,
}
pub struct ControllerEvent {
    pub generation: u64,
    pub update: ControllerUpdate,
}
pub enum ControllerUpdate {
    Snapshot(ControllerSnapshot),
    Turn(TurnUpdate),
    Error(ControllerError),
}
pub fn accepts_generation(current: u64, event: &ControllerEvent) -> bool {
    event.generation == current
}
```

- [ ] **Step 1: Write failing projection tests.** Construct an `AppState` with a temporary workspace and known history, then assert `ControllerSnapshot::from_state(7, &state)` preserves session ID, workspace, model, transcript order, queue count and question while excluding terminal layout fields. Assert an update tagged generation 6 is rejected by a generation-7 view filter.
- [ ] **Step 2: Run `cargo test controller::` and observe missing-type failures.**
- [ ] **Step 3: Add DTOs and pure projections.** Implement `from_state` using `AppState.history`, `current_response`, `status`, `config.models`, and pending question/approval. Convert private `AgentUiEvent` into public owned event records without exposing `ToolCall`, `ToolResult`, `AppState`, Ratatui, or GPUI. Put the public module declaration in `src/lib.rs`.
- [ ] **Step 4: Run `cargo test controller::` and `cargo check --tests`.**
- [ ] **Step 5: Commit.** `git add src/controller src/lib.rs && git commit -m "feat: define native controller contract"`.

### Task 3: Controller turn worker and event delivery

**Files:** Modify `src/controller/mod.rs`, `src/controller/events.rs`, `src/network/ui_adapter.rs`; test in `src/controller/mod.rs` and `src/network/ui_adapter.rs`.

**Interfaces:** `InteractiveController::spawn(tokio_handle: &tokio::runtime::Handle, launch_dir: PathBuf) -> (ControllerHandle, tokio::sync::mpsc::UnboundedReceiver<ControllerEvent>)`. `ControllerHandle::send(Command) -> Result<(), ControllerError>` is thread-safe. The worker starts with no `AppState`, creates one only for StartNew/Resume, owns client/cancellation, and forwards eventful queue updates tagged with its active generation.

```rust
let (handle, mut updates) = InteractiveController::spawn(&runtime.handle(), launch_dir);
assert!(matches!(updates.recv().await.unwrap().update,
    ControllerUpdate::Snapshot(snapshot) if snapshot.session_id.is_none()));
handle.send(Command::Submit("hello".into())).unwrap();
assert!(matches!(updates.recv().await.unwrap().update,
    ControllerUpdate::Error(ControllerError::NoActiveSession)));
```

- [ ] **Step 1: Write failing worker tests.** Assert spawning emits an empty start snapshot without creating a session; submitting before selection returns a typed error; starting a workspace then submitting against a local mock provider emits ordered prompt/text/finish updates and persisted history; provider failure emits an error and allows another command.
- [ ] **Step 2: Run the focused `cargo test controller::` tests and observe failure.**
- [ ] **Step 3: Implement the command loop.** Use `tokio::mpsc` for commands and updates. On StartNew construct `AppState::new_with_workspace_session(&workspace, None)`, set explicit `workspace_root` and `task_working_directory`, and enable `auto_confirm` for this frontend before sending a full snapshot. Spawn the queue with the Task 1 helper and forward `AgentUiEvent` updates through the Task 2 projection. Report production provider errors via the controller even though `AgentUiEvent::Error` is currently test-only; do not depend on that test-only variant. Observe JoinHandle failures and return a usable idle/error snapshot. Never lock state across a provider await.
- [ ] **Step 4: Run focused controller and event adapter tests.** Check ordered updates and that a failed turn does not wedge the worker.
- [ ] **Step 5: Commit.** `git add src/controller src/network/ui_adapter.rs && git commit -m "feat: run interactive turns through controller"`.

### Task 4: Sessions, workspace, model, and cancellation lifecycle

**Files:** Modify `src/controller/mod.rs`, `src/controller/snapshot.rs`, `src/app/session_controller.rs` only if needed; test in `src/controller/mod.rs`.

**Interfaces:** `Command::ListSessions`, `StartNew`, `Resume`, `SelectModel`, `Cancel` and `Shutdown` use the Task 2 event/snapshot contract. Session choices contain ID, title, date and message count; model choices come from the selected workspace's effective `AppConfig.models`.

```rust
handle.send(Command::Resume {
    session_id: saved_id.clone(),
    workspace: chosen_dir.clone(),
})?;
let snapshot = next_snapshot(&mut updates).await;
assert_eq!(snapshot.workspace, Some(chosen_dir));
assert_eq!(snapshot.session_id.as_deref(), Some(saved_id.as_str()));
assert!(snapshot.generation > old_generation);
```

- [ ] **Step 1: Write failing lifecycle tests.** Save a temporary legacy session, list it, resume it in an explicit workspace, and submit a continuation. Switch workspace/session while a mocked stream is active: assert cancellation, generation increment, and rejection of old-generation updates. Assert invalid directories/model names return errors without replacing the active session; cancellation preserves queued prompts under existing queue semantics; a background-task completion reaches the active session; shutdown flushes history.
- [ ] **Step 2: Run `cargo test controller::lifecycle` and observe failures.**
- [ ] **Step 3: Implement lifecycle operations.** Validate/canonicalize directories, cancel and await the old turn boundary before replacing state, increment generation before publishing a new snapshot, use `SessionController::resume` and existing session listing, and load the configured model profile rather than scanning files. Preserve the selected or launch directory for old sessions without project metadata. Apply explicit workspace paths to tool execution/context paths; fix any remaining process-CWD assumptions found by these tests without changing global CWD.
- [ ] **Step 4: Run focused lifecycle, workspace, and session tests.**
- [ ] **Step 5: Commit.** `git add src/controller src/app src/config src/tools src/context.rs && git commit -m "feat: manage native workspace and sessions"` (stage only files actually changed).

### Task 5: Native executable and GPUI Kit runtime

**Files:** Create `crates/rustcode-app/Cargo.toml`, `crates/rustcode-app/src/main.rs`, `crates/rustcode-app/src/backend.rs`, `crates/rustcode-app/src/view.rs`; modify root `Cargo.toml` and `Cargo.lock`; test in the app crate.

**Interfaces:** `NativeBackend::new(launch_dir) -> Result<Self, String>` owns `tokio::runtime::Runtime`, `ControllerHandle`, and update receiver. `AppView` owns the GPUI foreground state and dispatches controller commands; no Tokio task updates an entity directly.

```toml
[package]
name = "rustcode-app"
version = "0.55.10"
edition = "2024"

[dependencies]
rustcode = { path = "../.." }
gpui-kit = "=0.6.6"
tokio = { version = "1.52.3", features = ["rt-multi-thread", "sync"] }
```

- [ ] **Step 1: Write an app crate smoke/unit test.** Assert the backend begins with an empty snapshot and handles StartNew without a terminal. Run `cargo test -p rustcode-app` and observe the missing crate failure.
- [ ] **Step 2: Add the crate and window.** Add `crates/rustcode-app` to workspace members and pin `gpui-kit = "=0.6.6"` in its manifest. Call `gpui_kit::application().run(...)`, `gpui_kit::init(cx)`, and `gpui_kit::open_window(...)` following the 0.6.6 example. Create a Tokio multithread runtime before entering GPUI and keep it alive for the window lifetime. Render an empty start view containing the launch directory and actions to begin or choose a project.
- [ ] **Step 3: Bridge updates.** In a GPUI foreground task, receive controller updates, discard events whose generation is older than the active snapshot, and update the view entity. Keep channel closure and controller errors visible in the window.
- [ ] **Step 4: Run `cargo test -p rustcode-app` and `cargo build -p rustcode-app`.** Fix dependency/API mismatches against the pinned Kit docs; do not add an independent `gpui` crate.
- [ ] **Step 5: Commit.** `git add Cargo.toml Cargo.lock crates/rustcode-app && git commit -m "feat: add native GPUI executable"`.

### Task 6: Project and session selection

**Files:** Modify `crates/rustcode-app/src/view.rs`, `src/backend.rs`; test in `crates/rustcode-app/src/backend.rs` and view state tests.

**Interfaces:** StartNew and Resume dispatch typed commands. The folder action uses GPUI's `App::prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, .. })`; the session picker uses controller-provided SessionChoice records.

```rust
let picked = cx.prompt_for_paths(PathPromptOptions {
    files: false,
    directories: true,
    multiple: false,
    prompt: Some("Choose workspace".into()),
});
// Await `picked` in a GPUI foreground task; only Some(path) sends StartNew(path).
```

- [ ] **Step 1: Write failing selection tests.** Assert folder-dialog `None` leaves the current project/session unchanged, a chosen directory becomes the command workspace, session selection passes its ID and selected/launch directory, and invalid project errors stay visible on the start screen.
- [ ] **Step 2: Run `cargo test -p rustcode-app` and observe failures.**
- [ ] **Step 3: Implement Kit controls and native folder dialog.** Use a GPUI foreground task to await the dialog receiver, then send StartNew/Resume. Add a session list populated by `ListSessions`, with empty-state text and a direct new-chat action. Keep the launch CWD as the initial suggested project; do not automatically start a session.
- [ ] **Step 4: Run app tests and build the package.**
- [ ] **Step 5: Commit.** `git add crates/rustcode-app && git commit -m "feat: choose projects and resume sessions"`.

### Task 7: Chat, models, stop, questions, and approvals

**Files:** Modify `crates/rustcode-app/src/view.rs`, `src/projection.rs`, `src/backend.rs`; test in `crates/rustcode-app/src/projection.rs` and view state tests.

**Interfaces:** Project controller TranscriptItem/turn updates to GPUI Kit message, Markdown and tool rows. Composer submits `Command::Submit`, stop sends `Cancel`, model picker sends `SelectModel`, question dialog sends `AnswerQuestion`, and any nonautomatic approval sends `Approval`.

```rust
assert_eq!(project_rows(&[user("Hi"), assistant("Hello")], "!"),
    vec![user_row("Hi"), assistant_row("Hello!")]);
assert!(!can_submit(" \n"));
assert!(can_submit("Explain this"));
```

- [ ] **Step 1: Write failing UI-model tests.** Given ordered history plus live deltas, assert user/assistant/tool rows remain in order without duplicate final text. Test Send disabled for blank input; Stop available during a turn; question options/freeform map to the correct answer; denied approval remains explicit; a provider error leaves composer usable.
- [ ] **Step 2: Run `cargo test -p rustcode-app` and observe failures.**
- [ ] **Step 3: Implement the view.** Use Kit Textarea for multiline input, its message/scroller and Markdown-capable components where the pinned API fits, plus Kit buttons/dialogs/lists for controls. Derive visible state only from controller snapshots/updates. Auto-approve by default through RustCode's existing confirmation setting; show a dialog when policy still requires a decision. Coalesce rapid text updates for render cadence without reordering them.
- [ ] **Step 4: Run app tests and build the package.**
- [ ] **Step 5: Commit.** `git add crates/rustcode-app && git commit -m "feat: add native agent chat controls"`.

### Task 8: Regression, native smoke test, and integration

**Files:** Update only files needed to address failures; add a short run/build note to `README.md` if no equivalent exists.

**Interfaces:** Both executables remain separately buildable; the app uses the same session store and effective config as TUI.

- [ ] **Step 1: Run repository checks.** `cargo check --tests`, `cargo test`, `cargo build --bin rustcode`, and `cargo build -p rustcode-app`; record exit statuses and fix actual regressions.
- [ ] **Step 2: Smoke-test on macOS.** Launch `target/debug/rustcode-app`; verify empty start, current/selected folder, new session, model picker, prompt stream/tool activity, stop, saved-session continuation, and a question against a local or configured provider. Record any provider-dependent cases that cannot be exercised and the exact reason.
- [ ] **Step 3: Review the branch.** Inspect `git diff origin/main...HEAD`, check generated/secret files are absent, and obtain a code review before integration. Address findings and re-run affected checks.
- [ ] **Step 4: Commit remaining fixes/docs.** Commit only scoped corrections. Push the feature branch, open a PR to `main`, and merge from the isolated worktree per `AGENTS.md`; leave the original checkout branch untouched.
