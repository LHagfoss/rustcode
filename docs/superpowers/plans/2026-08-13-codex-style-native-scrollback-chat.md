# Codex-Style Native-Scrollback Chat Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Keep rustcode's present chat/composer appearance while committing completed conversation rows to native terminal scrollback and limiting Ratatui to the active tail and keyboard controls.

**Architecture:** The normal interactive process keeps a compact `Viewport::Inline` for the mutable assistant tail, `Working...`, queue preview, and composer. A `TranscriptCommitter` turns finalized history entries and newline-stable stream rows into `Line<'static>` blocks, inserts them before that viewport with `Terminal::insert_before`, and tracks exactly what was committed. Existing message rendering is extracted into reusable block builders so committed and live content share styling without retaining a whole-history viewport.

**Tech Stack:** Rust 2024, Tokio, crossterm, Ratatui 0.30.2 (`Viewport::Inline` and `Terminal::insert_before`), existing `AppState`, message/tool renderers, and UI tests.

## Global Constraints

- Preserve current visual styling for messages, tools, queue preview, composer, and keyboard interactions.
- The native terminal owns completed transcript scrolling, selection, copying, and search; do not emulate transcript scrolling in Rust.
- The live viewport contains only active tail/status/queue/composer or an inline interaction, never full history.
- Use `Working...` as the only model-turn status; remove Thinking/Responding wording.
- Remove mouse capture, hover/copy badge state, selection code, PageUp/PageDown transcript navigation, and runtime theme switching from normal interactive chat.
- Preserve agent execution, session persistence/resume, queued prompts, ACP, `--prompt`, and keyboard tool/question approval flows.
- Do not add dependencies. Verify with `cargo check --tests` and `cargo test`.

---

### Task 1: Specify stable streaming rows and commit boundaries

**Files:**
- Create: `src/ui/scrollback.rs`
- Modify: `src/ui/mod.rs`
- Modify: `src/ui/tests.rs`

**Interfaces:**
- Produces `pub(crate) fn split_stable_rows(text: &str) -> (Vec<String>, String)`.
- Produces `pub(crate) struct TranscriptCursor { next_history_index: usize, committed_stream: String }` with methods that return only uncommitted entries/rows.
- `split_stable_rows` returns every complete newline-terminated row without its newline and retains an incomplete suffix unchanged.

- [ ] **Step 1: Write the failing stable-tail and deduplication tests**

Add tests in the existing `src/ui/tests.rs` module that define the boundary contract:

```rust
#[test]
fn split_stable_rows_keeps_only_the_incomplete_suffix_live() {
    let (stable, tail) = super::scrollback::split_stable_rows("first\nsecond\nthird");

    assert_eq!(stable, vec!["first", "second"]);
    assert_eq!(tail, "third");
}

#[test]
fn transcript_cursor_never_recommits_history_or_stream_rows() {
    let mut cursor = super::scrollback::TranscriptCursor::default();
    assert_eq!(cursor.take_history_range(3), 0..3);
    assert_eq!(cursor.take_history_range(3), 3..3);
    assert_eq!(cursor.take_stable_stream("alpha\nbeta"), vec!["alpha"]);
    assert!(cursor.take_stable_stream("alpha\nbeta").is_empty());
}
```

- [ ] **Step 2: Run test to verify RED**

Run: `cargo test ui::tests::split_stable_rows_keeps_only_the_incomplete_suffix_live && cargo test ui::tests::transcript_cursor_never_recommits_history_or_stream_rows`

Expected: compilation fails because `ui::scrollback`, `split_stable_rows`, and `TranscriptCursor` do not exist.

- [ ] **Step 3: Write minimal production code**

Create `src/ui/scrollback.rs` with a cursor that advances only after its caller accepts a returned history range or stable line. It records committed stream text by byte content so repeated redraws cannot emit the same completed line. Export it from `src/ui/mod.rs` as `pub(crate) mod scrollback;`.

```rust
pub(crate) fn split_stable_rows(text: &str) -> (Vec<String>, String) {
    let Some(last_newline) = text.rfind('\n') else {
        return (Vec::new(), text.to_owned());
    };
    (
        text[..last_newline].split('\n').map(str::to_owned).collect(),
        text[last_newline + 1..].to_owned(),
    )
}
```

- [ ] **Step 4: Run test to verify GREEN**

Run: `cargo test ui::tests::split_stable_rows_keeps_only_the_incomplete_suffix_live && cargo test ui::tests::transcript_cursor_never_recommits_history_or_stream_rows`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/ui/scrollback.rs src/ui/mod.rs src/ui/tests.rs
git commit -m "feat(ui): track native scrollback commits"
```

### Task 2: Extract reusable message blocks and a live-only renderer

**Files:**
- Modify: `src/ui/mod.rs:1470-2696`
- Modify: `src/ui/tests.rs`

**Interfaces:**
- Produces `pub(crate) fn render_committed_history_block(state: &AppState, message_index: usize, width: u16) -> Vec<Line<'static>>`.
- Produces `pub(crate) fn render_live_tail(state: &AppState, width: u16) -> Vec<Line<'static>>`.
- `render_live_tail` includes only `state.current_response`'s incomplete suffix, the `Working...` line while active, queue preview, and composer-adjacent content; it never iterates historical messages.

- [ ] **Step 1: Write the failing renderer-scope test**

Add a test using two persisted messages plus a streaming response. The committed builder includes the selected persisted message; the live builder includes `Working...` and the unfinished suffix but not the older message.

```rust
#[test]
fn live_tail_excludes_committed_history() {
    let mut state = test_app_with_history("old completed answer");
    state.status = AppStatus::Streaming;
    state.current_response = "stable line\nunclosed tail".to_owned();

    let text = lines_to_text(&super::render_live_tail(&state, 80));

    assert!(text.contains("Working..."));
    assert!(text.contains("unclosed tail"));
    assert!(!text.contains("old completed answer"));
}
```

- [ ] **Step 2: Run test to verify RED**

Run: `cargo test ui::tests::live_tail_excludes_committed_history`

Expected: compilation fails because `render_live_tail` does not exist.

- [ ] **Step 3: Write minimal production code**

Extract the per-message branches inside `render_conversation` (system panels, tool cards, assistant/user Markdown, separators, and spacing) into a reusable block builder. Reuse `render_markdown`, `render_tool_result`, `render_status_panel`, and the existing tool-call context logic; do not redesign cards or Markdown styles. Build the live tail from `current_response` after `split_stable_rows`, adding literal `Working...` only for active model work.

- [ ] **Step 4: Replace the full-history frame renderer**

Change `render` so its chat section renders `render_live_tail` instead of `render_conversation`. Keep `render_queue_line` and `render_input` in the live frame. Delete `CHAT_CACHE`, full-transcript wrapping/scroll calculations, scroll-pill rendering, and copy-badge row mapping once no callers remain.

- [ ] **Step 5: Run tests to verify GREEN**

Run: `cargo test ui::tests`

Expected: PASS, including the new live-tail scope test and existing Markdown/tool rendering regressions adjusted only where they asserted removed scroll/copy controls.

- [ ] **Step 6: Commit**

```bash
git add src/ui/mod.rs src/ui/tests.rs
git commit -m "refactor(ui): render only the active chat tail"
```

### Task 3: Commit durable rows into the inline terminal's scrollback

**Files:**
- Modify: `src/main.rs:186-394`
- Modify: `src/ui/scrollback.rs`
- Modify: `src/ui/mod.rs`
- Modify: `src/ui/tests.rs`

**Interfaces:**
- Produces `pub(crate) fn pending_committed_blocks(state: &AppState, cursor: &mut TranscriptCursor, width: u16) -> Vec<Vec<Line<'static>>>`.
- `main` owns one `TranscriptCursor` and inserts each returned block via `terminal.insert_before(height, |buffer| ...)` before `terminal.draw`.
- The inline viewport is `Viewport::Inline(LIVE_VIEWPORT_ROWS)`, where `LIVE_VIEWPORT_ROWS` is a documented constant rather than terminal height.

- [ ] **Step 1: Write the failing pending-block test**

```rust
#[test]
fn pending_blocks_commit_history_then_stream_tail_once() {
    let mut cursor = TranscriptCursor::default();
    let mut state = test_app_with_history("completed");
    state.current_response = "stable\ntail".to_owned();

    assert!(!pending_committed_blocks(&state, &mut cursor, 80).is_empty());
    assert!(pending_committed_blocks(&state, &mut cursor, 80).is_empty());
    state.current_response.clear();
    state.history.push(ChatMessage::new("assistant", "stable\ntail"));
    assert_eq!(pending_committed_blocks(&state, &mut cursor, 80).len(), 1);
}
```

- [ ] **Step 2: Run test to verify RED**

Run: `cargo test ui::tests::pending_blocks_commit_history_then_stream_tail_once`

Expected: compilation failure because `pending_committed_blocks` is missing.

- [ ] **Step 3: Write minimal production code**

Render all newly finalized `AppState.history` entries with `render_committed_history_block`; render only newline-stable `current_response` rows while streaming. At completion, match the final assistant history entry to the committed stream prefix and commit just the remainder. Advance the cursor only after `Terminal::insert_before` succeeds. Render each `Line` into the supplied buffer with the existing line widget and use its wrapped line count as insertion height.

- [ ] **Step 4: Switch startup to a compact live viewport**

Replace `Viewport::Inline(terminal_height)` with a named cap:

```rust
const LIVE_VIEWPORT_ROWS: u16 = 12;
// ...
viewport: Viewport::Inline(LIVE_VIEWPORT_ROWS),
```

Before every normal `terminal.draw`, flush `pending_committed_blocks`; after a successful insertion redraw the live composer in its moved viewport. On resume, flush loaded visible history before the first draw.

- [ ] **Step 5: Run tests to verify GREEN**

Run: `cargo test ui::tests::pending_blocks_commit_history_then_stream_tail_once && cargo check --tests`

Expected: PASS and exit code 0.

- [ ] **Step 6: Commit**

```bash
git add src/main.rs src/ui/scrollback.rs src/ui/mod.rs src/ui/tests.rs
git commit -m "feat(terminal): commit chat history to scrollback"
```

### Task 4: Make the composer keyboard-only and remove obsolete UI paths

**Files:**
- Modify: `src/main.rs:1535-2059`
- Modify: `src/app/state.rs:764-929, 1500-1548, 1891-1908`
- Modify: `src/ui/mod.rs:808-1002, 2616-2696, 2794-2875`
- Modify: `src/ui/modals.rs`
- Modify: `src/ui/theme.rs`
- Modify: `src/app/suggestion.rs:146-154`
- Modify: `src/app/activity.rs`
- Modify: relevant unit tests in `src/app/state.rs` and `src/ui/tests.rs`

**Interfaces:**
- `AppState` no longer exposes transcript scroll, mouse hover, text-selection, code-copy, or runtime-theme-picker state.
- `render_live_controls` renders tool approval/question/simple picker content directly above `render_input` and receives keyboard focus before the composer.
- `/theme` is absent from `COMMANDS`; `ThinkingPicker` and theme-picker status/render/event branches are removed from normal interactive chat.

- [ ] **Step 1: Write failing cleanup and status tests**

Replace the hover test with tests that assert live status text is exactly `Working...` during streaming and the command list contains no `/theme` entry.

```rust
#[test]
fn streaming_status_uses_only_working_label() {
    let mut state = AppState::new();
    state.status = AppStatus::Streaming;
    assert_eq!(super::activity_status_label(&state), "Working...");
}

#[test]
fn command_palette_omits_runtime_theme_switching() {
    assert!(!COMMANDS.iter().any(|command| command.name == "/theme"));
}
```

- [ ] **Step 2: Run tests to verify RED**

Run: `cargo test streaming_status_uses_only_working_label && cargo test command_palette_omits_runtime_theme_switching`

Expected: status assertion fails (`Working`/another activity label) and `/theme` remains present.

- [ ] **Step 3: Remove mouse/transcript-scroll machinery**

Delete `HoverTarget`, scroll/copy/selection fields and helpers, hover tests, `PageUp`/`PageDown` transcript handlers, Ctrl-T mouse-capture toggle, and the `Event::Mouse` branch. Do not enable terminal mouse reporting. Preserve keyboard text editing, paste, Ctrl-C, escape cancellation, queue editing, and normal picker key handling.

- [ ] **Step 4: Inline temporary keyboard controls**

Replace full-frame modal calls with compact control blocks immediately above the composer. Reuse existing confirmation/question/picker content and key-state branches, but render no dimmed background, screen-sized `Clear`, or transcript overlay. Keep model, verbosity, protocol, command, history, and MCP flows keyboard-driven; remove unsupported theme and thinking chooser flows.

- [ ] **Step 5: Fix theme and activity surface**

Set the interactive palette once at startup to the default palette. Remove `/theme`, `show_theme_picker`, `render_theme_picker_modal`, and its key handling. Remove rotating `STREAMING_STATUS_WORDS` and activity-detail labels for model streaming; render exactly `Working...` while `Streaming` or `Queued`, retaining action-required labels only inside inline controls.

- [ ] **Step 6: Run tests to verify GREEN**

Run: `cargo test streaming_status_uses_only_working_label && cargo test command_palette_omits_runtime_theme_switching && cargo test ui::tests`

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add src/main.rs src/app/state.rs src/ui/mod.rs src/ui/modals.rs src/ui/theme.rs src/app/suggestion.rs src/app/activity.rs src/ui/tests.rs
git commit -m "refactor(ui): keep chat interactions keyboard-only"
```

### Task 5: Verify native-scrollback behavior and integration boundary

**Files:**
- Modify if needed: `src/main.rs`, `src/ui/scrollback.rs`, `src/ui/tests.rs`
- Inspect: `src/main.rs`, `src/app/state.rs`, `src/ui/mod.rs`, `src/ui/modals.rs`, `src/ui/theme.rs`, `src/app/suggestion.rs`

**Interfaces:**
- Normal interactive startup uses a compact inline viewport and `insert_before`; it has no full transcript frame or mouse input path.
- Resume is committed before the first composer frame.

- [ ] **Step 1: Add the resume-commit regression test**

Test the pending-block API with persisted user, assistant, tool, and visible system messages. Assert all visible entries appear in first-run blocks and a second call returns no blocks. Include one hidden system notice and assert it is not rendered.

- [ ] **Step 2: Run test to verify GREEN**

Run: `cargo test ui::tests::resume_history_is_committed_once`

Expected: PASS.

- [ ] **Step 3: Run required project verification**

Run:

```bash
cargo check --tests
cargo test
git diff --check
git status --short --branch
```

Expected: compilation and all tests pass, no whitespace errors, and only the approved scrollback implementation plus design/plan docs are changed on `feature/native-scrollback-chat`.

- [ ] **Step 4: Commit final verification adjustments**

```bash
git add src/main.rs src/app/state.rs src/ui docs/superpowers
git commit -m "test(ui): cover native scrollback chat"
```
