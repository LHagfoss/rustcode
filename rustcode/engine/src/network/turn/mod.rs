mod context;
mod finish;
mod queue;
mod recovery;
mod request;
pub(crate) mod tools;

pub use context::{GroundedArtifactEvidence, SegmentCheckpoint, TurnContext};
pub use finish::run_agent_turn;
pub(crate) use finish::run_agent_turn_with_context;
pub(crate) use finish::run_agent_turn_with_context_for_session;
pub(crate) use queue::process_queue_orchestrator_with_ui_events;
#[cfg(test)]
use recovery::reasoning_loop_final_response;
#[cfg(test)]
pub(crate) use recovery::record_malformed_call;
#[cfg(test)]
use request::messages_for_response_continuation;

use crate::app::{AppState, ChatMessage};
use std::sync::Arc;
use tokio::sync::Mutex;

pub(super) fn clear_turn_steerability_for_session(state: &mut AppState, session_id: &str) {
    if state.active_turn_steerable_session.as_deref() == Some(session_id) {
        state.active_turn_steerable_session = None;
    }
}

use super::events::ToolResult;
use super::lifecycle;
use super::policy;
use super::stream::StreamBuffer;
use super::tool_exec::tool_result_history_message;
use super::verification;
use super::{stop_turn_for_budget, turn_budget_exceeded, unanswered_call_results_with_kind};

#[cfg(test)]
pub(crate) fn take_turn_context_for_prompt(
    state: &mut AppState,
    is_wakeup: bool,
    max_tool_rounds: usize,
) -> TurnContext {
    take_turn_context_for_prompt_with_limits(
        state,
        is_wakeup,
        max_tool_rounds,
        crate::config::DEFAULT_MAX_TOTAL_TOOL_ROUNDS,
    )
}

pub(crate) fn take_turn_context_for_prompt_with_limits(
    state: &mut AppState,
    is_wakeup: bool,
    _max_tool_rounds: usize,
    _max_total_tool_rounds: usize,
) -> TurnContext {
    let mut context = if is_wakeup {
        let mut context = state
            .background_turn_context
            .take()
            .map(|context| *context)
            .unwrap_or_else(TurnContext::new);
        if context.budget.continuation_pending {
            context.begin_next_segment();
        }
        state.current_turn_token_usage = context.response.turn_token_usage.clone();
        state.current_turn_token_usage_is_estimated =
            context.response.turn_token_usage_is_estimated;
        state.current_round_token_usage = None;
        state.current_round_estimated_input_tokens = 0;
        state.current_round_estimated_output_tokens = 0;
        state.current_provider_request_prompt_estimate = 0;
        state.token_usage_in_flight = false;
        state.provider_request_in_flight = false;
        context
    } else {
        // A real user prompt starts a new logical task. Do not let a stale
        // background result inherit the previous task's loop or verification
        // budgets.
        state.background_turn_context = None;
        state.current_turn_token_usage = None;
        state.current_turn_token_usage_is_estimated = false;
        state.current_round_token_usage = None;
        state.current_round_estimated_input_tokens = 0;
        state.current_round_estimated_output_tokens = 0;
        state.current_provider_request_prompt_estimate = 0;
        state.token_usage_in_flight = false;
        state.provider_request_in_flight = false;
        // Held-over tool calls belong to the previous task (#1590); the new
        // prompt's transcript never promised them.
        state.clear_deferred_tool_calls();
        crate::config::clear_segment_checkpoint(&state.active_session_id);
        TurnContext::new()
    };
    // The arguments and checkpoint fields are retained for compatibility with
    // older callers and saved sessions. Neither live logical turns nor their
    // resumptions use a max-round or max-call ceiling.
    context.remove_round_limits();
    context
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
            )
            || context.budget.continuation_pending)
    {
        let background_pending = matches!(
            context.lifecycle.stop_reason,
            Some(lifecycle::StopReason::BackgroundPending)
        );
        // Persist the segment sidecar so a restart can resume the pending
        // continuation instead of losing the long task's budgets.
        crate::config::save_segment_checkpoint(
            &state.active_session_id,
            &context.segment_checkpoint(
                &state.active_session_id,
                context.budget.continuation_pending,
                background_pending,
            ),
        );
        state.background_turn_context = Some(Box::new(context));
    } else {
        crate::config::clear_segment_checkpoint(&state.active_session_id);
        state.background_turn_context = None;
        // The turn is over: anything the scheduler still held for it is
        // released rather than executed against the next task (#1590).
        state.clear_deferred_tool_calls();
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
    if verification::is_explicit_verification_command(&history[user_prompt_index].content, command)
    {
        ledger.record_explicit_command(command, record.exit_code);
    }
}

/// Warn once per logical turn, including when that turn resumes after a
/// background wakeup. Keep the checkpoint count explicit: the notice can
/// remain in history after subsequent rounds have consumed more budget.
pub(crate) fn take_round_budget_notice(ctx: &mut TurnContext) -> Option<String> {
    if ctx.budget.max_tool_rounds == usize::MAX {
        return None;
    }
    let segment_rounds = ctx.segment_rounds();
    let remaining = ctx.budget.max_tool_rounds.saturating_sub(segment_rounds);
    let warning_rounds = ctx.budget.max_tool_rounds.div_ceil(5).min(8);
    if ctx.budget.round_budget_notice_sent || remaining == 0 || remaining > warning_rounds {
        return None;
    }
    ctx.budget.round_budget_notice_sent = true;
    Some(format!(
        "[Turn budget checkpoint: {used}/{maximum} tool/recovery rounds used; \
         {remaining} rounds remain at this checkpoint before the hard stop. \
         This counts rounds, not individual tool calls. Prioritize outstanding errors \
         and required validation; avoid expanding scope. If the task cannot be completed \
         within the remaining budget, report the unfinished work and validation status \
         accurately. Do not claim success without evidence or bypass safety checks.]",
        used = segment_rounds,
        maximum = ctx.budget.max_tool_rounds,
    ))
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
    turn_session_id: &str,
) -> bool {
    dbg_log!("Starting agent loop round {}", ctx.budget.tool_rounds);

    // Phase 1: enforce lifecycle budgets and prepare the provider request.
    if !cancel_token.is_cancelled()
        && let Some(limit) = turn_budget_exceeded(ctx)
    {
        return stop_turn_for_budget(state, ctx, limit).await;
    }

    if !cancel_token.is_cancelled()
        && let Some(notice) = take_round_budget_notice(ctx)
    {
        state
            .lock()
            .await
            .history
            .push(ChatMessage::new("system", notice));
    }

    if !ctx.recovery.force_final {
        ctx.lifecycle.stop_reason = None;
    }

    let round = request::collect_round(
        client,
        state,
        cancel_token,
        stream_buffer,
        ctx,
        turn_session_id,
    )
    .await;
    {
        let mut app = state.lock().await;
        app.token_usage_in_flight = false;
        app.current_turn_token_usage = ctx.response.turn_token_usage.clone();
        app.current_turn_token_usage_is_estimated |= ctx.response.turn_token_usage_is_estimated
            || app.current_round_estimated_input_tokens > 0
            || app.current_round_estimated_output_tokens > 0;
        app.current_round_token_usage = None;
        app.current_round_estimated_input_tokens = 0;
        app.current_round_estimated_output_tokens = 0;
        app.current_provider_request_prompt_estimate = 0;
        app.provider_request_in_flight = false;
        app.request_redraw();
    }
    let round = match round {
        Ok(round) => round,
        Err(request::RoundCollectionError::Stop) => return false,
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
        stream_termination,
    } = round;
    ctx.response.final_content = content;
    ctx.response.last_stream_termination = stream_termination;
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
        turn_session_id,
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
        &turn_session_id,
    )
    .await
    {
        tools::ToolHandlingOutcome::Continue => return true,
        tools::ToolHandlingOutcome::Stop => return false,
        tools::ToolHandlingOutcome::NotHandled => {}
    }

    match finish::handle_plain_response_finish_for_session(
        state,
        cancel_token,
        policy,
        ctx,
        super::events::FinishReason::from_provider(response_finish_reason.as_deref()),
        turn_response_time_ms,
        turn_token_usage,
        thought_time_ms,
        thought_tokens,
        final_answer_boundary,
        provider_final_answer_state,
        turn_session_id,
    )
    .await
    {
        finish::FinishGateOutcome::Continue => true,
        finish::FinishGateOutcome::Stop => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        messages_for_response_continuation, reasoning_loop_final_response,
        take_turn_context_for_prompt, take_turn_context_for_prompt_with_limits,
    };
    use crate::app::{AppState, TokenUsage};
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

    #[test]
    fn background_wakeup_keeps_usage_and_a_new_prompt_resets_it() {
        let usage = TokenUsage {
            prompt_tokens: 120,
            completion_tokens: 30,
            total_tokens: 150,
            ..Default::default()
        };
        let mut context = super::TurnContext::new();
        context.response.turn_token_usage = Some(usage.clone());
        context.response.turn_token_usage_is_estimated = true;
        let checkpoint = context.segment_checkpoint("session-1", true, false);
        let checkpoint: super::SegmentCheckpoint = serde_json::from_str(
            &serde_json::to_string(&checkpoint).expect("serialize segment checkpoint"),
        )
        .expect("restore segment checkpoint data");
        let mut restored_context = super::TurnContext::new();
        assert!(restored_context.restore_segment(&checkpoint, "session-1"));
        let mut state = AppState::new();
        state.background_turn_context = Some(Box::new(restored_context));

        let resumed = take_turn_context_for_prompt(&mut state, true, 40);
        assert_eq!(resumed.response.turn_token_usage, Some(usage.clone()));
        assert_eq!(state.current_turn_token_usage, Some(usage));
        assert!(resumed.response.turn_token_usage_is_estimated);
        assert!(state.current_turn_token_usage_is_estimated);
        assert!(!state.token_usage_in_flight);

        let _new = take_turn_context_for_prompt(&mut state, false, 40);
        assert!(state.current_turn_token_usage.is_none());
        assert!(!state.current_turn_token_usage_is_estimated);
        assert!(!state.token_usage_in_flight);

        let provider_usage = TokenUsage {
            prompt_tokens: 120,
            completion_tokens: 30,
            total_tokens: 150,
            ..Default::default()
        };
        let mut provider_context = super::TurnContext::new();
        provider_context.response.turn_token_usage = Some(provider_usage.clone());
        let mut provider_state = AppState::new();
        provider_state.background_turn_context = Some(Box::new(provider_context));
        let provider_resumed = take_turn_context_for_prompt(&mut provider_state, true, 40);
        assert_eq!(
            provider_resumed.response.turn_token_usage,
            Some(provider_usage)
        );
        assert!(!provider_state.current_turn_token_usage_is_estimated);
    }

    #[test]
    fn live_turns_ignore_legacy_config_and_saved_round_ceilings() {
        let mut state = AppState::new();
        state.config.max_tool_rounds = 1000;
        state.config.max_total_tool_rounds = 2000;

        let mut saved = super::TurnContext::with_budgets(40, 80);
        saved.budget.tool_rounds = 40;
        saved.budget.continuation_pending = true;
        let checkpoint = saved.segment_checkpoint("legacy-session", true, false);
        let checkpoint: super::SegmentCheckpoint = serde_json::from_str(
            &serde_json::to_string(&checkpoint).expect("serialize legacy checkpoint"),
        )
        .expect("deserialize legacy checkpoint");
        let mut restored = super::TurnContext::new();
        assert!(restored.restore_segment(&checkpoint, "legacy-session"));
        state.background_turn_context = Some(Box::new(restored));

        let mut resumed = take_turn_context_for_prompt_with_limits(&mut state, true, 1000, 2000);
        assert_eq!(resumed.budget.tool_rounds, 40);
        assert_eq!(resumed.budget.max_tool_rounds, usize::MAX);
        assert_eq!(resumed.budget.max_total_tool_rounds, usize::MAX);
        resumed.budget.tool_rounds = 141;
        assert_eq!(
            crate::network::turn_budget_exceeded(&resumed),
            None,
            "a legacy 40-round checkpoint must keep running past 40"
        );

        let fresh = take_turn_context_for_prompt_with_limits(&mut state, false, 1000, 2000);
        assert_eq!(fresh.budget.max_tool_rounds, usize::MAX);
        assert_eq!(fresh.budget.max_total_tool_rounds, usize::MAX);
    }
}
