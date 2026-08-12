# Conditional Tool-Result Transcript Spacing

## Goal

Improve transcript readability by inserting one blank row between a completed tool-result block and the next visible assistant block, including assistant thinking previews, while preserving transcript history when starting or clearing a chat view.

## Design

The conversation renderer will inspect the next visible history message after each tool result. Hidden system notices are skipped. If that message is an assistant message, the renderer adds exactly one trailing blank row to the tool-result block. Existing behavior remains unchanged for consecutive tool results and for tool results followed by user messages, whose turn separator logic already owns that spacing.

The app will retain the full `AppState.history` for model requests. `/clear` records the current history length as a transient UI display boundary, so existing messages are hidden without being deleted while later messages remain visible. `/new` preserves the existing history and resets the display boundary to show the full transcript; `/resume` does the same. The conversation cache key includes the display boundary.

The change uses the existing `ChatMessage` history representation and line-rendering conventions. Focused tests will cover spacing transitions and the `/new`/`/clear` history-preservation behavior.

## Acceptance Criteria

- Tool result followed directly by assistant: one blank row is inserted.
- Tool result followed by hidden system notice and assistant: one blank row is inserted.
- Tool result followed by user: existing user separator behavior is unchanged.
- Consecutive tool results remain compact.
- `cargo check --tests` and `cargo test` pass.
- `/new` does not delete `AppState.history` and displays the full retained transcript.
- `/clear` hides only the current transcript view, retains `AppState.history`, and allows later messages to render.
- Reopening/resuming a session displays the full retained transcript.
