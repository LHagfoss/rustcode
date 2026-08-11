# Activity-Aware Terminal Status Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the terminal title and footer share a compact, deterministic, animated activity status with short session names.

**Architecture:** Add pure formatting and classification helpers in `src/app/activity.rs`. The main draw loop will use those helpers for OSC terminal titles, while the footer renderer will use the same activity snapshot and animation frame so both surfaces stay synchronized. Existing state ownership, terminal escape emission, token metrics, quota display, and model orchestration remain unchanged.

**Tech Stack:** Rust, Tokio state loop, Ratatui, Crossterm OSC title sequences, existing `AppStatus` and `AppState`.

## Global Constraints

- The terminal title must remain short and sanitize terminal-title control characters.
- Animation is active for queued requests, model response, tool execution, and action-required states.
- Idle titles are stable and must not emit unnecessary OSC sequences.
- Activity labels must be deterministic; remove random footer status phrases.
- Tool execution takes precedence over generic streaming text, and action-required states take precedence over background activity.
- Preserve the right-side footer metrics: tokens, context, quota, and command hints.
- Do not change model providers, rate-limit handling, tool parsing, orchestration behavior, or session persistence.
- Verify with `cargo check --tests` and `cargo test`.

---

### Task 1: Add pure activity classification and formatting helpers

**Files:**
- Create: `src/app/activity.rs`
- Modify: `src/app/mod.rs`

**Interfaces:**
- Consumes: `crate::app::AppStatus` and a borrowed `running_tools` slice.
- Produces: `ActivityKind`, `ActivitySnapshot`, `classify_activity`, `sanitize_session_name`, `animation_cells`, and `format_terminal_title` for the main loop and footer.

- [ ] **Step 1: Write failing unit tests in `src/app/activity.rs`**

Add tests for the public helper behavior:

```rust
#[test]
fn activity_precedence_prefers_action_required_then_tool_then_queue() {
    assert_eq!(
        classify_activity(&AppStatus::AwaitingQuestion, &["run_command".into()]).kind,
        ActivityKind::ActionRequired
    );
    assert_eq!(
        classify_activity(&AppStatus::Streaming, &["run_command".into()]).kind,
        ActivityKind::RunningTool
    );
    assert_eq!(
        classify_activity(&AppStatus::Queued, &[]).kind,
        ActivityKind::Queued
    );
}

#[test]
fn session_names_are_sanitized_and_truncated() {
    assert_eq!(sanitize_session_name("  fix | parser\u{0007}\nissue  ", 18), "fix / parser issue");
    assert!(sanitize_session_name("a very long session name", 12).chars().count() <= 12);
}

#[test]
fn terminal_title_contains_state_and_short_name() {
    let title = format_terminal_title(ActivityKind::Working, "tower defense", 2);
    assert_eq!(title, "[••] Working · tower defense");
}

#[test]
fn animation_cells_reach_both_edges() {
    let first = animation_cells(0, 12);
    let later = animation_cells(8, 12);
    assert!(first.iter().any(|cell| *cell));
    assert!(later.iter().any(|cell| *cell));
    assert_ne!(first, later);
}
```

Use the exact `AppStatus` variants already defined in `src/app/state.rs`. Keep the tests deterministic by passing an explicit frame index instead of reading wall-clock time.

- [ ] **Step 2: Run the focused tests and verify they fail**

Run:

```bash
cargo test app::activity::tests
```

Expected: compilation fails because `src/app/activity.rs` and its public helpers do not exist yet.

- [ ] **Step 3: Implement the pure helper module**

Create `src/app/activity.rs` with:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityKind {
    Ready,
    Queued,
    Working,
    RunningTool,
    ActionRequired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivitySnapshot {
    pub kind: ActivityKind,
    pub label: String,
    pub detail: Option<String>,
    pub animated: bool,
}

pub fn classify_activity(status: &AppStatus, running_tools: &[String]) -> ActivitySnapshot;
pub fn sanitize_session_name(raw: &str, max_chars: usize) -> String;
pub fn animation_cells(frame: u64, width: usize) -> Vec<bool>;
pub fn format_terminal_title(kind: ActivityKind, session_name: &str, frame: u64) -> String;
```

Classification requirements:

- `AwaitingToolConfirmation`, `AwaitingQuestion`, `VerbosityPicker`, `ThinkingPicker`, and `ProtocolPicker` produce `ActionRequired`.
- A non-empty `running_tools` slice produces `RunningTool` with the first tool name as `detail`.
- `Queued` produces `Queued`.
- `Streaming` produces `Working`.
- `Idle` produces `Ready`.

Sanitization must trim whitespace, replace `|` with `/`, remove ASCII control characters, collapse newlines to spaces, and truncate by Unicode scalar count without exceeding the requested limit. Empty names become `session`.

Use a 12-cell chase/pulse frame for the footer and title markers. `Ready` must return a stable non-animated frame. `format_terminal_title` must produce `[>] Queued`, `[!] Action Required`, `[·] Working`, `[•] Running`, or `rustcode · Ready`, followed by ` · <session>` for non-empty session names. Use the frame index to vary the working marker without reading global time.

- [ ] **Step 4: Export the module and run focused tests**

Add `pub mod activity;` to `src/app/mod.rs`, then run:

```bash
cargo test app::activity::tests
```

Expected: all activity helper tests pass.

- [ ] **Step 5: Commit the helper module**

```bash
git add src/app/activity.rs src/app/mod.rs
git commit -m "feat: add activity status formatting"
```

### Task 2: Integrate activity state into terminal titles

**Files:**
- Modify: `src/main.rs:359-395`
- Modify: `src/app/state.rs:739-743`

**Interfaces:**
- Consumes: `AppState::status`, `AppState::running_tools`, cached/generated session title, and the existing draw-loop timing.
- Produces: synchronized OSC titles such as `[••] Working · tower-defense`, `[!] Action Required · tower-defense`, and `rustcode · Ready · tower-defense`.

- [ ] **Step 1: Add title integration tests or extend helper tests**

Extend `src/app/activity.rs` tests to cover the final title forms:

```rust
#[test]
fn title_states_are_compact_and_distinct() {
    assert_eq!(format_terminal_title(ActivityKind::Queued, "bench", 0), "[>] Queued · bench");
    assert_eq!(format_terminal_title(ActivityKind::ActionRequired, "bench", 0), "[!] Action Required · bench");
    assert_eq!(format_terminal_title(ActivityKind::Ready, "bench", 0), "rustcode · Ready · bench");
}
```

- [ ] **Step 2: Track an animation frame in `AppState`**

Add a small monotonic frame counter or derive a frame from the existing draw-loop elapsed time. Prefer a field only if the draw loop already needs persistent frame state; otherwise derive `frame` from `Instant` so no new persistence is required. The value must change while `response_active` is true and remain stable for idle rendering.

- [ ] **Step 3: Replace inline title formatting in `main.rs`**

Keep the existing custom-title cache and prompt fallback. Pass the resulting session name plus `guard.status`, `guard.running_tools`, and the animation frame to `classify_activity`, `sanitize_session_name`, and `format_terminal_title`. Continue comparing against `guard.current_terminal_title` before emitting `\x1b]0;...\x07`.

Do not emit an OSC title update solely because the draw loop ran; the formatted title must differ first.

- [ ] **Step 4: Run title-related tests and compile checks**

Run:

```bash
cargo test app::activity::tests
cargo check --tests
```

Expected: all tests pass and the crate compiles.

- [ ] **Step 5: Commit terminal-title integration**

```bash
git add src/main.rs src/app/state.rs src/app/activity.rs
git commit -m "feat: show activity in terminal title"
```

### Task 3: Redesign the footer activity row and verify the full change

**Files:**
- Modify: `src/ui/mod.rs:720-1027`

**Interfaces:**
- Consumes: `classify_activity`, `animation_cells`, `AppState` status/tools/timing, and existing footer metric spans.
- Produces: a 12-cell deterministic activity block, a wider Auto-Confirm center block, and unchanged right-side metrics.

- [ ] **Step 1: Add footer formatting assertions**

Add unit tests near the existing footer animation tests for the 12-cell frame width and deterministic state labels. Keep pure assertions in `src/app/activity.rs`; use UI tests only for layout-specific behavior if needed.

- [ ] **Step 2: Replace random footer statuses**

In `render_footer`, remove the `random_statuses` array and build the left row from the shared `ActivitySnapshot`:

- `Ready`: 12 dim cells plus `Ready`.
- `Queued`: animated cells plus `Queued · waiting for model`.
- `Working`: animated cells plus `Working · Responding` and elapsed seconds.
- `RunningTool`: animated cells plus `Running · <first tool>` and elapsed seconds.
- `ActionRequired`: 12 highlighted exclamation/attention cells plus `ACTION REQUIRED` and the relevant tool/question detail when available.

Keep the existing `esc interrupt` hint only for interruptible states. Keep `show_picker` theming behavior intact.

- [ ] **Step 3: Widen the center footer block**

Change the fixed `Constraint::Length(22)` to a width that accommodates `Auto-Confirm: OFF` with surrounding space, targeting `Constraint::Length(28)`. Preserve the two flexible outer regions and right-aligned metric content.

- [ ] **Step 4: Run the full verification suite**

Run:

```bash
cargo check --tests
cargo test
```

Expected: both commands pass with no regressions.

- [ ] **Step 5: Inspect the diff and commit the footer integration**

```bash
git diff --check
git diff main...HEAD -- src/app/activity.rs src/app/mod.rs src/app/state.rs src/main.rs src/ui/mod.rs
git add src/ui/mod.rs
git commit -m "feat: improve activity footer"
```

- [ ] **Step 6: Final branch verification**

Run:

```bash
cargo check --tests
cargo test
git status --short --branch
```

Expected: checks pass and only intentional committed branch changes remain.
