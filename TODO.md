# Rustcode agent harness TODO

## Status after PRs #121–#132

The core correctness, safety, and architectural backlog is implemented and
merged into `main` through PR #134: skill execution barriers and explicit
execution paths, tool-call validation, structured tool-result metadata,
centralized authorization, read-only scheduling, shared turn-runner policy,
semantic subagent loop guards, explicit subagent write contracts, typed history
normalization, response provenance, and isolated writable-subagent worktrees
with review manifests.

The detailed entries below are retained as design documentation and regression
ideas; they are no longer an unimplemented blocker list.

## Current status (2026-07-30)

Completed in the latest feature workflow:

- Structured tool-result metadata is persisted in history (PR #143).
- Native OpenAI-compatible tool arguments fail closed when malformed, while
  preserving provider error context (PR #144).
- Metadata-only operational lifecycle events are written to `debug.log` (PR
  #145).
- Safe dead code, unreachable input handling, and compiler warnings were
  removed (PR #146).
- `complete_task` summaries now include harness-derived build status and
  changed paths (PR #147).

Still worth considering, but not required for the current harness baseline:

- Extract the remaining side effects from the typed turn state machine into
  smaller adapters.
- Unify the remaining TUI/raw CLI/subagent history and approval adapters.
- Make compaction explicitly model/budget aware.
- Decide whether to expose a user-facing observability view beyond
  `~/.config/rustcode/debug.log`.
- Add deeper provider-specific adapters if Gemini or local servers require
  behavior beyond the OpenAI-compatible protocol.

This is the backlog after the initial Codex-inspired harness refactor. The
repository currently has typed tool calls/events, bounded context fragments,
semantic loop guards, explicit tool capabilities, opt-in delegation, and
subagent lifecycle tracking. The remaining work below is intentionally split
into small feature branches.

## P0 — fix incorrect tool sequencing and skill use

### 1. ✅ Make skill loading a hard execution barrier

Observed in session `~/.config/rustcode/sessions/1785360941941/history.json`:

- User asked to lower Spotify volume to 3%.
- The assistant emitted four duplicated `use_skill` calls and two command
  calls in one response.
- The command used `osascript`, even though the Spotify skill explicitly says
  to use `spotify-cli`.
- The harness executed the command before the skill result could affect the
  next model decision.

Required changes:

- Detect `use_skill` calls as control-plane calls.
- Execute one skill call by itself and immediately resume sampling.
- Do not execute later calls from the same model response after a skill call.
- Deduplicate identical tool calls within one response.
- Make the loaded skill content part of the next request context.
- Add a test proving this sequence:

  ```text
  model emits use_skill + run_command
    -> execute only use_skill
    -> return skill content
    -> resample
    -> allow run_command only if the new response requests it
  ```

Acceptance criteria:

- A skill-defined command cannot be bypassed by an earlier response.
- Repeated `use_skill` calls do not execute repeatedly.
- The Spotify case uses `spotify-cli`, not an invented alternative.

### 2. ✅ Add tool-call response validation

The harness currently trusts multiple parsed calls from one text response.
Before execution, validate the batch:

- reject duplicate calls with identical name/arguments
- reject control-plane plus side-effect calls in the same response
- reject calls not advertised in the current tool registry
- reject malformed arguments before execution
- preserve the raw response for diagnostics

### 3. ✅ Stop claiming unavailable tools exist

Observed in the same session:

- Calendar skill requested `manage-apple-calendar`.
- The tool failed with `unknown tool` because only `use_skill` and generic
  tools were available.
- The assistant then incorrectly claimed the terminal was unavailable,
  although `run_command` was available.

Required changes:

- Skills must declare whether they are instruction-only or backed by a native
  tool.
- `use_skill` must return available execution paths explicitly.
- Prompt/tool inventory must contain only tools actually registered.
- The agent must never claim a tool is unavailable unless the harness has
  returned that exact availability state.
- If a skill says to use a CLI and `run_command` exists, the agent should use
  the CLI rather than inventing a missing native tool.

## P0 — make the harness less “stupid”

### 4. ✅ Replace text-protocol parsing with structured provider calls

The JSON/fenced parser is still the main path for some providers. It permits
multiple tool blocks, prose mixed with calls, duplicated calls, and accidental
tool-like content. Keep the parser for compatibility, but prefer structured
API-native tool calls whenever supported.

Tasks:

- Define one normalized `ModelResponse` type.
- Convert provider-native tool calls into `ToolCall` directly.
- Keep text parsing as an explicit fallback protocol.
- Record parse source: native, fenced, tag, or repaired JSON.
- Make parser repair visible in the tool result and logs.

### 5. ✅ Add a model-response quality gate

Before executing a tool call, check:

- Does the call match an advertised schema?
- Is the tool allowed in the current mode?
- Is the call appropriate for the current task state?
- Did the previous tool result already answer this request?
- Is this an exact or semantic loop?
- Does a skill require a particular command/workflow?

The gate should return a structured rejection reason that is fed back to the
model instead of silently executing a bad call.

### 6. ✅ Make tool results authoritative and structured

`ToolResult` exists, but history still serializes many results into strings.
Add fields for:

- call ID
- tool name
- arguments hash
- success/error status
- exit code when applicable
- changed paths
- diff metadata
- truncation metadata
- full-output artifact path

This will improve planning, loop detection, compaction, and final reporting.

## P1 — finish the event-driven loop

### 7. ✅ Replace implicit branches with a real `TurnState`

Current state-machine work has `AgentEvent` and `TurnAction`, but the large
orchestrator still owns most control flow in `src/network.rs`.

Extract:

- `TurnState`
- `TurnInput`
- `TurnOutput`
- `ContinuationReason`
- `TerminalReason`
- context rollover action
- approval-wait action
- tool execution action
- final-response action

Each transition should be pure where possible and unit tested.

### 8. ✅ Remove duplicated loops

The TUI, raw CLI, and subagent paths still have separate streaming/continuation
logic. Move them onto one shared turn runner with adapters for:

- UI updates
- approval prompts
- history persistence
- subagent status

The raw CLI no longer has `MAX_ROUNDS`, but the subagent path still has a
fixed per-agent round guard. Keep depth and loop guards, but make them part of
the shared policy rather than unrelated local loops.

## P1 — context and compaction

### 9. ✅ Introduce typed history entries

Replace role/content-only history with explicit variants:

- user message
- assistant text
- tool call
- tool result
- system instruction
- context fragment
- compaction summary
- lifecycle event

Enforce call/result pairing during normalization.

### 10. Make compaction policy model-aware

- Track input and output budgets separately.
- Preserve the current user request and active edit context.
- Preserve tool call/result pairs.
- Prefer pruning old throwaway output before source-file content.
- Emit a compaction event and summary.
- Test repeated compaction and rollover behavior.

## P1 — tools and execution

### 11. ✅ Implement capability-aware scheduling

`ToolSafety` metadata now exists, but execution is still sequential.

Implement:

- parallel execution only when every call is `ReadOnly`
- exclusive lock for mutations/process control/interactive tools
- cancellation propagation to all in-flight read calls
- deterministic result ordering
- no parallel execution for unknown/MCP tools unless explicitly opted in
- tests for mixed read/write batches

### 12. ✅ Centralize approval policy

Move plan-mode checks, confirmation checks, and tool capabilities into one
authorization function. It should return a typed decision:

```text
Allow
RequireConfirmation
Deny(reason)
```

No tool should independently invent approval behavior.

## P1 — subagents

### 13. ✅ Add isolated workspaces for writing subagents

Lifecycle states and active-agent limits now exist. Next add:

- read-only default subagents
- explicit write permission
- isolated worktree or scoped workspace for write tasks
- changed-path manifest
- parent review before merge into the main workspace
- cancellation and timeout propagation

### 14. ✅ Make delegation task contracts explicit

Every delegated task should include:

- objective
- allowed paths
- forbidden paths
- expected artifact/result
- verification command
- completion condition

The parent must reject vague or overlapping delegation requests.

## P2 — prompt and product quality

### 15. Separate stable prompt policy from runtime state

Keep the base prompt stable for caching. Inject runtime state as typed,
bounded fragments. Avoid repeating reminders every turn unless a state change
requires one.

### 16. ✅ Add “why this tool” and “what changed” observability

Debug logs should record:

- selected skill and reason
- tool authorization decision
- parsed call source
- loop-detector signal
- context token estimate
- compaction action
- subagent lifecycle transition
- final terminal reason

Do not expose hidden chain-of-thought; record concise operational metadata only.

### 17. ✅ Improve final-answer grounding

The final response should be generated from verified state:

- changed files from the workspace
- test/check results from tools
- unresolved errors
- subagent reports marked advisory until inspected
- explicit blocker if verification was not possible

## Verification requirements for each future PR

- focused unit tests for the changed boundary
- `cargo test` before the final architecture PR
- `git diff --check`
- no unrelated formatter churn
- feature branch, PR, merge, checkout `main`, and `git pull`
- update this TODO as items are completed
