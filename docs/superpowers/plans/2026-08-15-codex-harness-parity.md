# Codex-Like Agent Harness and TUI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Incrementally make rustcode’s local interactive CLI behave and look like Codex while preserving the existing provider, tool, policy, headless, ACP, and turn-engine behavior.

**Architecture:** Keep `src/network/turn_engine.rs` as the initial behavioral authority and place a typed adapter between it and a new interactive application runtime. Extract terminal ownership, events, redraw scheduling, application commands, composer, transcript, status, overlays, sessions, and subagent navigation into independently testable components.

**Tech Stack:** Rust, Tokio, crossterm, Ratatui, existing `InlineTerminal`, existing `AppState`, existing network/tool modules, Ratatui `TestBackend`, and the standard library wherever possible.

**Spec:** `docs/superpowers/specs/2026-08-15-codex-harness-parity-design.md`

## Global Constraints

- Start from the updated `main` branch and work only on `feature/codex-harness-parity-plan` until the migration is intentionally split into implementation branches.
- Preserve headless CLI and ACP behavior; those paths must not import interactive-only TUI types.
- Preserve existing provider profiles, tool names, config files, session history format, and public CLI flags.
- Do not copy Codex source code; reproduce the observed behavior with rustcode-native interfaces.
- Prefer existing crossterm, Ratatui, Tokio, and standard-library functionality; add no dependency unless the existing stack cannot provide the required behavior.
- Keep the current turn state machine and tool semantics unchanged until the adapter phase explicitly proves equivalent behavior.
- Every task ends with a focused test run, `cargo check --tests`, and an intentional commit.
- Do not perform unrelated refactors, formatting sweeps, or naming changes.
- Use `apply_patch` for source edits.

---

### Task 1: Establish deterministic UI acceptance fixtures

**Files:**
- Modify: `src/ui/tests.rs`
- Test: `src/ui/tests.rs`

**Interfaces:**
- Add a private test helper `render_state_to_text(state: &mut AppState, width: u16, height: u16) -> String` that renders through the existing `InlineTerminal<TestBackend>` and returns newline-separated terminal rows.
- Keep all production rendering entrypoints unchanged: `ui::render`, `ui::render_with_transcript`, and `ui::desired_height` remain the behavior under test.

- [ ] **Step 1: Add the shared terminal-buffer helper**

  Reuse the existing `InlineTerminal` and `TestBackend` setup already repeated throughout `src/ui/tests.rs`. Preserve trailing spaces only when a test needs exact geometry; otherwise trim rows for readable assertions.

- [ ] **Step 2: Add baseline acceptance tests**

  Cover these current states:

  - empty session with welcome banner and composer;
  - committed user and assistant messages;
  - active streaming response with working/status row;
  - live tool activity and committed tool result;
  - tool confirmation modal above the composer;
  - command picker below the composer;
  - narrow terminal rendering without panic or lost footer;
  - transcript replay after `TranscriptCursor::reset()`.

- [ ] **Step 3: Run the focused UI tests**

  Run: `cargo test ui::tests`

  Expected: PASS with the existing UI behavior captured as executable acceptance criteria.

- [ ] **Step 4: Run the repository test gate**

  Run: `cargo check --tests`

- [ ] **Step 5: Commit the baseline contract**

  ```bash
  git add src/ui/tests.rs
  git commit -m "test: establish TUI acceptance fixtures"
  ```

**Done when:** The current UI has deterministic tests for the states later migration tasks must preserve.

### Task 2: Extract terminal lifecycle ownership

**Files:**
- Create: `src/ui/terminal_runtime.rs`
- Modify: `src/main.rs`
- Modify: `src/ui/mod.rs`
- Test: `src/ui/terminal_runtime.rs` or `src/ui/tests.rs`

**Interfaces:**
- Add `pub(crate) struct TerminalRuntime` owning the interactive `InlineTerminal<CrosstermBackend<io::Stdout>>` and raw-mode lifecycle.
- Add `pub(crate) fn TerminalRuntime::start() -> Result<Self, Box<dyn std::error::Error>>`.
- Add `pub(crate) fn TerminalRuntime::restore(&mut self) -> io::Result<()>`.
- Add `pub(crate) fn TerminalRuntime::terminal(&mut self) -> &mut InlineTerminal<CrosstermBackend<io::Stdout>>`.
- Add `pub(crate) async fn TerminalRuntime::with_restored<F, Fut, T>(&mut self, f: F) -> T` for external interactive programs, pausing input and restoring terminal modes around `f`.

- [ ] **Step 1: Move terminal setup and cleanup into `TerminalRuntime`**

  Move only raw mode, bracketed paste/focus setup, cursor style, title initialization, and cleanup currently owned by `main.rs`. Keep rendering and event dispatch in `main.rs` for this task.

- [ ] **Step 2: Make restoration idempotent**

  `restore` must tolerate repeated calls and must disable bracketed paste/focus, show the cursor, disable raw mode, and leave the terminal at a stable cursor position.

- [ ] **Step 3: Route the existing main loop through the runtime**

  Replace direct terminal setup in `main.rs` with `TerminalRuntime::start()` and replace shutdown cleanup with `restore()`.

- [ ] **Step 4: Add lifecycle tests that do not require a real TTY**

  Test the idempotent state transition separately from crossterm escape output. Continue using existing `InlineTerminal<TestBackend>` tests for rendering.

- [ ] **Step 5: Run and commit**

  Run: `cargo test ui::tests && cargo check --tests`

  ```bash
  git add src/main.rs src/ui/mod.rs src/ui/terminal_runtime.rs
  git commit -m "refactor: isolate terminal lifecycle"
  ```

**Done when:** Only `TerminalRuntime` owns interactive terminal setup, restoration, and external-program handoff.

### Task 3: Introduce typed terminal events

**Files:**
- Create: `src/ui/events.rs`
- Modify: `src/ui/mod.rs`
- Modify: `src/main.rs`
- Modify: `Cargo.toml` only if the existing crossterm dependency lacks `event-stream`
- Test: `src/ui/events.rs`

**Interfaces:**
- Add:

  ```rust
  pub(crate) enum TuiEvent {
      Key(crossterm::event::KeyEvent),
      Paste(String),
      Resize { width: u16, height: u16 },
      FocusGained,
      FocusLost,
      Draw,
  }
  ```

- Add `pub(crate) struct TuiEventStream` with `new`, `pause`, `resume`, and an async `next` method returning `io::Result<Option<TuiEvent>>`.
- Normalize key release events, paste payloads, focus events, and resize dimensions in this layer.

- [ ] **Step 1: Write event normalization tests**

  Test that crossterm key, paste, focus, and resize events map to exactly one `TuiEvent`, and that key-release events are ignored.

- [ ] **Step 2: Implement the event source**

  Use crossterm’s existing event-stream support when available. Ensure only one reader owns stdin and that `pause` drops or suspends the underlying reader before external programs run.

- [ ] **Step 3: Replace direct event conversion in `main.rs`**

  Keep the current dispatch behavior, but make it consume `TuiEvent`. Do not change key meanings in this task.

- [ ] **Step 4: Verify ordering and shutdown behavior**

  Run: `cargo test ui::events && cargo check --tests`

- [ ] **Step 5: Commit**

  ```bash
  git add Cargo.toml src/main.rs src/ui/mod.rs src/ui/events.rs
  git commit -m "refactor: add typed TUI events"
  ```

**Done when:** Terminal input is represented by typed events and no application code needs to inspect raw crossterm `Event` variants.

### Task 4: Add coalesced frame scheduling

**Files:**
- Create: `src/ui/frame_requester.rs`
- Modify: `src/ui/mod.rs`
- Modify: `src/main.rs`
- Test: `src/ui/frame_requester.rs`

**Interfaces:**
- Add cloneable `pub(crate) struct FrameRequester` with `schedule_frame()` and `schedule_frame_in(Duration)`.
- Add `pub(crate) struct FrameStream` or an equivalent receiver owned by the application loop.
- Frame requests must be coalesced and rate-limited before producing `TuiEvent::Draw`.

- [ ] **Step 1: Write scheduler tests**

  Test that multiple immediate requests produce one draw notification, delayed requests do not fire early, and the scheduler stops when all senders are dropped.

- [ ] **Step 2: Implement the scheduler with Tokio channels**

  Use one request channel and one draw notification channel. Do not render from the scheduler task.

- [ ] **Step 3: Replace the streaming-only redraw timer**

  Keep the existing 60Hz behavior as the initial limit, but trigger it through `FrameRequester` instead of a hard-coded `since_last_draw` branch.

- [ ] **Step 4: Make existing background wakeups request frames**

  Replace direct redraw flag mutation at the integration points that already call `AppState::request_redraw`; retain the flag temporarily for compatibility until Task 6.

- [ ] **Step 5: Run and commit**

  Run: `cargo test ui::frame_requester && cargo test ui::tests && cargo check --tests`

  ```bash
  git add src/main.rs src/ui/mod.rs src/ui/frame_requester.rs
  git commit -m "refactor: coalesce TUI frame requests"
  ```

**Done when:** Streaming and background work request redraws through one rate-limited mechanism.

### Task 5: Add the application event and command bus

**Files:**
- Create: `src/app/events.rs`
- Modify: `src/app/mod.rs`
- Modify: `src/main.rs`
- Test: `src/app/events.rs`

**Interfaces:**
- Add `pub(crate) enum AppEvent` covering `Tui(TuiEvent)`, `SubmitPrompt(String)`, `CancelActiveTurn`, `ApprovalDecision`, `OpenOverlay`, `CloseOverlay`, `NewSession`, `ResumeSession`, `ForkSession`, `SelectSubagent`, `RequestDraw`, and `Exit`.
- Add `pub(crate) enum ApprovalDecision { Approve, Deny, ApproveAll, Custom(String) }`.
- Add `pub(crate) enum AppCommand` for commands sent from the controller to the turn/session layers.
- Add `AppEventSender` wrapping an unbounded Tokio sender so widgets do not receive raw channel types.

- [ ] **Step 1: Write event routing tests**

  Test that approval, cancellation, submit, and exit events preserve their payloads and that event construction does not depend on Ratatui.

- [ ] **Step 2: Implement the event types and sender**

  Keep the types under `app` and exclude them from `raw_cli` and `acp` modules.

- [ ] **Step 3: Route current keyboard outcomes through `AppEvent`**

  The behavior remains in `main.rs`; only the communication mechanism changes.

- [ ] **Step 4: Run and commit**

  Run: `cargo test app::events && cargo check --tests`

  ```bash
  git add src/app/mod.rs src/app/events.rs src/main.rs
  git commit -m "refactor: add application event bus"
  ```

**Done when:** UI actions have typed application-level messages and no new widget code needs direct access to the main loop.

### Task 6: Extract the interactive application controller

**Files:**
- Create: `src/app/runtime.rs`
- Modify: `src/main.rs`
- Modify: `src/app/mod.rs`
- Test: `src/app/runtime.rs`

**Interfaces:**
- Add `pub(crate) struct AppRuntime` owning `TerminalRuntime`, `AppState`, `TuiEventStream`, `FrameRequester`, current cancellation token, HTTP client, and event channels.
- Add `pub(crate) async fn AppRuntime::run(self) -> Result<ExitSummary, Box<dyn Error>>`.
- Add `pub(crate) async fn AppRuntime::handle_event(&mut self, AppEvent) -> Result<AppRunControl, AppError>`.
- Add `pub(crate) enum AppRunControl { Continue, Exit(ExitSummary) }`.

- [ ] **Step 1: Move startup-owned fields from `main` into `AppRuntime`**

  Move only ownership first; preserve the current rendering and input behavior through delegated methods.

- [ ] **Step 2: Implement the multiplexed loop**

  Merge application events, TUI events, frame notifications, and active background work with `tokio::select!` or the smallest equivalent compatible with the current runtime.

- [ ] **Step 3: Add controller tests**

  Feed synthetic `AppEvent` values into a runtime with a test backend and verify submit, cancel, redraw, and exit state transitions.

- [ ] **Step 4: Keep `main.rs` as a thin CLI adapter**

  `main` should parse CLI modes and invoke either raw/ACP paths or `AppRuntime::run`.

- [ ] **Step 5: Run and commit**

  Run: `cargo test app::runtime && cargo check --tests && cargo test`

  ```bash
  git add src/main.rs src/app/mod.rs src/app/runtime.rs
  git commit -m "refactor: extract interactive app runtime"
  ```

**Done when:** `main.rs` no longer owns the interactive event loop or application state lifecycle.

### Task 7: Decompose interaction state without changing behavior

**Files:**
- Create: `src/app/composer.rs`
- Create: `src/app/transcript.rs`
- Create: `src/app/status.rs`
- Create: `src/app/overlays.rs`
- Modify: `src/app/state.rs`
- Modify: `src/ui/mod.rs`
- Modify: `src/main.rs`
- Test: the new modules and existing `src/ui/tests.rs`

**Interfaces:**
- Add `ComposerState` for input buffer, cursor, input history, pending queue editing, and suggestions.
- Add `TranscriptState` for committed-history cursor, live response, scroll position, and resize replay state.
- Add `StatusState` for `AppStatus`, response timing, stream metrics, active tools, and terminal title data.
- Add `OverlayState` for picker/modal visibility and selected indices.
- Keep `AppState` as a compatibility facade exposing the existing fields/methods until all callers migrate.

- [ ] **Step 1: Move one responsibility at a time behind accessors**

  Start with composer state, then transcript, status, and overlays. Do not delete existing fields until all current call sites use the new owner.

- [ ] **Step 2: Preserve serialization and public behavior**

  These state objects are runtime-only; `ChatMessage` and config/session serialization remain unchanged.

- [ ] **Step 3: Move focused tests with each state owner**

  Keep cursor, history recall, scroll, modal, and status tests next to the extracted owner while retaining integration rendering tests in `src/ui/tests.rs`.

- [ ] **Step 4: Run and commit**

  Run: `cargo test app::state && cargo test ui::tests && cargo check --tests`

  ```bash
  git add src/app src/main.rs src/ui/mod.rs
  git commit -m "refactor: split interactive application state"
  ```

**Done when:** The UI can evolve by component without expanding the shared state struct.

### Task 8: Introduce transcript/history cells and stable streaming reflow

**Files:**
- Create: `src/ui/transcript.rs`
- Modify: `src/ui/history_cell.rs`
- Modify: `src/ui/scrollback.rs`
- Modify: `src/ui/mod.rs`
- Modify: `src/app/transcript.rs`
- Test: `src/ui/tests.rs`, `src/ui/transcript.rs`

**Interfaces:**
- Add `pub(crate) enum HistoryCell { User, Assistant, Tool, System, Error, Plan }` with owned render data.
- Add `pub(crate) struct TranscriptModel` containing committed cells, one mutable live cell, and replay cursor state.
- Add `pub(crate) fn TranscriptModel::apply(AgentUiEvent)` and `pub(crate) fn TranscriptModel::render(width, height)`.
- Keep `ChatMessage` as the persistence format and convert it to cells at the UI boundary.

- [ ] **Step 1: Write tests for cell conversion and stream consolidation**

  Verify that committed history is rendered once, only the incomplete stream suffix remains mutable, completed code fences/tables remain intact, and a resize replays the canonical transcript.

- [ ] **Step 2: Implement the model using existing markdown/tool renderers**

  Reuse `src/ui/markdown.rs`, `src/ui/tool_result.rs`, and existing `TranscriptCursor` logic rather than introducing a second markdown parser.

- [ ] **Step 3: Route `render_with_transcript` through `TranscriptModel`**

  Preserve current visual output in the acceptance fixtures from Task 1.

- [ ] **Step 4: Run and commit**

  Run: `cargo test ui::tests && cargo check --tests`

  ```bash
  git add src/ui src/app/transcript.rs
  git commit -m "refactor: model transcript cells and stream reflow"
  ```

**Done when:** Streaming and resize behavior are driven by a canonical transcript model rather than ad hoc scrollback mutation.

### Task 9: Extract the Codex-like composer and keymap layer

**Files:**
- Create: `src/ui/composer.rs`
- Create: `src/ui/keymap.rs`
- Modify: `src/app/composer.rs`
- Modify: `src/ui/mod.rs`
- Modify: `src/main.rs`
- Test: `src/ui/composer.rs`, `src/ui/keymap.rs`, `src/ui/tests.rs`

**Interfaces:**
- Add `pub(crate) struct Composer` with `handle_key`, `handle_paste`, `submit`, `recall_previous`, `recall_next`, and `render` methods.
- Add `pub(crate) struct KeyMap` with explicit bindings for navigation, submit, cancel, picker, history, and queue editing.
- Add terminal-specific fallback selection for bindings that are unreliable in tmux, VS Code, Warp, and Apple Terminal.
- Composer actions emit `AppEvent`; they do not start a network request directly.

- [ ] **Step 1: Move input editing tests**

  Cover Unicode cursor movement, multiline input, paste normalization, slash suggestions, input history, queued-prompt restore, and Ctrl+C behavior.

- [ ] **Step 2: Implement the keymap table**

  Replace the large direct key match in `main.rs` incrementally. Preserve every current binding until a deliberate Codex-style binding change has an acceptance test.

- [ ] **Step 3: Render a bottom pane with stable key hints**

  Reuse current footer/status formatting while separating composer geometry from transcript geometry.

- [ ] **Step 4: Run and commit**

  Run: `cargo test ui::composer ui::keymap ui::tests && cargo check --tests`

  ```bash
  git add src/main.rs src/app/composer.rs src/ui/composer.rs src/ui/keymap.rs src/ui/mod.rs
  git commit -m "refactor: extract composer and keymaps"
  ```

**Done when:** Keyboard behavior is defined by tested keymaps and the composer can be reused by overlays and session pickers.

### Task 10: Add a typed agent-to-UI adapter

**Files:**
- Create: `src/network/ui_adapter.rs`
- Modify: `src/network/events.rs`
- Modify: `src/network/turn_engine.rs`
- Modify: `src/network/mod.rs`
- Modify: `src/app/runtime.rs`
- Test: `src/network/ui_adapter.rs`

**Interfaces:**
- Add:

  ```rust
  pub(crate) enum AgentUiEvent {
      PromptStarted { prompt: String },
      TextDelta { text: String },
      ToolStarted { name: String, id: String },
      ApprovalRequested { calls: Vec<ToolCall> },
      ToolFinished { id: String, result: ToolResult },
      TurnRecovered { message: String },
      TurnFinished { content: String, completed: bool },
      Cancelled { completed_tool_ids: Vec<String> },
      Error { message: String, retryable: bool },
  }
  ```

- Add `AgentUiEventSender` and `AgentUiEventReceiver` using Tokio channels.
- The adapter must expose the existing `run_agent_turn` result unchanged while publishing UI events.

- [ ] **Step 1: Write mapping tests**

  Test text deltas, tool calls, approval, denial, retry, completion, cancellation, and error mapping from the existing `AgentEvent`/`TurnContext` types.

- [ ] **Step 2: Publish events at existing lifecycle boundaries**

  Add publication beside current state/history updates; do not remove those updates yet.

- [ ] **Step 3: Consume events in `AppRuntime` and `TranscriptModel`**

  The UI should update from the event stream and request frames through `FrameRequester`.

- [ ] **Step 4: Prove headless and ACP isolation**

  Run existing raw/ACP tests and ensure the adapter is only constructed by the interactive runtime.

- [ ] **Step 5: Run and commit**

  Run: `cargo test network::ui_adapter && cargo check --tests && cargo test`

  ```bash
  git add src/network src/app/runtime.rs
  git commit -m "refactor: expose typed agent UI events"
  ```

**Done when:** The interactive UI can observe the turn lifecycle without reading provider responses or mutating network state directly.

### Task 11: Rebuild approvals, questions, and tool surfaces around events

**Files:**
- Modify: `src/ui/modals.rs`
- Modify: `src/ui/tool_result.rs`
- Modify: `src/ui/composer.rs`
- Modify: `src/app/events.rs`
- Modify: `src/app/overlays.rs`
- Modify: `src/network/ui_adapter.rs`
- Test: `src/ui/tests.rs`, `src/network/ui_adapter.rs`

**Interfaces:**
- Approval UI consumes `AgentUiEvent::ApprovalRequested` and emits `AppEvent::ApprovalDecision`.
- Question UI emits a typed answer without writing directly to `AppState`.
- Tool result rendering consumes immutable `ToolResult`/history-cell data.

- [x] **Step 1: Add event-driven approval tests**

  Verify approve, deny, approve-all, custom answer, Esc cancellation, and short-terminal layouts.

- [x] **Step 2: Move modal selection state behind `OverlayState`**

  Preserve the current modal copy and geometry first; change styling only after event ownership is correct.

- [x] **Step 3: Connect decisions to the existing `TurnPolicy`**

  Ensure a denied approval produces the same persisted tool result and next-turn behavior as today.

- [x] **Step 4: Run and commit**

  Run: `cargo test ui::tests network::ui_adapter && cargo check --tests`

  ```bash
  git add src/ui src/app/events.rs src/app/overlays.rs src/network/ui_adapter.rs
  git commit -m "refactor: route tool approvals through TUI events"
  ```

**Done when:** Approval and question interactions are UI components with typed responses and no direct network/UI coupling.

### Task 12: Extract session lifecycle and Codex-like resume/fork flows

**Files:**
- Create: `src/app/session_controller.rs`
- Modify: `src/config.rs`
- Modify: `src/app/runtime.rs`
- Modify: `src/ui/modals.rs`
- Modify: `src/app/events.rs`
- Test: `src/app/session_controller.rs`, `src/config.rs`, `src/ui/tests.rs`

**Interfaces:**
- Add `pub(crate) struct SessionController` with `start_fresh`, `resume`, `fork`, `clear`, `archive`, `delete`, and `active_session` methods.
- Keep `SessionId` represented as the existing persisted `String` until a typed identifier is proven compatible with all config/history callers.
- Session actions return `Result<SessionTransition, SessionError>` and emit a redraw/session event after state changes.

- [x] **Step 1: Write persistence tests**

  Cover new session creation, resumable-session filtering, title persistence, queued history flush before switching sessions, resume, fork, and delete confirmation.

- [x] **Step 2: Move session mutations out of `main.rs`**

  Reuse `src/config.rs`’s atomic/debounced history writer and preserve `history.json`, `title.txt`, sandbox, artifacts, and image-cache paths.

- [x] **Step 3: Implement picker events and rendering**

  Use the existing modal renderer and make selection produce `AppEvent` rather than mutating picker fields from the main loop.

- [x] **Step 4: Run and commit**

  Run: `cargo test config::tests app::session_controller ui::tests && cargo check --tests`

  ```bash
  git add src/config.rs src/app src/ui/modals.rs
  git commit -m "refactor: centralize session lifecycle"
  ```

**Done when:** Session operations are independently testable and the UI supports Codex-like new/resume/fork navigation without changing the stored history format.

### Task 13: Model subagents as navigable conversation contexts

**Files:**
- Create: `src/app/subagent_controller.rs`
- Modify: `src/app/state.rs`
- Modify: `src/network/subagents.rs`
- Modify: `src/network/ui_adapter.rs`
- Modify: `src/app/events.rs`
- Modify: `src/ui/modals.rs`
- Test: `src/app/subagent_controller.rs`, `src/network/subagents.rs`, `src/ui/tests.rs`

**Interfaces:**
- Add `SubagentId` as a private newtype around the existing subagent identifier.
- Add `SubagentContext { id, name, status, history, active_turn, parent_id }`.
- Add controller methods `spawn`, `send_input`, `interrupt`, `select`, and `list`.
- Subagent progress is delivered through `AgentUiEvent` and frame requests; selection changes the active transcript context without losing the parent context.

- [x] **Step 1: Write lifecycle and navigation tests**

  Cover spawn registration, running/completed/cancelled states, parent ownership, selection, missing IDs, and output routing.

- [x] **Step 2: Adapt existing subagent tool calls**

  Preserve the current tool protocol and max-active-subagent limit while recording typed lifecycle events.

- [x] **Step 3: Add the agent picker/status surface**

  Render the active agent, parent/child relationship, status, and available navigation keys using existing modal infrastructure.

- [x] **Step 4: Run and commit**

  Run: `cargo test network::subagents app::subagent_controller ui::tests && cargo check --tests`

  ```bash
  git add src/app src/network/subagents.rs src/network/ui_adapter.rs src/ui/modals.rs
  git commit -m "feat: add navigable subagent contexts"
  ```

**Done when:** Subagents are independently selectable conversations rather than only rows in global application state.

### Task 14: Match Codex terminal presentation and interaction polish

**Files:**
- Modify: `src/ui/mod.rs`
- Modify: `src/ui/theme.rs`
- Modify: `src/ui/modals.rs`
- Modify: `src/ui/composer.rs`
- Modify: `src/ui/terminal_runtime.rs`
- Modify: `src/app/status.rs`
- Test: `src/ui/tests.rs`

**Interfaces:**
- Keep rendering pure: all render functions consume state and geometry and return Ratatui widgets/lines without performing network or terminal lifecycle operations.
- Centralize status-line, footer, key-hint, terminal-title, and notification formatting in `StatusState`/status helpers.

- [x] **Step 1: Add visual acceptance cases**

  Cover inline/alternate-screen mode, narrow terminal, focused/unfocused terminal, active stream, tool approval, session picker, subagent picker, and external-editor transition.

- [x] **Step 2: Align information hierarchy**

  Apply Codex-like ordering: transcript first, live working/tool state next, composer at the bottom, status/key hints below or alongside the composer, and overlays anchored to the active input area.

- [x] **Step 3: Align motion and notifications**

  Use the frame scheduler for shimmer/spinners, update terminal title from activity state, and notify only on configured focus/finish conditions.

- [x] **Step 4: Run and commit**

  Run: `cargo test ui::tests && cargo check --tests`

  ```bash
  git add src/ui src/app/status.rs
  git commit -m "feat: align Codex-style terminal presentation"
  ```

**Done when:** The interactive UI has Codex-like layout, status, key-hint, overlay, resize, and notification behavior.

### Task 15: Complete regression verification and remove superseded paths

**Files:**
- Modify: only files identified by the preceding tasks’ compatibility shims
- Test: all Rust tests and focused integration tests
- Docs: `README.md` only if user-visible keybindings or modes changed

**Interfaces:**
- No new public interfaces. This task removes only code paths proven unused by the migrated runtime.

- [x] **Step 1: Search for superseded direct coupling**

  Use `rg` to find direct `event::read`, direct terminal escape output, UI calls from network modules, and `AppState::request_redraw` calls that should now use typed events/frame requests.

- [x] **Step 2: Review compatibility paths one category at a time**

  The remaining direct input and terminal paths are owned by the interactive runtime. The redraw flag bridge is still consumed by the runtime for network/background wakeups, so it remains until a typed redraw event replaces every producer safely.

- [x] **Step 3: Run the required repository gates**

  ```bash
  cargo check --tests
  cargo test
  ```

- [x] **Step 4: Verify non-interactive paths**

  Run the existing raw CLI and ACP tests and confirm their binaries do not depend on TUI initialization.

- [x] **Step 5: Review the final diff and commit**

  ```bash
  git diff --check main...HEAD
  git status --short --branch
  git commit -m "refactor: complete Codex harness parity migration"
  ```

**Done when:** All acceptance criteria in the design spec pass, no superseded runtime path remains, and the branch is ready for PR review.

## Execution order and checkpoints

Implement tasks in order. Stop after Tasks 1, 4, 6, 10, 12, and 15 for a review checkpoint because each checkpoint changes a major boundary. Do not start the next checkpoint while the preceding focused tests or repository gates are failing.

The first implementation slice is Task 1 only. It establishes the regression contract without changing production behavior; Task 2 begins the first runtime refactor after that contract is green.
