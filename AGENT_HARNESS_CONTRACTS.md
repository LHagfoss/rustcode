# Agent harness contracts

This document describes behavior enforced by rustcode's implementation and
tests. Guidance shown in a model prompt is not treated as enforcement unless a
tool, policy, state machine, or test also implements it.

## Turn and message contract

The network layer normalizes a provider response into typed `AgentEvent` values
before orchestration. A response can contain text, structured tool calls, a
finish reason, cancellation, or an error. The source is recorded as native,
fenced, tagged, repaired JSON, or plain text by
[`src/network/events.rs`](src/network/events.rs).

`TurnMachine` is the lifecycle guard. The normal path is:

```text
AwaitingModel -> AwaitingApproval -> ExecutingTools -> AwaitingModel
AwaitingModel -> Completed
```

Cancellation and recoverable provider errors have explicit transitions.
Abandoned tool phases rewind both pending approval and executing tools, which
keeps loop recovery and forced wrap-up responses legal. The transition table
and invalid-transition tests live in
[`src/network/events.rs`](src/network/events.rs).

The orchestrator stores assistant tool calls and one structured result per
call. Interrupted, rejected, truncated, or otherwise unanswered calls receive
an explicit tool error result so the next provider request cannot infer a
result that never happened. See `unanswered_call_results` and the history tests
in [`src/network.rs`](src/network.rs) and
[`src/network/history.rs`](src/network/history.rs).

## Tool and result contract

Tool calls are validated against the registered schema before execution. The
batch is truncated to the executor's limit rather than allowing the model's
prose to describe results for calls that did not run. Control-plane calls such
as `use_skill` are isolated from workspace calls.

Every execution produces authoritative `ToolResultMetadata` alongside display
text. The metadata records success, exit code, changed paths, truncation,
recovery artifacts, and argument identity. The UI and finish gates use this
metadata; they do not infer success from text such as “done”. The types and
executor are in [`src/network/events.rs`](src/network/events.rs) and
[`src/network.rs`](src/network.rs).

Real workspace progress means a mutating result reports a real change. Failed
edits and idempotent “already applied” results are not progress, do not reset
loop detection, and count toward safety budgets. The regression tests for this
contract are named `mutation_made_progress_*` in
[`src/network.rs`](src/network.rs).

## Provider protocol contract

The active provider profile selects the tool protocol deterministically. API
native profiles receive structured tool schemas and preserve provider call IDs;
JSON/fenced/tagged text providers go through the text adapter. The normal
orchestration path consumes the same typed events after normalization, so text
syntax is not executed directly by the tool runner.

Protocol selection and capability probing are implemented in
[`src/app/state.rs`](src/app/state.rs) and [`src/network.rs`](src/network.rs).
The protocol and alignment tests are in
[`src/network/events.rs`](src/network/events.rs),
[`src/network/history.rs`](src/network/history.rs), and the `protocol_tests`
module in [`src/app/state.rs`](src/app/state.rs).

## Recovery and stop contract

The harness distinguishes normal completion from safety stops. Current machine-
readable stop reasons include:

- `completed` or `stopped` for ordinary turn finalization;
- `cancelled` for user cancellation;
- `provider_error` and `provider_error:429` for provider failures;
- `budget:<limit>` for a semantic or configured budget stop;
- `failure_replan` when equivalent failed mutations require a new plan.

The configured `max_tool_rounds` value is a final backstop, not the primary
loop strategy. Semantic guards run first: repeated calls, unchanged output,
failed mutations, no-progress mutations, compiler diagnostics, malformed
calls, and verification failures each have their own handling. Budget stops
leave the task explicitly incomplete and persist the transcript. See
[`src/network.rs`](src/network.rs) and
[`src/network/loop_detect.rs`](src/network/loop_detect.rs).

Two equivalent failed mutations are tracked separately from ordinary call
repetition and trigger a concise replan message. The message states that those
attempts changed no files, prohibits retrying the same edit, and asks for a
user decision when safe progress is not possible. A successful mutation resets
the relevant failure streak. The regression test is
`equivalent_failed_mutations_escalate_and_progress_resets_them` in
[`src/network/loop_detect.rs`](src/network/loop_detect.rs).

Completion is also gated: a `complete_task` claim cannot silently pass when
all attempted edits failed, when fresh verification is missing, or when the
build has known errors. The gate is implemented in
[`src/network.rs`](src/network.rs) and covered by the completion, verification,
and compiler-diagnostic tests there.

## Git safety contract

Tool authorization is centralized in [`src/tools/mod.rs`](src/tools/mod.rs).
Unknown tools and registered tools marked as requiring confirmation do not run
silently in interactive mode.

`run_command` uses conservative command-aware authorization. Explicitly
allowlisted read-only inspection such as Git `status`, `diff`, `log`, `show`,
and `rev-parse`, supported `gh` list/view/status commands, and search commands
is non-blocking. Unknown command families, shell redirection, and potentially
mutating segments require explicit confirmation. Each shell segment is
inspected independently; `restore`, path checkout, `reset`, `clean`, branch
deletion, and force operations retain named destructive scopes in the
confirmation preview.
The classifier and regression tests are in
[`src/tools/exec.rs`](src/tools/exec.rs), with interactive policy wiring in
[`src/network/policy.rs`](src/network/policy.rs).

This is a runtime safety rule. The system prompt also tells the model to keep
destructive operations inspectable, but that prompt text is not the security
boundary.

## Subagent contract

Subagents are opt-in at the prompt level and constrained by executable checks:

- subagents are read-only unless `write_access` is explicitly requested;
- write-enabled subagents receive an explicit `allowed_paths` contract;
- paths outside that contract are rejected;
- subagents cannot spawn or message other subagents;
- subagent loops use the same repeat detector and stop after repeated actions;
- subagent reports are advisory and the main agent must inspect the workspace.

The enforcement path is in [`src/network.rs`](src/network.rs), while the
prompt wording is assembled in [`src/tools/mod.rs`](src/tools/mod.rs). The
prompt is therefore documentation of the executable policy, not its substitute.

## Phase checkpoints, metrics, and persistence

`todo_write` updates the active phase checkpoint. The checkpoint is persisted
as a model-directed system note in session history, allowing a resumed run to
recover the current phase without rereading the entire transcript.

Each agent turn emits a `turn.summary` operational event and includes metrics
such as tool rounds, tool calls, tokens, malformed calls, no-progress results,
failure replans, compiler-diagnostic streak, provider errors, changed paths,
phase checkpoint, and stop reason. `turn.finish` carries the same summary when
there is a final assistant response. The implementation is in
[`src/network.rs`](src/network.rs); summary and checkpoint regressions are
covered by the `benchmark_summary_*` and `active_todo_*` tests there.

## Offline replay and benchmark workflow

The deterministic replay fixtures use scripted model steps and the real local
tool executor in temporary workspaces. They do not open a provider socket.
The failed fixture covers failed edits, state restore, repeated reads, loop
warnings, result pairing, bounded recovery, forced termination, lifecycle
states, and workspace safety. The successful fixture covers a multi-step edit,
verification read, real changed paths, and completion. Both are in
[`src/network.rs`](src/network.rs) as `*_session_replay_*` tests.

Repository work is intentionally separate from runtime behavior: for a code
change, create a `feature/...` or `fix/...` branch from `main`, verify it,
commit, push, open a PR, merge into `main`, then pull `main`. This is the
repository workflow documented in `AGENTS.md`; it is not a permission granted
to the model by a prompt.
