# Codex-Like Agent Harness and TUI Design

**Date:** 2026-08-15
**Status:** Proposed
**Repository:** rustcode

## Goal

Make rustcode’s interactive CLI behave and feel like the Codex CLI while preserving rustcode’s existing model providers, tool registry, policy system, ACP mode, and turn-engine behavior during the migration.

The target is user-visible harness and TUI parity: a responsive terminal application with typed event flow, coalesced redraws, separated transcript/composer/status surfaces, reliable tool approvals and cancellation, and first-class session/subagent navigation.

## Scope

This work covers the interactive local CLI:

- terminal lifecycle and restoration;
- keyboard, paste, focus, resize, and redraw events;
- application event routing;
- composer, transcript, status, modal, and tool-result surfaces;
- streaming model output and tool lifecycle presentation;
- session creation, resume, fork, replay, and deletion/archive behavior;
- subagent/thread navigation;
- deterministic UI and harness verification.

The existing network turn engine remains the behavioral source of truth initially. The migration will introduce an adapter around it rather than replacing it with Codex source or immediately adopting Codex’s cloud-specific runtime.

## Non-goals

The initial parity effort does not reproduce Codex-specific services that do not exist in rustcode:

- ChatGPT account authentication and billing surfaces;
- remote app-server transports;
- Codex cloud configuration and managed environments;
- Codex plugin marketplaces;
- Codex-specific analytics or telemetry contracts;
- wholesale replacement of rustcode’s provider and tool implementations.

Those systems may be considered after the local harness and TUI boundaries are stable.

## Current architecture

Rustcode currently starts an `InlineTerminal`, stores interactive state in a shared `Arc<Mutex<AppState>>`, polls crossterm input from `src/main.rs`, and spawns `process_queue_orchestrator` for queued prompts. The orchestrator calls `run_agent_turn`, which repeatedly calls `run_single_turn`; the turn state machine classifies model output, gates tool approval, executes tools, persists history, and continues until completion.

The main constraints are:

- terminal input, rendering, application lifecycle, and command dispatch are concentrated in `src/main.rs`;
- `AppState` contains transcript, composer, modal, tool, session, scroll, and subagent state together;
- asynchronous work updates shared state directly;
- redraw decisions are made by the main loop rather than requested through a typed frame channel;
- session history is durable, but the UI and agent lifecycle are not represented as separate thread/event domains.

Relevant current components:

- `src/main.rs` — terminal setup, event loop, keyboard routing, queue startup, and rendering coordination;
- `src/app/state.rs` — shared application and interaction state;
- `src/ui/mod.rs` and `src/ui/modals.rs` — transcript, composer, status, and modal rendering;
- `src/network/turn_engine.rs` — agent turn and queue orchestration;
- `src/network/events.rs` — turn state machine and agent event classification;
- `src/network/tool_exec.rs` — tool approval and execution;
- `src/config.rs` — session history persistence and resume metadata.

## Target architecture

The target is a layered local harness:

```text
CLI entrypoint
  └─ AppRuntime
       ├─ TerminalRuntime
       ├─ TuiEventStream
       ├─ FrameRequester
       ├─ AppEvent channel
       ├─ SessionController
       ├─ AgentTurnAdapter
       └─ ChatWidget
            ├─ Composer
            ├─ Transcript
            ├─ Tool/approval surfaces
            ├─ Status/footer
            ├─ Session controls
            └─ Subagent navigation
```

### TerminalRuntime

Owns raw mode, alternate-screen and inline viewport transitions, terminal cleanup, focus state, external-editor restoration, resize handling, and stderr isolation. No agent or widget code may need to emit terminal escape sequences directly.

### TuiEventStream

Normalizes terminal input into typed events:

```rust
pub enum TuiEvent {
    Key(crossterm::event::KeyEvent),
    Paste(String),
    Resize { width: u16, height: u16 },
    FocusGained,
    FocusLost,
    Draw,
}
```

The event source must support pausing and resuming input while an external interactive program owns the terminal.

### FrameRequester

Background tasks and widgets receive a cloneable redraw handle:

```rust
#[derive(Clone, Debug)]
pub struct FrameRequester { /* scheduler handle */ }

impl FrameRequester {
    pub fn schedule_frame(&self);
    pub fn schedule_frame_in(&self, duration: std::time::Duration);
}
```

Requests are coalesced and rate-limited before reaching the application loop.

### AppEvent

Widgets communicate actions to the application controller through typed events instead of mutating orchestration state directly. The initial event family must cover:

- submit/edit/queue user input;
- cancel or interrupt active work;
- approve/deny tool requests;
- open and close overlays;
- start/resume/fork/clear/delete sessions;
- select or navigate subagents;
- request redraw and terminal restoration;
- report fatal shutdown conditions.

The application controller owns event ordering and invokes the existing turn engine through an adapter.

### AgentTurnAdapter

The existing `run_agent_turn` and `process_queue_orchestrator` remain operationally authoritative. The adapter translates their state changes into UI-facing events and accepts user decisions through the existing `TurnPolicy` path. This permits UI migration without changing provider payloads, tool parsing, or turn semantics in the first stages.

The adapter must preserve:

- approval-before-execution behavior;
- cancellation propagation;
- tool batching and tool-result persistence;
- loop/recovery and finish-gate behavior;
- history writes and final transcript generation;
- headless/ACP behavior outside the interactive TUI.

### ChatWidget

Owns only presentation and interaction state for the active session. It does not run the agent. It renders model/tool progress and emits `AppEvent` or agent commands.

The widget is decomposed by responsibility:

- `composer` — text editing, cursor, paste, suggestions, slash commands, and input history;
- `transcript` — durable history cells, live streaming cells, tool cells, and replay/reflow;
- `status` — running state, model, context/quota, terminal title, and footer hints;
- `approval` — tool confirmations, questions, and permission controls;
- `session` — new/resume/fork/clear/archive/delete actions;
- `subagents` — thread list, liveness, selection, and navigation;
- `overlays` — command/model/theme/history/config pickers.

## Data flow

### User input

```text
Terminal event
  → TuiEventStream
  → App::handle_tui_event
  → ChatWidget/composer
  → AppEvent::SubmitUserMessage
  → SessionController
  → AgentTurnAdapter
  → existing turn engine
```

### Model/tool progress

```text
turn engine
  → AgentUiEvent
  → active session event channel
  → ChatWidget state
  → FrameRequester
  → App::render
```

### Tool approval

```text
tool request
  → AgentUiEvent::ApprovalRequested
  → approval surface
  → AppEvent::ApprovalDecision
  → TurnPolicy / turn engine
  → tool execution or denied result
```

### Cancellation

Cancellation must travel through one path: user input creates an application cancellation command, the controller cancels the active token, the turn engine records completed work plus typed cancellation results, and the UI receives a terminal turn event. No layer may silently drop an in-flight operation.

## Migration phases

Each phase must compile, pass focused tests, and leave the interactive CLI usable. Phases are ordered by dependency and should normally become separate PRs.

1. Baseline UI snapshots and acceptance tests.
2. Terminal runtime extraction and cleanup guards.
3. Typed TUI events and centralized event dispatch.
4. Coalesced frame scheduling.
5. App event/command channels.
6. Application/controller extraction from `main.rs`.
7. Chat state decomposition.
8. Transcript/history-cell and streaming reflow model.
9. Composer, keymaps, and bottom-pane interaction.
10. Turn-engine UI adapter.
11. Approval, tool, question, and cancellation surfaces.
12. Session lifecycle and resume/fork UI.
13. Subagent/thread navigation.
14. Codex-like terminal polish, status, notifications, and narrow-terminal behavior.
15. Full regression verification and removal of superseded paths.

## Compatibility rules

- Headless CLI and ACP paths must not depend on Ratatui or TUI-only types.
- Existing provider profiles, tool names, config files, and session history remain readable throughout the migration.
- New event types must be private to the interactive harness unless a shared protocol is required by ACP or another existing public interface.
- Do not add a dependency when existing crossterm, Ratatui, Tokio, and standard-library facilities are sufficient.
- Preserve current behavior unless a phase explicitly changes it for Codex parity.
- Avoid copying Codex implementation code; reproduce behavior through rustcode-native interfaces.

## Verification strategy

Every phase requires focused tests before integration:

- pure state-transition tests for event routing and session lifecycle;
- Ratatui `TestBackend` or terminal-buffer snapshots for visual surfaces;
- event-stream tests for key, paste, resize, focus, and draw ordering;
- frame scheduler tests for coalescing and rate limiting;
- turn adapter tests proving approval, tool execution, cancellation, retry, and completion mapping;
- transcript replay and resize tests;
- session resume/fork persistence tests;
- subagent navigation and liveness tests.

The branch-level gates remain:

```bash
cargo check --tests
cargo test
```

## Acceptance criteria

The migration is successful when:

1. The interactive CLI has a typed, multiplexed event loop for terminal, redraw, agent, and session events.
2. Background model/tool streaming can request redraws without direct terminal writes or ad hoc polling changes.
3. Composer, transcript, status, overlays, approvals, and session controls are independently testable.
4. Resize, focus loss, cancellation, external-editor execution, and terminal restoration do not corrupt the visible transcript or leave terminal modes enabled.
5. New/resume/fork/clear/archive flows preserve the existing history format and produce Codex-like interaction behavior.
6. Subagents are navigable as independent conversation contexts rather than only status rows in global state.
7. Headless and ACP execution continue to use the existing non-TUI paths.
8. The final UI has Codex-like information hierarchy and interaction patterns without requiring Codex-specific cloud services.
