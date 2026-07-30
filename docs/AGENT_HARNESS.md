# Agent harness architecture

This document describes the current boundaries of the agent runtime and the
direction for future harness work.

## Turn flow

```text
user input
  -> context/history preparation
  -> provider streaming request
  -> typed response classification
  -> TurnMachine transitions
  -> tool execution and ToolResult records
  -> history update
  -> next turn or terminal response
```

The response and turn boundaries live in `src/network/events.rs`. The
orchestrator uses `TurnMachine` to validate state transitions (e.g. streaming,
waiting for tool approval, executing tools, or recovering from errors) and 
ensure consistent turn lifecycle guarantees.

The turn execution loop is encapsulated by the `run_single_turn` helper in `src/network.rs`.
This runner executes a single model interaction and handles streaming, history, and
tool dispatch consistently across both the graphical UI and the raw CLI.

## Policies and Gates

Different environments have different requirements for user interaction. `src/network/policy.rs`
defines a `TurnPolicy` trait for abstracting approval and validation logic:

- `InteractivePolicy` (UI): Prompts the user before executing dangerous tool calls, and 
  verifies that the compilation state is green before accepting task completion.
- `HeadlessPolicy` (Raw CLI): Auto-approves tool execution (unless mutating tools are used 
  in read-only `plan_mode`), prioritizing uninterrupted execution without user interaction.

## Context and history

`src/network/history.rs` owns conversion from internal `ChatMessage` values to
provider messages. Tool results are represented as user-context messages and
multimodal user content is converted consistently for the raw CLI and TUI.

Dynamic context is assembled from bounded `ContextFragment` values. Individual
fragments and the complete context tail have size limits so environment data,
file lists, and task plans cannot grow without bound.

## Tools

Tool calls use the typed `ToolCall` record in `src/tools/mod.rs`. Each tool has
a `ToolSafety` capability:

- `ReadOnly` may be parallelized in a future scheduler.
- `WorkspaceMutation`, `ProcessControl`, `Interactive`, and `Delegation` are
  serialized by default.
- Unknown tools are conservative and are never treated as parallel-safe.

The current executor remains sequential, which preserves tool ordering while
the capability metadata is adopted by a scheduler.

## Safety and stopping

The raw CLI has no fixed task-round limit. It stops on a final response,
cancellation, retry exhaustion, malformed-call retry exhaustion, or semantic
loop detection. The main orchestrator retains compiler, cancellation, context,
and loop guards.

Subagents require explicit delegation, have lifecycle states, are limited to
four active agents, cannot recursively spawn agents, and reject follow-ups to
failed or cancelled agents.

## Extension rules

When adding a harness feature:

1. Add or update characterization tests first.
2. Put state transitions in a pure, typed module where possible.
3. Keep provider parsing separate from tool execution.
4. Treat unknown tools as unsafe by default.
5. Preserve cancellation and loop guards when changing continuation logic.
6. Merge one focused feature branch before starting the next.
