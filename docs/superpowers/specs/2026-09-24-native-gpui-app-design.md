# Native GPUI App Design

Date: 2026-09-24

## Intent

Build a native macOS RustCode app that runs RustCode's agent in process. The first usable version supports chat and agent turns in a selected project. It opens to an empty start state where the user can begin in the launch directory, choose another folder, or continue a saved session. The existing terminal program remains available and independently buildable.

This design specifies the backend boundary and first functional scope. Visual layout, styling details, and broader TUI feature parity belong to later design work.

## Deliverable and build boundary

- Add a `rustcode-app` workspace crate that produces a native GPUI executable. It links the existing RustCode library; it does not launch `rustcode --acp` or use a WebView.
- Use GPUI Kit's styled `gpui-component` layer through the `gpui-kit` facade. Pin the selected Kit release and its lockfile together, and do not add an independent GPUI version. The initially researched release is `gpui-kit = "=0.6.6"`; confirm availability when implementation begins.
- Keep `rustcode` as the TUI binary. The intended build commands are `cargo build --bin rustcode` and `cargo build -p rustcode-app`.
- The first deliverable is a Cargo-built macOS executable with a native window. App bundle and installer packaging are subsequent work.
- Keep the GPUI Kit dependency in the new app crate, so the TUI target does not directly depend on GUI types. The app's dependency on the existing RustCode library may still compile library modules used by the TUI; extracting a separate backend crate is deferred.

## Shared interactive backend

Expose a small public controller API from the existing `rustcode` library. The controller owns one active workspace and session, `AppState`, the HTTP client, turn cancellation, queue orchestration, background task observation, and session operations. It presents typed commands and immutable UI-neutral snapshots/events to frontends. GPUI must not mutate `AppState` fields or use Ratatui render snapshots.

The controller supports these operations for the first app: choose workspace, start session, list resumable sessions, resume session by ID, select a configured model, submit prompt, cancel active turn, and answer an agent question. It also carries approval decisions as an API operation, though the first app enables RustCode's existing automatic confirmation behavior by default. Existing tool authorization policy still governs actions that cannot be approved automatically.

Prompt submission must preserve RustCode's current FIFO queue, live steering, single-flight orchestrator lease, persistence, and cancellation semantics. The production TUI path currently submits through `app::actions::handle_enter`, while `AppEvent::SubmitPrompt` only updates a draft in test code. Extract an actual submit operation instead of exposing that event as the app command. Consolidate the queue launch path so all turns publish the same agent events, including turns started by pressing Enter in the TUI.

The terminal frontend continues to own keyboard handling, slash-command presentation, Ratatui drawing, terminal scrollback, and terminal-only layout state. It calls the shared controller for agent and session operations. Extraction should be limited to behavior required by the first app; the full TUI state model need not be redesigned in one change.

## Runtime and event flow

GPUI owns the native main thread and its view entities. RustCode's async backend runs on a long-lived Tokio runtime. Typed command and event channels cross that boundary. Tokio tasks do not directly update GPUI entities; a GPUI foreground task consumes backend updates and applies them to the current view.

The backend provides a full snapshot when a workspace or session opens, followed by ordered updates for prompt start, response text, tool activity/results, question requests, turn completion/cancellation, and errors. Snapshots include the current session ID, selected workspace, configured model, visible history, live response, queue/turn status, and pending question. Every update is associated with a session generation or equivalent identity; the frontend discards updates from a session closed or replaced during an active turn. The backend is authoritative for persisted history. The app can coalesce rapid text deltas for rendering while preserving their order.

The first app uses GPUI Kit's existing chat, multiline text, Markdown, list, and dialog components where their behavior fits. The RustCode-specific transcript projection maps backend messages and tool activity into those components. The UI does not parse ACP messages or read `history.json` directly.

## Startup, projects, and sessions

- Opening the app shows an empty start state; it does not automatically resume a session or start an agent turn.
- A terminal launch offers the current working directory as the initial project. The user may choose another existing directory through a native folder picker. One project and one active session are supported at a time in the first version.
- Selecting a saved session resumes it as a live conversation. Existing session metadata does not record a project path, so resume uses the currently selected or launch directory; if no suitable directory has been selected, the app asks for one. New session metadata can record the chosen project in later work, without changing how legacy sessions load.
- Starting a new session uses the chosen project and RustCode's existing workspace configuration and persistence. Changing project or session cancels or finishes the old turn through the controller before replacing the visible state.
- The app lists models from RustCode's effective configuration for the selected workspace. It does not scan TOML text or use a hard-coded model list as the Tauri prototype did.

The previous `~/code/rustcode-app` prototype is a behavior reference for folder selection, project/branch context, model selection, chat, stop, and session discovery. Its React UI, Tauri commands, and ACP bridge are not part of the new implementation.

## First usable behavior

The app can start or resume a session, choose a model, submit a prompt, show streaming assistant content and tool activity, answer an agent question, stop an active turn, and show an actionable error when startup or a turn fails. Automatic tool confirmation is on by default, matching the previous prototype. The app uses the same session store as the TUI and leaves both executables usable.

Settings screens, multiple simultaneous workspaces, detailed visual design, full slash-command parity, a built-in terminal, app bundles/installers, and Linux/Windows validation are outside this first deliverable.

## Failure handling and verification

The controller reports startup, configuration, provider, and session errors as typed failures. The app keeps a usable start or chat state after an error. Cancellation and session switching do not let stale agent events update a newer session. Shutting the app down cancels active work and flushes session history through existing RustCode persistence paths.

Implementation verification covers controller submission/queue/streaming, cancellation, session switching, model selection, question responses, and TUI behavior after extraction. Run `cargo check --tests` and `cargo test` as required by this repository, build each executable separately, and smoke-test the macOS app window with a local or configured model. The first implementation need not publish a release artifact.

## Alternatives considered

- An ACP subprocess repeats the process and protocol bridge that caused friction in the Tauri app, so it is not the native app's backend path.
- Moving all interactive code into a new backend crate before building the GUI would cross many current `app`, `network`, `config`, `tools`, and TUI references. The controller API will be established inside the current library first; a backend crate can follow if independent builds or ownership boundaries justify it.

## Source anchors

- RustCode TUI runtime and queue: `src/app/runtime/mod.rs`, `src/app/runtime/orchestration.rs`, `src/app/actions/enter.rs`, `src/network/turn/queue.rs`.
- Existing event and session seams: `src/network/ui_adapter.rs`, `src/app/events.rs`, `src/app/session_controller.rs`, `src/app/state.rs`.
- GPUI Kit setup and component catalog: <https://github.com/longbridge/gpui-kit> and <https://github.com/longbridge/gpui-kit/releases>.
