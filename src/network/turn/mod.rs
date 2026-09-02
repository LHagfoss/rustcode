mod context;
mod finish;
mod queue;
mod recovery;
mod request;
mod tools;

pub use context::TurnContext;
pub use finish::run_agent_turn;
pub(crate) use finish::run_agent_turn_with_context;
pub use queue::process_queue_orchestrator;
pub(crate) use queue::process_queue_orchestrator_with_ui_events;
use recovery::reasoning_loop_final_response;
pub(crate) use recovery::record_malformed_call;
use request::messages_for_response_continuation;

use crate::app::{AppState, ChatMessage};
use std::sync::Arc;
use tokio::sync::Mutex;

use super::events::ToolResult;
use super::lifecycle;
use super::policy;
use super::stream::StreamBuffer;
use super::tool_exec::tool_result_history_message;
use super::verification;
use super::{stop_turn_for_budget, turn_budget_exceeded, unanswered_call_results_with_kind};

pub(crate) fn take_turn_context_for_prompt(
    state: &mut AppState,
    is_wakeup: bool,
    max_tool_rounds: usize,
) -> TurnContext {
    if is_wakeup {
        state
            .background_turn_context
            .take()
            .map(|context| *context)
            .unwrap_or_else(|| TurnContext::with_max_tool_rounds(max_tool_rounds))
    } else {
        // A real user prompt starts a new logical task. Do not let a stale
        // background result inherit the previous task's loop or verification
        // budgets.
        state.background_turn_context = None;
        TurnContext::with_max_tool_rounds(max_tool_rounds)
    }
}

pub(crate) fn save_turn_context_after_run(
    state: &mut AppState,
    context: TurnContext,
    preserve_for_wakeup: bool,
) {
    if preserve_for_wakeup
        && (!context.lifecycle.task_completed
            || matches!(
                context.lifecycle.stop_reason,
                Some(lifecycle::StopReason::BackgroundPending)
            ))
    {
        state.background_turn_context = Some(Box::new(context));
    } else {
        state.background_turn_context = None;
    }
}

/// Persist every result that actually completed before cancellation, then
/// close any remaining native calls with typed cancellation results. Dropping
/// the completed prefix and marking the whole batch cancelled would lie about
/// successful work and lose the provider call/result pairing on reload.
pub(crate) fn append_cancelled_batch_results(
    history: &mut Vec<ChatMessage>,
    results: Vec<ToolResult>,
    call_refs: &[crate::app::ToolCallRef],
) {
    let executed = results.len();
    for (position, mut result) in results.into_iter().enumerate() {
        let answered_call = call_refs.get(position).map(|call| call.id.clone());
        result.metadata.call_id = answered_call.clone();
        history.push(tool_result_history_message(result, answered_call));
    }
    if executed < call_refs.len() {
        history.extend(unanswered_call_results_with_kind(
            &call_refs[executed..],
            "interrupted by the user",
            crate::tools::ToolErrorKind::Cancelled,
        ));
    }
}

pub(crate) fn hydrate_explicit_verification_from_history(
    ledger: &mut verification::VerificationLedger,
    history: &[ChatMessage],
    user_prompt_index: usize,
) {
    let Some(record) = history
        .iter()
        .skip(user_prompt_index.saturating_add(1))
        .rev()
        .filter_map(|message| message.tool_result.as_ref())
        .find(|record| !record.pending && record.command.is_some())
    else {
        return;
    };
    let Some(command) = record.command.as_deref() else {
        return;
    };
    ledger.record_command(command, record.exit_code);
    ledger.record_explicit_command(command, record.exit_code);
}

/// Record a tool-call protocol failure and return whether it is identical to
/// the immediately preceding malformed request. Parsed calls use a stable
/// name/arguments fingerprint; unparseable fences fall back to their bounded
/// raw text so they still consume the same recovery budget.
pub async fn run_single_turn<P: policy::TurnPolicy + 'static>(
    client: &reqwest::Client,
    state: &Arc<Mutex<AppState>>,
    cancel_token: &tokio_util::sync::CancellationToken,
    policy: &Arc<P>,
    stream_buffer: &Arc<Mutex<StreamBuffer>>,
    ctx: &mut TurnContext,
) -> bool {
    dbg_log!("Starting agent loop round {}", ctx.budget.tool_rounds);

    // Phase 1: enforce lifecycle budgets and prepare the provider request.
    if !cancel_token.is_cancelled()
        && let Some(limit) = turn_budget_exceeded(ctx)
    {
        return stop_turn_for_budget(state, ctx, limit).await;
    }

    if !ctx.recovery.force_final {
        ctx.lifecycle.stop_reason = None;
    }

    let round = match request::collect_round(client, state, cancel_token, stream_buffer, ctx).await
    {
        Ok(round) => round,
        Err(()) => return false,
    };
    let request::RoundResponse {
        content,
        final_answer_boundary,
        provider_final_answer_state,
        finish_reason: response_finish_reason,
        response_time_ms: turn_response_time_ms,
        token_usage: turn_token_usage,
        thought_time_ms,
        thought_tokens,
        native_tool_calls,
    } = round;
    ctx.response.final_content = content;
    ctx.response.final_content_persisted = false;
    dbg_log!(
        "Stream completed successfully. Content length: {} chars",
        ctx.response.final_content.len()
    );

    match recovery::handle_response_recovery(
        state,
        ctx,
        native_tool_calls.is_empty(),
        response_finish_reason.as_deref(),
        turn_response_time_ms,
        turn_token_usage.clone(),
        thought_time_ms,
        thought_tokens,
        final_answer_boundary,
        provider_final_answer_state,
    )
    .await
    {
        recovery::ResponseRecoveryOutcome::Continue => return true,
        recovery::ResponseRecoveryOutcome::Stop => return false,
        recovery::ResponseRecoveryOutcome::Proceed => {}
    }

    match tools::handle_tool_response(
        client,
        state,
        cancel_token,
        policy,
        ctx,
        response_finish_reason.as_deref(),
        turn_response_time_ms,
        turn_token_usage.clone(),
        thought_time_ms,
        thought_tokens,
        native_tool_calls,
    )
    .await
    {
        tools::ToolHandlingOutcome::Continue => return true,
        tools::ToolHandlingOutcome::Stop => return false,
        tools::ToolHandlingOutcome::NotHandled => {}
    }

    match finish::handle_plain_response_finish(
        state,
        policy,
        ctx,
        turn_response_time_ms,
        turn_token_usage,
        thought_time_ms,
        thought_tokens,
        final_answer_boundary,
        provider_final_answer_state,
    )
    .await
    {
        finish::FinishGateOutcome::Continue => true,
        finish::FinishGateOutcome::Stop => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{messages_for_response_continuation, reasoning_loop_final_response};
    use crate::network::EMPTY_RESPONSE_RECOVERY_PROMPT;

    #[test]
    fn continuation_reuses_base_then_appends_provider_visible_delta() {
        let base = vec![
            serde_json::json!({"role": "system", "content": "rules"}),
            serde_json::json!({"role": "user", "content": "question"}),
        ];
        let initial = messages_for_response_continuation(&base, "");
        assert!(matches!(initial, std::borrow::Cow::Borrowed(_)));
        assert_eq!(initial.as_ref(), base.as_slice());

        let continued = messages_for_response_continuation(&base, "partial answer");
        assert_eq!(&continued[..base.len()], base.as_slice());
        assert_eq!(continued[base.len()]["role"], "assistant");
        assert_eq!(continued[base.len() + 1]["role"], "user");
        assert_eq!(
            base.len(),
            2,
            "continuation must not mutate the shared base"
        );
    }

    #[test]
    fn exhausted_reasoning_loop_uses_a_concise_terminal_response() {
        let response = reasoning_loop_final_response();

        assert_eq!(
            response,
            "I stopped after repeated reasoning to avoid looping. Please review the current changes and continue from there."
        );
        assert!(response.len() <= 160);
    }

    #[test]
    fn empty_response_recovery_prompt_is_bounded_and_answer_focused() {
        assert!(EMPTY_RESPONSE_RECOVERY_PROMPT.len() <= 300);
        assert!(EMPTY_RESPONSE_RECOVERY_PROMPT.contains("answer the user's request"));
        assert!(EMPTY_RESPONSE_RECOVERY_PROMPT.contains("Do not call tools"));
    }
}
