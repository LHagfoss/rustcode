# Native Settings, Interaction Polish, and Harness Reliability Design

Date: 2026-09-25
Issues: [#1411](https://github.com/LHagfoss/rustcode/issues/1411), [#1412](https://github.com/LHagfoss/rustcode/issues/1412), [#1413](https://github.com/LHagfoss/rustcode/issues/1413), [#1414](https://github.com/LHagfoss/rustcode/issues/1414)

## Purpose

RustCode's new GPUI desktop application should feel coherent and dependable: navigation should be predictable, permission prompts must be actionable, and common controls should remain usable under keyboard and pointer interaction. Separately, the agent harness must not stop productive work because a sandboxed checker could not create temporary files.

The work is delivered as two focused pull requests. The native GPUI pull request owns desktop presentation and interaction. The harness reliability pull request owns the session failure reproduced from session `01a0d840854b-7000-9b08-10aa-10aa1ee60034`. The two changes share a design document but no runtime dependency.

## Goals

### Native GPUI application

- Replace the temporary Settings dialog with a scalable in-app Settings page.
- Keep the session sidebar visible and add a bottom-anchored Settings destination below a divider.
- Open Settings on the General section from both the sidebar and `Cmd+,`.
- Centralize the desktop dark-gray palette and use a brighter gray for the selected session.
- Give model and approval preferences clear, rounded, accessible controls.
- Make required tool approvals visibly actionable with persistent Approve and Deny controls.
- Make slash-command selection work with Up, Down, Escape, pointer selection, and Enter.
- Complete a slash command without submitting it and leave the caret after the inserted command.
- Render recognized slash-command drafts in bold high-contrast neutral text.
- Keep user and assistant copy buttons visible while the pointer moves onto them.
- Preserve queued follow-up prompts when Stop cancels the active turn and show that a message is queued.
- Use one contextual Send or Stop control and reduce the composer's default height.
- Improve tool-call observability with an animated running state and an obvious bounded output viewport.
- Add `/info` with the active session ID and a small first batch of genuinely supported native diagnostic commands.
- Add restrained state transitions that respect reduced-motion preferences.

### Harness reliability

- Run inline compiler and formatter checks with an isolated writable temporary directory.
- Prevent checker scratch files from appearing in the active project.
- Distinguish source diagnostics from checker startup, sandbox, and environment failures.
- Ensure infrastructure failures do not increment the unchanged-compiler-diagnostic budget.
- Preserve the active session workspace when dispatching sandboxed shell commands.
- Cover the captured failure with regression tests.

## Non-goals

- Redesigning the terminal UI.
- Sharing the TUI theme-file format with the desktop app.
- Adding new user-configurable preferences beyond the currently supported model and approval mode.
- Prompting for every safe or read-only tool call. Ask-first continues to prompt only when policy requires confirmation.
- Replacing the GPUI text engine solely to gain per-token rich-text styling.
- Advertising the full TUI slash-command registry before native controller behavior exists for those commands.
- Requiring live subprocess output streaming in this pull request if the executor has no existing bounded chunk callback.
- Changing tool authorization policy or weakening sandbox restrictions.

## Delivery Structure

### Pull request 1: Native GPUI settings and interactions

This pull request is based on issue #1411 and changes only the desktop application plus any narrowly required controller projection tests. It introduces page routing, the desktop palette, settings presentation, approval presentation, slash-command behavior, and hover fixes as one coherent UX update.

### Pull request 2: Harness checker and workspace reliability

This pull request is based on issue #1412 and starts from the then-current `main`. It fixes checker scratch handling, diagnostic classification, and active-workspace propagation. Keeping it separate makes the operational regression independently reviewable and reversible.

## Native Application Design

### Navigation and page state

`AppView` gains a small explicit page state with at least `Chat` and `Settings(SettingsSection::General)`. Chat mounts the session sidebar. Settings replaces that rail in the same width with a dedicated Settings navigation rail, avoiding a nested three-column layout.

The chat sidebar becomes a full-height column with its existing header and session groups in a flexible upper region. A full-width divider separates a bottom Settings row with a settings icon and label. Activating Settings replaces this rail with a Settings rail containing a clear Back to app action and the compact General destination. Back to app, a session, or New Chat returns to Chat. Settings changes the main content to General without opening another operating-system window.

The existing `OpenSettings` action is retained but changes behavior: `Cmd+,` routes to `Settings(General)`. Reusing the action preserves the macOS menu integration already added by the native app.

### Settings page

The Settings page uses a dedicated left rail plus a constrained, scrollable content column modeled after a native preferences surface, with:

- a `General` page title and section headings;
- a Model card with the active model, model identifier, and a dropdown/popup menu containing available models;
- a Permissions card with a two-choice control for Ask first and Auto approve, explanatory text, and disabled/session-unavailable states where appropriate.

Grouped settings surfaces use consistent generous radii, subtle borders, compact rows, and clear labels/supporting text like a native preferences page. The state and commands remain the existing `SettingsState`, `Command::SelectModel`, and `Command::SetAutoApprove`; the redesign does not add a parallel settings backend or advertise categories that are not implemented.

The route and section are explicit enums so future sections can be added without replacing the navigation model. Only General is implemented now.

### Desktop palette

A native-app palette module defines semantic colors for:

- application, sidebar, elevated, card, hover, and selected surfaces;
- subtle and strong borders;
- primary, secondary, muted, danger, and neutral selection text;
- shared control radii.

The app applies matching gpui-kit theme tokens at initialization for components whose selected state is owned by the toolkit, including `SidebarMenuItem`. App-authored surfaces consume the semantic palette instead of duplicating RGB literals in the touched UI.

All app-authored selection, focus, inline-code, command, and active-navigation treatments use neutral graphite/gray/white colors. Purple and blue-purple accents are not part of this theme. This is intentionally desktop-local. The TUI theme system remains independent.

### Slash-command interaction

Slash suggestions remain derived from `slash.rs`, while keyboard behavior is expressed through a small testable interaction transition:

- Up and Down enter and move the highlighted suggestion, wrapping at the ends.
- Escape dismisses the popup without changing the draft.
- Enter with an open, non-dismissed popup inserts the highlighted command and returns without submitting.
- Pointer selection uses the same completion helper as Enter.
- Completion writes the new value, explicitly places the multiline caret at the end, and restores focus.

The GPUI test harness will reproduce key dispatch through the focused composer. The final handler will be attached at the narrowest level demonstrated by the failing test, rather than relying only on pure `move_selection` tests.

The current `Textarea` API cannot apply per-range text styling. While the draft is recognized as a slash-command draft, the textarea therefore applies high-contrast neutral foreground and bold weight to the complete draft. Ordinary prompts retain normal body styling. Arguments typed after a completed command return to ordinary styling when they no longer meet the recognized slash-draft predicate.

Selected Settings rows use the same compact, left-aligned shape, spacing, typography, and neutral selected surface as selected session rows. The outer sidebar Settings destination and the inner General destination should read as navigation peers, not large centered call-to-action buttons.

### Approval presentation

When the controller exposes a pending required approval, the desktop app renders a persistent permission card above the composer. The card includes:

- tool name and bounded description;
- Approve and Deny buttons;
- clear pending styling distinct from ordinary status text.

The card sends the existing `Command::Approval` variants and remains visible until state confirms resolution. A streamed `ApprovalRequested` event and a pending approval in a snapshot both drive the same view state. Safe tools continue without prompting under existing policy.

Approval decisions carry the exact controller-owned pending-batch identity. The worker compares that identity atomically against the currently pending batch immediately before resolving it, so a delayed callback cannot authorize a replacement batch. Batch identities do not rely solely on provider tool-call IDs, which may repeat.

Every confirmation-required action in a batch is disclosed. Each action shows a bounded preview in the list and offers an in-card disclosure for the complete literal details inside a bounded selectable scroll area; truncation is never the only way to inspect an action before approving it.

Tests cover controller-event projection into pending approval state and the rendered action surface. If the event adapter fails to recover a real pending tool call, that projection defect is fixed at the adapter boundary rather than masked in the view.

### Copy-button hover behavior

The copy action and the visual message area share one hover hit region. The button may visually sit at the lower edge, but the parent layout reserves its space so moving from content to the button never leaves the hover group. User and assistant messages use the same helper or layout pattern.

The copy action is left-aligned below the message content. Its visible icon is slightly larger than the original and its transparent clickable container adds padding/minimum size as hit slop, without expanding the colored user-message bubble or covering selectable text.

Clipboard behavior and labels remain unchanged.

### Queue, composer, and contextual action

Submitting a valid draft with Enter while a turn is active queues the follow-up through the existing controller path. Native projection retains `queued_count` and displays a compact queued indicator near the composer. Stopping the active turn cancels only that turn; the queued follow-up remains and begins when the controller returns to the queue boundary. A controller regression test covers the exact submit-while-streaming, then Stop ordering.

The footer has one primary circular action: Send while idle and Stop while a turn is active. This avoids adjacent contradictory controls while preserving Enter-to-queue. The textarea begins at one row and grows to a smaller bounded multiline height, with tighter vertical padding than the current composer.

### Tool activity, output, and motion

Running tool rows use a GPUI repeating rotation animation on the loader icon. The animation uses the framework's reduced-motion-aware path; completed and failed transitions settle into their existing status icons. Expanded arguments and output sit in a visually distinct, bounded viewport with an obvious scrollbar and selectable content.

The implementation inspects the existing executor progress path for bounded output chunks. If a safe callback already exists, chunks are projected through a typed controller event; otherwise this pull request keeps final output projection and records live streaming as follow-up scope rather than inventing an unbounded parallel execution channel.

Chat/tool state changes may use short opacity/position transitions where GPUI supports them without changing hit testing or delaying content. Motion is functional, subtle, and disabled by the framework under reduced-motion preferences.

### Native diagnostic commands

The native command parser and slash registry add `/info` as the canonical diagnostic command. It renders the active session ID plus selected model, active/idle state, and queued-message count from the controller snapshot. `/session` may be provided as a discoverable alias if it uses the same tested handler. Only commands backed by native controller behavior appear in suggestions; the larger TUI registry remains an incremental roadmap.

### Transcript position rail

The chat transcript gains a quiet right-edge position rail made from short neutral dashes. It is driven by the virtual list's actual logical top row, not by message count or tail-following guesses. As the user scrolls, the current dash becomes brighter and slightly wider with a short reduced-motion-aware transition. Markers are clickable and scroll to the represented display row.

Each grouped `DisplayRow` is one navigation position for normal-size conversations. Long conversations use a fixed maximum number of representative markers, mapping rows proportionally into those markers, so the rail has stable visual density and constant rendering cost instead of creating an element for every historical row. The rail is absent when there is no meaningful position choice and does not replace the existing jump-to-latest action or virtualized scrolling.

## Harness Reliability Design

### Captured failure

The referenced session completed provider requests successfully and was not cancelled. After file deletions, every inline `bunx biome check .` invocation failed with a temporary-directory `PermissionDenied`. The checker created `.hm` files and a `bunx-501-biome@latest` directory in the project. Four identical outputs were marked as compiler diagnostics, triggering `TurnBudgetLimit::CompilerDiagnostics` and stopping an incomplete task.

The same session also returned `macOS shell sandbox needs an active workspace` for `run_command`, despite workspace-aware file and Git tools operating on the project.

### Checker scratch directory

Each inline compiler/checker invocation receives a harness-owned temporary directory outside the project and inside the sandbox's allowed session scratch roots. Relevant temporary-environment variables are set to that explicit directory. The directory lifetime covers the subprocess and is cleaned after completion.

The project root remains the command working directory so project discovery is unchanged. Only scratch output moves out of the project.

### Diagnostic classification

Compiler/checker execution returns a typed outcome or an equivalent explicit classification:

- Passed;
- Source diagnostics;
- Unverified infrastructure failure.

Nonzero output that clearly represents compiler or linter findings remains source diagnostics. Sandbox setup failures, missing active workspace, process-launch failures, cancellation, timeout, and temporary-directory failures become unverified infrastructure failures.

Only source diagnostics receive the compiler diagnostic marker and fingerprint. Unverified failures remain visible to the agent, keep the compiler cache dirty, and cannot increment `consecutive_diagnostics`.

### Active workspace propagation

The shell execution path uses the active task workspace already held by session state when the tool call omits an explicit working directory. The resolved command working directory, sandbox workspace root, and writable workspace root agree. If no valid workspace exists, execution still fails closed with the existing actionable error.

No sandbox permission is broadened beyond the active workspace and harness-owned session scratch directory.

## Error Handling

- Settings actions that fail continue through the existing controller error surface.
- Empty model lists show an explanatory disabled state rather than an empty picker.
- Approval commands that race with resolution surface the controller error and allow the next snapshot to remove the stale card.
- Slash completion is a no-op if the selected index is no longer valid after filtering.
- Checker scratch setup failure produces an unverified result; it does not silently report success.
- Invalid or absent session workspaces continue to fail closed.

## Testing

### Native GPUI pull request

- Unit tests for route transitions and recognized slash-draft styling predicate.
- Existing slash filtering/wrapping tests plus completion/caret tests.
- GPUI interaction tests for Up/Down, Escape, Enter completion, and no accidental submission.
- Projection/render tests for pending approval controls.
- Tests for settings state and General-section model/approval selection.
- Controller regression for queuing during streaming followed by Stop, plus native queue projection.
- Contextual Send/Stop and compact composer-state tests.
- Tool running/finished presentation and bounded scroll-viewport tests.
- Parser/controller tests for `/info` (and `/session` if included), asserting the session ID is present.
- Targeted visual/manual verification of selected-session contrast, Settings layout, approval card, and both copy buttons.
- Full `cargo check --tests` and `cargo test`.

### Harness reliability pull request

- A regression test reproducing a checker that needs a writable temporary directory.
- A regression test proving scratch artifacts do not appear in the project root.
- Classification tests for source diagnostics versus infrastructure failures.
- A budget test proving repeated unverified failures do not advance the compiler-diagnostic streak.
- A workspace-propagation test for a sandboxed shell call without an explicit `cwd`.
- Replay of the originating deletion scenario in a fresh session when practical, comparing stop reason and compiler-diagnostic streak with the captured session.
- Full `cargo check --tests` and `cargo test`.

## Acceptance Summary

The work is complete when the native app has a dedicated, extensible General settings page; the reported slash, approval, selected-session, and copy-hover interactions work under their real input paths; and the captured TUI session failure can no longer stop a turn because a checker lacked temporary-directory access or a shell call lost its active workspace.
