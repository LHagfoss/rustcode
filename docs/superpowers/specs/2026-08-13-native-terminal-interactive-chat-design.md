# Native Terminal Interactive Chat

## Goal

Remove terminal raw mode from normal `rustcode` interactive chat so the host terminal owns line editing and scrollback, while making `rustcode --resume` print the complete resumed transcript before accepting new input.

## Scope

- Route the normal interactive entrypoint through a standard stdin/stdout loop.
- Do not call crossterm raw-mode, alternate-screen, mouse-capture, or ratatui event-loop setup for normal chat.
- Render resumed user, assistant, tool, and meaningful system messages as append-only terminal output.
- Keep `--prompt`, ACP, session persistence, tool execution, and existing unrelated worktree changes intact.
- Preserve `/exit`, `/quit`, and `/clear` in the interactive loop.

## Design

`main` will perform argument handling and delegate normal interactive execution to `raw_cli::run_interactive_cli`. That function will own a shared `AppState`, load the latest session when requested, print its transcript using a small deterministic formatter, then read one line at a time from stdin. Each submitted line is appended to history, executed through the existing agent-turn pipeline, and persisted.

The formatter will avoid TUI-only layout and scrolling state. It will print complete message content, including tool result content, so the terminal scrollback contains the full resumed conversation. Hidden internal system notices remain hidden; user and assistant content remain visible.

## Verification

- Add unit tests for transcript formatting, including multiple messages and tool output.
- Verify the focused tests fail before implementation and pass afterward.
- Run `cargo check --tests` and `cargo test`.
- Confirm the final diff does not include unrelated dirty worktree changes.
