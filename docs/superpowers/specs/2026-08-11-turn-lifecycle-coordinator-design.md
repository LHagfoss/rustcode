# Turn Lifecycle Coordinator Design

## Problem

The agent loop already shares `run_agent_turn` between the interactive queue and raw CLI, but terminal behavior is distributed across `run_single_turn`, budget handling, provider-error handling, cancellation branches, completion-gate branches, and the final wrapper. Those paths write history, set `AppStatus::Idle`, persist sessions, and notify completion independently. `TurnContext.stop_reason` is also an untyped string, so benchmark output cannot reliably distinguish normal completion from cancellation, failed recovery, loop escalation, provider failure, or unavailable tools.

## Goals

- Keep interactive and raw CLI execution on the same lifecycle coordinator.
- Represent terminal outcomes with a typed `StopReason` while preserving stable benchmark strings.
- Ensure every terminal path has a reason and a coherent transcript.
- Make final history persistence, idle transition, usage accounting, and completion notification one idempotent operation.
- Preserve the existing `TurnMachine`, tool execution, finish gates, recovery behavior, and public benchmark fields.

## Non-goals

- Rewriting the model request/streaming layer.
- Changing tool approval policy or the interactive UI.
- Adding a second orchestration loop for raw CLI.
- Changing existing stop-reason JSON strings unless required to represent one of the explicit typed variants.

## Design

### Typed stop reasons

Add a `StopReason` enum in the network lifecycle module:

```rust
pub(crate) enum StopReason {
    Completed,
    Cancelled,
    RecoveryFailed,
    LoopEscalation,
    ProviderError(Option<u16>),
    UnavailableTool,
    BudgetExceeded(String),
}
```

`Display` maps variants to the existing stable strings where they already exist: `completed`, `cancelled`, `provider_error`, `provider_error:429`, and `budget:<limit>`. New explicit values are `recovery_failed`, `loop_escalation`, and `unavailable_tool`. `TurnContext` stores `Option<StopReason>`, while `benchmark_summary()` serializes it through `Display` so current consumers remain string-based.

Provider error parsing keeps the existing 429 counter and maps the first provider failure to `ProviderError(Some(429))`; other provider failures map to `ProviderError(None)`. Budget stops remain distinct detail-bearing outcomes. A successful `complete_task` sets `Completed`. Cancellation always takes precedence over a budget or provider outcome when the cancellation token is set.

### Lifecycle coordinator

Create `src/network/lifecycle.rs` with a small `TurnLifecycle` coordinator. It owns only terminal bookkeeping, not model requests or tools:

```rust
pub(crate) struct TurnLifecycle {
    finalized: bool,
}

impl TurnLifecycle {
    pub(crate) fn new() -> Self;
    pub(crate) fn mark_finalized(&mut self) -> bool;
}
```

The coordinator exposes the pure stop-reason formatting and the idempotence guard. `run_agent_turn` uses it for its single finalization block, which always:

1. assign a fallback `StopReason` if no branch assigned one;
2. append the final assistant/system transcript when needed, including empty-output and cancellation summaries;
3. attach response time and token usage;
4. save session history and flush the queued history snapshot;
5. set continuous mode off, status idle, clear the current response, and request redraw;
6. track usage and send exactly one completion/cancellation notification.

The existing completion-task branch may append its task summary before returning. The coordinator recognizes `ctx.task_completed` and does not append a duplicate assistant message, but it still performs the remaining finalization work once.

### Terminal path mapping

- `complete_task` accepted by the finish gate: `Completed`.
- User cancellation or cancelled stream/tool execution: `Cancelled`.
- Bounded recovery/replan that cannot continue: `RecoveryFailed`.
- Forced final after loop detection or a safety loop escalation: `LoopEscalation`.
- Provider/request failure: `ProviderError(...)`.
- A terminal response rejected because the requested tool is not in the registry: `UnavailableTool`.
- Safety budgets: `BudgetExceeded(limit)`.

Recoverable errors and finish-gate retries do not finalize the turn and therefore do not receive a terminal stop reason yet. If the loop exits without a more specific reason, the coordinator uses `RecoveryFailed` instead of the current untyped `stopped` fallback.

### Transcript consistency

Every terminal path reaches `run_agent_turn` finalization. If there is no final model text, the coordinator records a concise system marker containing the stop reason; if tool calls were interrupted, the existing unanswered-call result records remain authoritative. This prevents a cancellation, provider error, or empty response from leaving the session with only an idle UI state and no durable explanation.

## Testing

- Unit-test `StopReason` display/serialization for all variants, including provider 429 and budget detail.
- Unit-test `TurnLifecycle` finalization guard to prove the second call is a no-op.
- Extend `TurnContext` tests so benchmark summaries use typed stop reasons.
- Test terminal mapping helpers for cancellation precedence, provider errors, loop escalation, recovery failure, and unavailable tool validation.
- Keep existing `TurnMachine` tests and the full suite green.
- Verify `cargo check --tests`, `cargo test`, and `git diff --check` before delivery.

## Self-review

- The coordinator is limited to terminal bookkeeping and does not duplicate the model/tool loop.
- Interactive and raw CLI already call `run_agent_turn`; no second path is introduced.
- All required explicit stop reasons have a mapping and a test target.
- Existing benchmark-compatible strings are retained where present.
- Empty and cancelled responses now have a durable transcript marker.
- Finalization is guarded against duplicate assistant messages, idle transitions, persistence, and notifications.
