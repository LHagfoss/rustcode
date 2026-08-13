# Tool Transcript Hierarchy Design

## Goal

Make completed tool calls readable as compact transcript blocks while retaining
the native terminal scrollback architecture.

## Design

Each visible tool result gets three layers:

```text
• Bash · cargo test
  └ ✓ exit 0
    │ 504 passed
```

The action and target come from the corresponding preceding tool-call record,
using the existing `format_pi_tool_action` formatter. The result record is the
authoritative source for success and exit-code state. When that metadata is
absent (older persisted history), command result text provides a best-effort
exit-code fallback.

Tool result rendering continues to own the tool-specific output body. The
committed-history renderer composes the header and status around that body and
indents it by one level. High-verbosity mode and control-plane results remain
hidden, matching current behavior.

## Non-goals

- No tool execution, persistence, or protocol changes.
- No changes to scrollback ownership, resize behavior, or input handling.
- No new mouse interaction, copy controls, themes, or modal UI.
