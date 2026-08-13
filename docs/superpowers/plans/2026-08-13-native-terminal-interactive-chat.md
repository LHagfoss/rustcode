# Native Terminal Interactive Chat Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make normal interactive rustcode chat use standard terminal input/output with native scrollback and print the complete resumed transcript.

**Architecture:** `main` will delegate the normal interactive path to the existing shared headless turn runner in `raw_cli.rs`. A pure transcript formatter in that module will convert persisted `ChatMessage` values to append-only terminal text; the REPL will print the loaded transcript, accept one line per prompt, execute the existing agent turn, and persist the resulting session.

**Tech Stack:** Rust 2024, Tokio, crossterm/ratatui retained for existing UI modules but not initialized by normal interactive chat, existing `AppState` and `run_agent_turn` lifecycle.

## Global Constraints

- Do not call `enable_raw_mode` or `disable_raw_mode` for normal interactive chat.
- Do not enter an alternate screen or enable mouse capture for normal interactive chat.
- Preserve `--prompt`, ACP, session persistence, tool execution, and unrelated dirty worktree changes.
- Use native terminal scrollback by appending output; do not emulate scrolling in the new path.
- Verify with `cargo check --tests` and `cargo test`.

---

### Task 1: Add the transcript regression test

**Files:**
- Modify: `src/raw_cli.rs` in the existing unit-test module

**Interfaces:**
- Consumes: `ChatMessage::new` and the new transcript-formatting function signature `format_history_for_terminal(&[ChatMessage]) -> String`.
- Produces: A failing test that specifies visible user, assistant, tool, and system transcript output.

- [ ] **Step 1: Write the failing test**

Add a test that builds a four-message history and asserts the formatter includes each visible message and tool output, with the user prompt marked by `❯` and assistant output marked by `Assistant:`.

```rust
#[test]
fn formats_complete_history_for_native_terminal_scrollback() {
    let history = vec![
        ChatMessage::new("user", "inspect the project"),
        ChatMessage::new("assistant", "I found the project."),
        ChatMessage::new("tool", "run_command: cargo check"),
        ChatMessage::new("system", "Workspace is clean"),
    ];

    let rendered = format_history_for_terminal(&history);

    assert!(rendered.contains("❯ inspect the project"));
    assert!(rendered.contains("Assistant: I found the project."));
    assert!(rendered.contains("● run_command: cargo check"));
    assert!(rendered.contains("Workspace is clean"));
}
```

- [ ] **Step 2: Run the focused test and verify it fails**

Run: `cargo test raw_cli::tests::formats_complete_history_for_native_terminal_scrollback`

Expected: compilation failure because `format_history_for_terminal` does not exist yet.

### Task 2: Implement native transcript formatting and wire the no-raw interactive path

**Files:**
- Modify: `src/raw_cli.rs` near `run_interactive_cli`
- Modify: `src/main.rs` at the normal interactive startup branch

**Interfaces:**
- Consumes: `AppState`, `ChatMessage`, `HeadlessPolicy`, `run_agent_turn`, and `resume_latest_session`.
- Produces: `format_history_for_terminal(&[ChatMessage]) -> String`; normal `main` execution delegates to `run_interactive_cli` without initializing crossterm raw mode or ratatui.

- [ ] **Step 1: Implement the smallest formatter**

Format each message as append-only text:

- `user`: `❯ ` followed by complete content.
- `assistant`: `Assistant: ` followed by complete content.
- `tool`: `● ` followed by complete content.
- meaningful `system`: complete content.
- hidden internal notices and empty messages: omit.

Separate messages with a blank line and return an empty string for no visible messages.

- [ ] **Step 2: Run the focused test and verify it passes**

Run: `cargo test raw_cli::tests::formats_complete_history_for_native_terminal_scrollback`

Expected: PASS.

- [ ] **Step 3: Delegate normal interactive startup before raw-mode setup**

In `main`, after the existing `--prompt` branch, call:

```rust
raw_cli::run_interactive_cli(model_override.as_deref(), cli_args.resume || cli_args.continue_session).await?;
crate::config::flush_history();
return Ok(());
```

This keeps all existing raw-mode/TUI code unreachable from normal chat while preserving it for compilation and minimizing unrelated deletion.

- [ ] **Step 4: Print the full resumed transcript**

In `run_interactive_cli`, replace the resume count-only message with the formatter output, then print the normal prompt. Print each submitted user prompt and the resulting stored assistant/tool history through the same append-only path after the agent turn completes. Keep `/exit`, `/quit`, and `/clear` behavior.

- [ ] **Step 5: Run focused tests and compile checks**

Run: `cargo test raw_cli::tests`

Expected: all raw CLI tests pass.

Run: `cargo check --tests`

Expected: exit code 0.

### Task 3: Full verification and diff review

**Files:**
- Inspect: `src/main.rs`, `src/raw_cli.rs`, and the design/plan docs

- [ ] **Step 1: Run the complete test suite**

Run: `cargo test`

Expected: all tests pass with zero failures.

- [ ] **Step 2: Check the diff boundary**

Run: `git diff --check` and `git diff --stat -- src/main.rs src/raw_cli.rs docs/superpowers`

Expected: no whitespace errors; only the focused implementation and approved docs are shown in the branch diff. Existing unrelated dirty files remain unmodified by this task.

- [ ] **Step 3: Recheck raw-mode calls in the normal entrypoint**

Run: `rg -n "enable_raw_mode|disable_raw_mode|EnterAlternateScreen|EnableMouseCapture" src/main.rs src/raw_cli.rs`

Expected: no calls are reachable from the normal interactive branch; the delegated branch returns before the old TUI setup.
