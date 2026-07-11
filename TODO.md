# rustcode

## stuff

- [x] pressing shift + cambiamenti in the future tool calls".
- [x] accept tool call should insta close modal, currently wait command to finish
- [x] press ESC or Deny on "tool modal", should stop or purge agent, cancel it.
- [x] pressing ESC normally in chat should insta stop everything or like it stops but takes a while (bad ux), it needs visual feedback it stopped.
- [x] cmd + v doesnt paste (currently ctrl + v)
- [x] Thought: in ms, can be in seconds if more than like 1000ms

## Tool Optimizations (Claude Code style)

- [x] Improve `read_file`: Support multi-range reads to reduce turn count.
- [x] Improve `edit`: Move from exact string replacement to a more robust line-based or block-replacement system (less fragile than `old_string`).
- [x] Implement Symbol Search: Add indexing for functions/classes to avoid brute-force grepping paths.
- [x] Shell Output: Review and optimize output capping to ensure critical logs aren't lost while preventing context overflow.
- [x] Implement Tokens/s display in footer next to "Context Used". Format: `Tokens/s: (n)` — shows real-time speed of streamed tokens per second during assistant replies.

## Tool Modal & Nonblocking Execution Fixes (branch `fix/tool-modal-nonblocking`)

- [x] Fix Keypress Blocking: Set status to `Streaming` or `ExecutingTool` when tool starts so keypresses aren't blocked in `main.rs` while the tool runs.
- [x] Wire up `AppState::tool_running`: Correctly update `tool_running` to `true` while the tool executes, and `false` after it completes.
- [x] Visual Indicator for Running Tools: Update UI (footer / message area) to show when a tool is running (e.g. spinner or "Executing <tool_name>...").
- [x] Immediate Cancellation: Ensure pressing ESC immediately cancels the agent/orchestrator loop without waiting for the blocked tool execution to finish (using `tokio::select!` or cancel token checks.
