# Edit Diff Rendering After Session Reload

## Goal

Ensure Edit tool results still render their embedded diff when the preceding assistant tool-call message is unavailable after session reload.

## Design

The transcript renderer will resolve the tool name from the persisted `tool_result.tool_name` metadata, falling back to the existing content prefix when necessary. It will then pass the result through the existing structured tool-result renderer. Live `ChatMessage.diff` behavior remains unchanged, and no additional diff data is persisted.

## Acceptance criteria

- A persisted Edit result renders its diff without a parseable preceding assistant tool call.
- Existing live Edit and Write rendering remains unchanged.
- The regression is covered by a UI unit test.
