use std::sync::Arc;

use tokio::sync::Mutex;

use crate::app::{AppState, AppStatus, ChatMessage, StreamTracker};

use super::super::events::FinishReason;
use super::super::fetch_model_quota;
use super::super::lifecycle;
use super::super::policy;
use super::super::stream::{FinalAnswerBoundary, ProviderFinalAnswerState, StreamBuffer};
use super::recovery::{
    completed_inspection_synthesis, outstanding_external_action, reasoning_loop_final_response,
};
use super::{TurnContext, run_single_turn};

pub async fn run_agent_turn<P: policy::TurnPolicy + 'static>(
    client: &reqwest::Client,
    state: &Arc<Mutex<AppState>>,
    cancel_token: &tokio_util::sync::CancellationToken,
    policy: &Arc<P>,
    stream_buffer: &Arc<Mutex<StreamBuffer>>,
) -> TurnContext {
    let (max_tool_rounds, max_total_tool_rounds) = {
        let s = state.lock().await;
        (s.config.max_tool_rounds, s.config.max_total_tool_rounds)
    };
    run_agent_turn_with_context(
        client,
        state,
        cancel_token,
        policy,
        stream_buffer,
        TurnContext::with_budgets(max_tool_rounds, max_total_tool_rounds),
    )
    .await
}

pub(crate) async fn run_agent_turn_with_context<P: policy::TurnPolicy + 'static>(
    client: &reqwest::Client,
    state: &Arc<Mutex<AppState>>,
    cancel_token: &tokio_util::sync::CancellationToken,
    policy: &Arc<P>,
    stream_buffer: &Arc<Mutex<StreamBuffer>>,
    ctx: TurnContext,
) -> TurnContext {
    let turn_session_id = state.lock().await.active_session_id.clone();
    run_agent_turn_with_context_for_session(
        client,
        state,
        cancel_token,
        policy,
        stream_buffer,
        ctx,
        turn_session_id,
    )
    .await
}

pub(crate) async fn run_agent_turn_with_context_for_session<P: policy::TurnPolicy + 'static>(
    client: &reqwest::Client,
    state: &Arc<Mutex<AppState>>,
    cancel_token: &tokio_util::sync::CancellationToken,
    policy: &Arc<P>,
    stream_buffer: &Arc<Mutex<StreamBuffer>>,
    mut ctx: TurnContext,
    turn_session_id: String,
) -> TurnContext {
    let prompt_start_time = std::time::Instant::now();
    let mut turn_lifecycle = lifecycle::TurnLifecycle::new();
    while run_single_turn(
        client,
        state,
        cancel_token,
        policy,
        stream_buffer,
        &mut ctx,
        &turn_session_id,
    )
    .await
    {}

    if ctx.lifecycle.stop_reason.is_none() {
        ctx.lifecycle.stop_reason = Some(if ctx.lifecycle.task_completed {
            lifecycle::StopReason::Completed
        } else {
            lifecycle::StopReason::RecoveryFailed
        });
    }
    let stop_reason = ctx
        .lifecycle
        .stop_reason
        .clone()
        .expect("turn finalization always assigns a stop reason");
    if matches!(stop_reason, lifecycle::StopReason::LoopEscalation) {
        let promoted =
            super::super::text::promote_bare_thought_markers(&ctx.response.final_content);
        let clean = super::super::text::strip_tool_call_syntax(
            &super::super::text::strip_think_blocks(&promoted),
        );
        ctx.response.final_content = if clean.trim().is_empty() {
            super::recovery::reasoning_loop_final_response().to_string()
        } else {
            clean.trim().to_string()
        };
        ctx.response.final_content_persisted = false;
    }
    if !turn_lifecycle.mark_finalized() {
        return ctx;
    }
    let had_final_content =
        !ctx.response.final_content_persisted && !ctx.response.final_content.trim().is_empty();
    let final_transcript = lifecycle::final_transcript_content(
        ctx.lifecycle.task_completed,
        &ctx.response.final_content,
        ctx.response.final_content_persisted,
        &stop_reason,
    );
    if let Some(content) = final_transcript.as_ref()
        && !had_final_content
    {
        ctx.response.final_content = content.clone();
    }
    crate::logger::operational_event(
        "turn.summary",
        serde_json::json!({
            "session_id": turn_session_id,
            "completed_task": ctx.lifecycle.task_completed,
            "metrics": ctx.benchmark_summary(),
        }),
    );

    dbg_log!("Finishing agent loop, writing final transcript");
    crate::logger::operational_event(
        "turn.finish",
        serde_json::json!({
            "session_id": turn_session_id,
            "completed_task": ctx.lifecycle.task_completed,
            "tool_rounds": ctx.budget.tool_rounds,
            "content_bytes": ctx.response.final_content.len(),
            "cancelled": cancel_token.is_cancelled(),
            "metrics": ctx.benchmark_summary(),
        }),
    );

    let mut s = state.lock().await;
    // A cancelled turn may finish after /history has attached another
    // session. Its final response belongs to the old session and must not be
    // appended to the newly selected conversation.
    if s.active_session_id != turn_session_id {
        super::clear_turn_steerability_for_session(&mut s, &turn_session_id);
        return ctx;
    }
    // Completed, cancelled, and failed turns all share this finalization
    // boundary and must stop accepting new steers.
    super::clear_turn_steerability_for_session(&mut s, &turn_session_id);
    let usage = s
        .current_token_usage
        .clone()
        .or_else(|| ctx.response.last_token_usage.clone());
    let turn_usage = ctx
        .response
        .turn_token_usage
        .clone()
        .or_else(|| usage.clone());
    s.continuous_mode = false;
    s.response_time = Some(prompt_start_time.elapsed());
    if let Some(content) = final_transcript {
        let role = if had_final_content {
            "assistant"
        } else {
            "system"
        };
        let mut msg = ChatMessage::new(role, content);
        msg.response_time_ms = s.response_time.map(|d| d.as_millis() as u64);
        if msg.content.contains("<think>") {
            msg.thought_time_ms = Some(s.current_thought_time_ms);
            msg.thought_tokens = Some(s.current_thought_tokens);
        }
        msg.token_usage = usage.clone();
        s.history.push(msg);
    }
    let latest_user_index = s
        .history
        .iter()
        .enumerate()
        .rev()
        .find(|(_, message)| message.role == "user" && !message.conversation_recap)
        .map(|(index, _)| index);
    s.last_turn_had_model_final_response = ctx.lifecycle.task_completed
        && latest_user_index.is_some_and(|index| {
            s.history.iter().skip(index + 1).any(|message| {
                message.role == "assistant"
                    && !message.conversation_recap
                    && !message.unexecuted_tool_call_checkpoint
                    && message.tool_calls.is_empty()
                    && !message.content.trim().is_empty()
            })
        });
    if let Some(msg) = s.history.iter_mut().rev().find(|m| m.role == "assistant")
        && msg.token_usage.is_none()
    {
        msg.token_usage = usage.clone();
    }
    let active_id = s.active_session_id.clone();
    crate::config::save_session_history(&active_id, &s.history);
    crate::config::flush_history_async();
    s.clear_current_response();
    s.clear_live_tool_calls();
    s.enter_idle();
    s.request_redraw();
    if let Some(u) = &turn_usage {
        crate::config::track_usage(u.prompt_tokens as u64, u.completion_tokens as u64);
    }
    s.current_token_usage = usage;
    drop(s);

    let state_quota = Arc::clone(state);
    let client_quota = client.clone();
    tokio::spawn(async move {
        fetch_model_quota(&client_quota, &state_quota).await;
    });
    let notification = finished_notification_status(&ctx, cancel_token.is_cancelled());
    let _ = crate::notifications::notify_finished(notification);
    ctx
}

fn finished_notification_status(
    ctx: &TurnContext,
    cancelled: bool,
) -> crate::notifications::FinishedStatus {
    if cancelled
        || matches!(
            ctx.lifecycle.stop_reason.as_ref(),
            Some(lifecycle::StopReason::Cancelled)
        )
    {
        crate::notifications::FinishedStatus::Cancelled
    } else if ctx.lifecycle.task_completed
        && matches!(
            ctx.lifecycle.stop_reason.as_ref(),
            Some(lifecycle::StopReason::Completed | lifecycle::StopReason::CompletedWithWarning(_))
        )
    {
        crate::notifications::FinishedStatus::Success
    } else {
        crate::notifications::FinishedStatus::Incomplete
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FinishGateOutcome {
    Continue,
    Stop,
}

fn has_verified_implicit_completion(ctx: &TurnContext) -> bool {
    if !ctx.progress.made_edits
        || ctx.recovery.force_final
        || ctx.verification.ledger.explicit_last_failure().is_some()
        || !ctx.verification.ledger.has_fresh_successful_verification()
    {
        return false;
    }

    has_substantive_final_prose(&ctx.response.final_content)
}

fn has_substantive_final_prose(content: &str) -> bool {
    let promoted = super::super::text::promote_bare_thought_markers(content);
    let prose = super::super::text::strip_tool_call_syntax(
        &super::super::text::strip_think_blocks(&promoted),
    );
    !prose.trim().is_empty()
}

/// Issue #1234: when the final response is thinking-only (no user-facing
/// prose) but the turn did work, persisting the raw think-dump leaves the
/// user with kilobytes of dithering and no answer. Substitute a deterministic
/// work summary so the transcript always ends the turn with readable text.
/// Turns with no work keep their content untouched.
fn presentable_final_content(ctx: &TurnContext) -> String {
    if has_substantive_final_prose(&ctx.response.final_content) {
        return ctx.response.final_content.clone();
    }
    let inspections = ctx.progress.complete_inspection_results;
    let edits = ctx.progress.made_edits;
    if inspections == 0 && !edits && ctx.progress.changed_paths.is_empty() {
        return ctx.response.final_content.clone();
    }
    let mut parts = Vec::new();
    if inspections > 0 {
        parts.push(format!("{inspections} inspection result(s)"));
    }
    if edits {
        parts.push("file edit(s)".to_owned());
    }
    let mut summary = format!(
        "Turn completed without a final summary. Work done: {}.",
        parts.join(", ")
    );
    if !ctx.progress.changed_paths.is_empty() {
        let paths: Vec<&str> = ctx
            .progress
            .changed_paths
            .iter()
            .take(8)
            .map(String::as_str)
            .collect();
        summary.push_str(&format!(" Files touched: {}.", paths.join(", ")));
    }
    summary
}

fn can_complete_interactive_plain_response(
    ctx: &TurnContext,
    cancel_token: &tokio_util::sync::CancellationToken,
    finish_reason: &FinishReason,
    has_outstanding_external_action: bool,
) -> bool {
    matches!(finish_reason, FinishReason::Stop)
        && !cancel_token.is_cancelled()
        && !ctx.recovery.force_final
        && !ctx.progress.made_edits
        && ctx.progress.failed_mutations == 0
        && ctx.verification.ledger.last_failure().is_none()
        && ctx.lifecycle.stop_reason.is_none()
        && !has_outstanding_external_action
        && has_substantive_final_prose(&ctx.response.final_content)
}

fn terminalize_exhausted_reasoning_response(
    ctx: &mut TurnContext,
    response_finish_reason: &FinishReason,
) -> bool {
    if ctx.recovery.reasoning_recovery_attempts == 0 {
        return false;
    }

    let output_budget_exhausted = matches!(response_finish_reason, FinishReason::Length);
    let answer_missing = !has_substantive_final_prose(&ctx.response.final_content);
    if !output_budget_exhausted && !answer_missing {
        return false;
    }

    ctx.response.final_content = reasoning_loop_final_response().to_string();
    ctx.response.final_content_persisted = false;
    ctx.lifecycle.task_completed = false;
    ctx.lifecycle.stop_reason = Some(lifecycle::StopReason::LoopEscalation);
    true
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_plain_response_finish<P: policy::TurnPolicy + 'static>(
    state: &Arc<Mutex<AppState>>,
    cancel_token: &tokio_util::sync::CancellationToken,
    policy: &Arc<P>,
    ctx: &mut TurnContext,
    response_finish_reason: FinishReason,
    turn_response_time_ms: u64,
    turn_token_usage: Option<crate::app::TokenUsage>,
    thought_time_ms: Option<u64>,
    thought_tokens: Option<u32>,
    final_answer_boundary: FinalAnswerBoundary,
    provider_final_answer_state: ProviderFinalAnswerState,
) -> FinishGateOutcome {
    let turn_session_id = state.lock().await.active_session_id.clone();
    handle_plain_response_finish_for_session(
        state,
        cancel_token,
        policy,
        ctx,
        response_finish_reason,
        turn_response_time_ms,
        turn_token_usage,
        thought_time_ms,
        thought_tokens,
        final_answer_boundary,
        provider_final_answer_state,
        &turn_session_id,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_plain_response_finish_for_session<P: policy::TurnPolicy + 'static>(
    state: &Arc<Mutex<AppState>>,
    cancel_token: &tokio_util::sync::CancellationToken,
    policy: &Arc<P>,
    ctx: &mut TurnContext,
    response_finish_reason: FinishReason,
    turn_response_time_ms: u64,
    turn_token_usage: Option<crate::app::TokenUsage>,
    thought_time_ms: Option<u64>,
    thought_tokens: Option<u32>,
    final_answer_boundary: FinalAnswerBoundary,
    provider_final_answer_state: ProviderFinalAnswerState,
    turn_session_id: &str,
) -> FinishGateOutcome {
    const MAX_FINISH_GATE_RETRIES: u32 = 2;
    use super::super::cached_compiler_check;
    {
        let mut s = state.lock().await;
        super::clear_turn_steerability_for_session(&mut s, turn_session_id);
    }
    let is_continuous = { state.lock().await.continuous_mode };
    if is_continuous && ctx.budget.tool_rounds > 0 {
        dbg_log!(
            "Continuous mode active, assistant responded with text prose. Ending continuous mode turn."
        );
        let mut s = state.lock().await;
        s.continuous_mode = false;
    } else if is_continuous && ctx.budget.tool_rounds == 0 {
        dbg_log!(
            "Continuous mode active, but assistant gave a plain conversational reply (no tools used). Ending turn."
        );
        let mut s = state.lock().await;
        s.continuous_mode = false;
    }

    if terminalize_exhausted_reasoning_response(ctx, &response_finish_reason) {
        dbg_log!(
            "Reasoning recovery exhausted without a complete answer; returning concise diagnostic"
        );
        return FinishGateOutcome::Stop;
    }

    // Normalize thinking-only finales before the finish gate evaluates or
    // persists them (see presentable_final_content).
    ctx.response.final_content = presentable_final_content(ctx);

    let mut finish_gate_passed = !policy.should_verify_completion() || !ctx.progress.made_edits;
    if policy.should_verify_completion()
        && ctx.progress.made_edits
        && !ctx.recovery.force_final
        && ctx.recovery.finish_gate_retries < MAX_FINISH_GATE_RETRIES
    {
        let root = ctx
            .compiler
            .edit_root
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        dbg_log!(
            "Finish gate: compile-checking {} before accepting done",
            root.display()
        );
        {
            let mut s = state.lock().await;
            s.status = AppStatus::Streaming;
        }
        if let Some(errors) = cached_compiler_check(
            &root,
            &mut ctx.compiler.dirty,
            &mut ctx.compiler.cache,
            cancel_token,
        )
        .await
        {
            if errors.starts_with("__BUILD_UNVERIFIED__") {
                dbg_log!("Finish gate: build unverified — {errors}");
                let mut s = state.lock().await;
                s.history.push(ChatMessage::new(
                    "system",
                    format!("[⚠ Build could not be verified — {errors}]"),
                ));
                crate::config::save_session_history(&s.active_session_id, &s.history);
                drop(s);
            } else {
                ctx.recovery.finish_gate_retries += 1;
                ctx.budget.tool_rounds += 1;
                dbg_log!(
                    "Finish gate: build is RED, forcing a fix round ({}/{})",
                    ctx.recovery.finish_gate_retries,
                    MAX_FINISH_GATE_RETRIES
                );
                let mut s = state.lock().await;
                let mut msg = ChatMessage::new("assistant", ctx.response.final_content.clone());
                msg.response_time_ms = Some(turn_response_time_ms);
                msg.token_usage = turn_token_usage.clone();
                msg.thought_time_ms = thought_time_ms;
                msg.thought_tokens = thought_tokens;
                s.history.push(msg);
                ctx.response.final_content_persisted = true;
                s.history.push(ChatMessage::new(
                            "system",
                            format!(
                                "[Finish blocked — the build does not compile. You cannot report this \
                                 task as done while there are compiler errors. Fix them, then finish. \
                                 Compiler errors:\n{errors}]"
                            ),
                        ));
                crate::config::save_session_history(&s.active_session_id, &s.history);
                s.clear_current_response();
                s.status = AppStatus::Streaming;
                s.stream_tracker = Some(StreamTracker::new());
                drop(s);
                if let Err(invalid) = ctx.lifecycle.turn_machine.retry_for_finish_gate() {
                    dbg_log!("Turn machine rejected finish-gate retry: {invalid}");
                    return FinishGateOutcome::Stop;
                }
                return FinishGateOutcome::Continue;
            }
        } else {
            finish_gate_passed = true;
        }
        if finish_gate_passed {
            dbg_log!("Finish gate: build is green, accepting done");
        }
    }

    let has_outstanding_external_action = {
        let state = state.lock().await;
        outstanding_external_action(&state.history)
    };

    if finish_gate_passed
        && !policy.is_headless()
        && can_complete_interactive_plain_response(
            ctx,
            cancel_token,
            &response_finish_reason,
            has_outstanding_external_action,
        )
    {
        dbg_log!("Normal interactive prose accepted as completion");
        let mut s = state.lock().await;
        if !ctx.response.final_content_persisted {
            let mut msg = ChatMessage::new("assistant", ctx.response.final_content.clone());
            msg.response_time_ms = Some(turn_response_time_ms);
            msg.token_usage = turn_token_usage;
            msg.thought_time_ms = thought_time_ms;
            msg.thought_tokens = thought_tokens;
            s.history.push(msg);
            crate::config::save_session_history(&s.active_session_id, &s.history);
            ctx.response.final_content_persisted = true;
        }
        ctx.lifecycle.task_completed = true;
        ctx.lifecycle.stop_reason = Some(lifecycle::StopReason::Completed);
    } else if finish_gate_passed
        && policy.is_headless()
        && ctx.lifecycle.stop_reason.is_none()
        && let Some(summary) = completed_inspection_synthesis(
            ctx,
            &ctx.response.final_content,
            true,
            final_answer_boundary,
            provider_final_answer_state,
        )
    {
        dbg_log!("Complete read-only inspection accepted as headless completion");
        let mut s = state.lock().await;
        if !ctx.response.final_content_persisted {
            let mut msg = ChatMessage::new("assistant", summary.clone());
            msg.response_time_ms = Some(turn_response_time_ms);
            msg.token_usage = turn_token_usage;
            msg.thought_time_ms = thought_time_ms;
            msg.thought_tokens = thought_tokens;
            s.history.push(msg);
            crate::config::save_session_history(&s.active_session_id, &s.history);
            ctx.response.final_content_persisted = true;
        }
        ctx.response.final_content = summary;
        ctx.lifecycle.task_completed = true;
        ctx.lifecycle.stop_reason = Some(lifecycle::StopReason::Completed);
    } else if finish_gate_passed && has_verified_implicit_completion(ctx) {
        dbg_log!("Verified final prose accepted as implicit completion");
        let mut s = state.lock().await;
        let mut msg = ChatMessage::new("assistant", ctx.response.final_content.clone());
        msg.response_time_ms = Some(turn_response_time_ms);
        msg.token_usage = turn_token_usage;
        msg.thought_time_ms = thought_time_ms;
        msg.thought_tokens = thought_tokens;
        s.history.push(msg);
        crate::config::save_session_history(&s.active_session_id, &s.history);
        ctx.response.final_content_persisted = true;
        ctx.lifecycle.task_completed = true;
        ctx.lifecycle.stop_reason = Some(lifecycle::StopReason::Completed);
    }

    FinishGateOutcome::Stop
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn verified_edit_context(content: &str) -> TurnContext {
        let mut ctx = TurnContext::new();
        ctx.progress.made_edits = true;
        ctx.response.final_content = content.to_string();
        ctx.verification.ledger.record_edit();
        ctx.verification
            .ledger
            .record_command("cargo test", Some(0));
        ctx
    }

    #[test]
    fn finalization_clears_only_the_marker_for_its_turn_session() {
        let mut state = AppState::new();
        let turn_session_id = state.active_session_id.clone();
        state.active_turn_steerable_session = Some(turn_session_id.clone());

        super::super::clear_turn_steerability_for_session(&mut state, &turn_session_id);

        assert_eq!(state.active_turn_steerable_session, None);

        state.active_turn_steerable_session = Some("replacement-session".to_owned());
        super::super::clear_turn_steerability_for_session(&mut state, &turn_session_id);
        assert_eq!(
            state.active_turn_steerable_session.as_deref(),
            Some("replacement-session")
        );
    }

    #[test]
    fn fresh_verification_and_substantive_final_prose_can_complete_implicitly() {
        let ctx = verified_edit_context("Implemented the CLI and all tests pass.");
        assert!(has_verified_implicit_completion(&ctx));
    }

    #[test]
    fn edits_after_verification_cannot_complete_implicitly() {
        let mut ctx = verified_edit_context("Everything is done.");
        ctx.verification.ledger.record_edit();
        assert!(!has_verified_implicit_completion(&ctx));
    }

    #[test]
    fn failed_explicit_verification_cannot_complete_implicitly() {
        let mut ctx = verified_edit_context("Everything is done.");
        ctx.verification
            .ledger
            .record_explicit_command("cargo test --all", Some(1));
        assert!(!has_verified_implicit_completion(&ctx));
    }

    #[test]
    fn thoughts_without_final_prose_cannot_complete_implicitly() {
        let ctx = verified_edit_context("<think>I should probably finish now.</think>");
        assert!(!has_verified_implicit_completion(&ctx));
    }

    #[test]
    fn force_final_recovery_cannot_complete_implicitly() {
        let mut ctx = verified_edit_context("The work is complete.");
        ctx.recovery.force_final = true;
        assert!(!has_verified_implicit_completion(&ctx));
    }

    #[test]
    fn loop_escalation_is_an_incomplete_notification() {
        let mut ctx = TurnContext::new();
        ctx.lifecycle.stop_reason = Some(lifecycle::StopReason::LoopEscalation);

        assert_eq!(
            finished_notification_status(&ctx, false),
            crate::notifications::FinishedStatus::Incomplete
        );
    }

    #[test]
    fn only_verified_completion_is_a_success_notification() {
        let mut ctx = TurnContext::new();
        ctx.lifecycle.task_completed = true;
        ctx.lifecycle.stop_reason = Some(lifecycle::StopReason::Completed);

        assert_eq!(
            finished_notification_status(&ctx, false),
            crate::notifications::FinishedStatus::Success
        );

        ctx.lifecycle.stop_reason = Some(lifecycle::StopReason::RecoveryFailed);
        assert_eq!(
            finished_notification_status(&ctx, false),
            crate::notifications::FinishedStatus::Incomplete
        );
    }

    #[test]
    fn outstanding_external_action_blocks_plain_response_completion() {
        let mut ctx = TurnContext::new();
        ctx.response.final_content = "The reply was not sent.".to_owned();
        let cancel_token = tokio_util::sync::CancellationToken::new();

        assert!(can_complete_interactive_plain_response(
            &ctx,
            &cancel_token,
            &FinishReason::Stop,
            false,
        ));
        assert!(!can_complete_interactive_plain_response(
            &ctx,
            &cancel_token,
            &FinishReason::Stop,
            true,
        ));
    }

    #[test]
    fn cancellation_remains_distinct_from_incomplete_completion() {
        let ctx = TurnContext::new();
        assert_eq!(
            finished_notification_status(&ctx, true),
            crate::notifications::FinishedStatus::Cancelled
        );
    }

    #[tokio::test]
    async fn headless_complete_inspection_report_is_completed_and_persisted_once() {
        let state = Arc::new(Mutex::new(AppState::new()));
        let policy = Arc::new(crate::raw_cli::HeadlessPolicy { quiet: true });
        let mut ctx = TurnContext::new();
        ctx.progress.complete_inspection_results = 1;
        ctx.response.final_content =
            "<think>Reviewed the source.</think>Findings: src/app.ts validates its export input."
                .to_string();

        let outcome = handle_plain_response_finish(
            &state,
            &tokio_util::sync::CancellationToken::new(),
            &policy,
            &mut ctx,
            FinishReason::Stop,
            0,
            None,
            None,
            None,
            FinalAnswerBoundary::ReasoningClosed,
            ProviderFinalAnswerState::Terminal,
        )
        .await;

        assert_eq!(outcome, FinishGateOutcome::Stop);
        assert!(ctx.lifecycle.task_completed);
        assert_eq!(
            ctx.lifecycle.stop_reason,
            Some(lifecycle::StopReason::Completed)
        );
        assert!(ctx.response.final_content_persisted);

        // A repeated finish callback must not duplicate the durable report.
        let _ = handle_plain_response_finish(
            &state,
            &tokio_util::sync::CancellationToken::new(),
            &policy,
            &mut ctx,
            FinishReason::Stop,
            0,
            None,
            None,
            None,
            FinalAnswerBoundary::ReasoningClosed,
            ProviderFinalAnswerState::Terminal,
        )
        .await;

        let state = state.lock().await;
        let reports = state
            .history
            .iter()
            .filter(|message| message.role == "assistant")
            .collect::<Vec<_>>();
        assert_eq!(reports.len(), 1);
        assert_eq!(
            reports[0].content,
            "Findings: src/app.ts validates its export input."
        );
    }

    fn interactive_plain_context(content: &str) -> TurnContext {
        let mut ctx = TurnContext::new();
        ctx.response.final_content = content.to_string();
        ctx
    }

    #[tokio::test]
    async fn ordinary_interactive_prose_completes_and_is_persisted_once() {
        let state = Arc::new(Mutex::new(AppState::new()));
        {
            let mut state = state.lock().await;
            state.status = AppStatus::Streaming;
            state.active_turn_steerable_session = Some(state.active_session_id.clone());
        }
        let policy = Arc::new(policy::InteractivePolicy);
        let mut ctx = interactive_plain_context("Your current progress is 38%.");

        let outcome = handle_plain_response_finish(
            &state,
            &tokio_util::sync::CancellationToken::new(),
            &policy,
            &mut ctx,
            FinishReason::Stop,
            12,
            None,
            None,
            None,
            FinalAnswerBoundary::None,
            ProviderFinalAnswerState::None,
        )
        .await;

        assert_eq!(outcome, FinishGateOutcome::Stop);
        assert!(ctx.lifecycle.task_completed);
        assert_eq!(
            ctx.lifecycle.stop_reason,
            Some(lifecycle::StopReason::Completed)
        );
        assert!(ctx.response.final_content_persisted);
        assert_eq!(state.lock().await.active_turn_steerable_session, None);

        let _ = handle_plain_response_finish(
            &state,
            &tokio_util::sync::CancellationToken::new(),
            &policy,
            &mut ctx,
            FinishReason::Stop,
            12,
            None,
            None,
            None,
            FinalAnswerBoundary::None,
            ProviderFinalAnswerState::None,
        )
        .await;

        let state = state.lock().await;
        let reports = state
            .history
            .iter()
            .filter(|message| message.role == "assistant")
            .collect::<Vec<_>>();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].content, "Your current progress is 38%.");
    }

    #[tokio::test]
    async fn stale_final_response_cannot_clear_a_replacement_session_marker() {
        let state = Arc::new(Mutex::new(AppState::new()));
        let replacement_session_id = {
            let mut state = state.lock().await;
            state.status = AppStatus::Streaming;
            state.active_turn_steerable_session = Some(state.active_session_id.clone());
            state.active_session_id.clone()
        };
        let policy = Arc::new(policy::InteractivePolicy);
        let mut ctx = interactive_plain_context("The answer is ready.");

        let _ = handle_plain_response_finish_for_session(
            &state,
            &tokio_util::sync::CancellationToken::new(),
            &policy,
            &mut ctx,
            FinishReason::Stop,
            12,
            None,
            None,
            None,
            FinalAnswerBoundary::None,
            ProviderFinalAnswerState::None,
            "old-turn-session",
        )
        .await;

        assert_eq!(
            state.lock().await.active_turn_steerable_session.as_deref(),
            Some(replacement_session_id.as_str())
        );
    }

    #[tokio::test]
    async fn successful_interactive_tool_round_followed_by_prose_completes() {
        let state = Arc::new(Mutex::new(AppState::new()));
        state
            .lock()
            .await
            .history
            .push(ChatMessage::new("tool", "get_status: progress_percent=38"));
        let policy = Arc::new(policy::InteractivePolicy);
        let mut ctx = interactive_plain_context("Your current progress is 38%.");
        ctx.budget.tool_rounds = 1;
        ctx.metrics.tool_calls = 1;
        ctx.progress.last_reason =
            Some(super::super::super::loop_detect::ProgressReason::NewInformation);

        let outcome = handle_plain_response_finish(
            &state,
            &tokio_util::sync::CancellationToken::new(),
            &policy,
            &mut ctx,
            FinishReason::Stop,
            12,
            None,
            None,
            None,
            FinalAnswerBoundary::None,
            ProviderFinalAnswerState::None,
        )
        .await;

        assert_eq!(outcome, FinishGateOutcome::Stop);
        assert!(ctx.lifecycle.task_completed);
        assert_eq!(
            ctx.lifecycle.stop_reason,
            Some(lifecycle::StopReason::Completed)
        );
        assert_eq!(
            state
                .lock()
                .await
                .history
                .last()
                .map(|message| message.content.as_str()),
            Some("Your current progress is 38%.")
        );
    }

    #[test]
    fn interactive_plain_completion_preserves_terminal_guards() {
        let normal = FinishReason::Stop;
        let cancel_token = tokio_util::sync::CancellationToken::new();
        cancel_token.cancel();
        let ctx = interactive_plain_context("Done.");
        assert!(!can_complete_interactive_plain_response(
            &ctx,
            &cancel_token,
            &normal,
            false,
        ));

        let cancel_token = tokio_util::sync::CancellationToken::new();
        let mut forced = interactive_plain_context("I stopped safely.");
        forced.recovery.force_final = true;
        assert!(!can_complete_interactive_plain_response(
            &forced,
            &cancel_token,
            &normal,
            false,
        ));

        let output_limit = FinishReason::Length;
        let ctx = interactive_plain_context("Partial response");
        assert!(!can_complete_interactive_plain_response(
            &ctx,
            &cancel_token,
            &output_limit,
            false,
        ));

        let mut failed_verification = interactive_plain_context("Done.");
        failed_verification
            .verification
            .ledger
            .record_command("cargo test", Some(1));
        assert!(!can_complete_interactive_plain_response(
            &failed_verification,
            &cancel_token,
            &normal,
            false,
        ));

        let empty = interactive_plain_context("");
        assert!(!can_complete_interactive_plain_response(
            &empty,
            &cancel_token,
            &normal,
            false,
        ));
        let thought_only = interactive_plain_context("<think>still working</think>");
        assert!(!can_complete_interactive_plain_response(
            &thought_only,
            &cancel_token,
            &normal,
            false,
        ));

        let mut stopped = interactive_plain_context("Done.");
        stopped.lifecycle.stop_reason = Some(lifecycle::StopReason::LoopEscalation);
        assert!(!can_complete_interactive_plain_response(
            &stopped,
            &cancel_token,
            &normal,
            false,
        ));
    }

    #[test]
    fn thinking_only_finale_with_work_synthesizes_a_summary() {
        // Issue #1234: the slime-mold session ended its turn with 18KB of
        // <think> dithering and no user-facing text.
        let mut ctx = TurnContext::new();
        ctx.response.final_content =
            "<think>\nShould I paste the code? Yes. No. Let me reconsider...\n</think>".to_string();
        ctx.progress.complete_inspection_results = 7;
        ctx.progress.changed_paths.insert("slime.html".to_string());
        let presented = presentable_final_content(&ctx);
        assert!(
            !presented.contains("<think>"),
            "think-dump must not reach the transcript: {presented:?}"
        );
        assert!(presented.contains('7'), "{presented:?}");
        assert!(presented.contains("slime.html"), "{presented:?}");

        // Prose passes through untouched.
        let mut ctx = TurnContext::new();
        ctx.response.final_content = "Done, all tests pass.".to_string();
        assert_eq!(presentable_final_content(&ctx), "Done, all tests pass.");

        // No work, no prose: leave alone (existing recovery paths handle it).
        let ctx = TurnContext::new();
        assert_eq!(presentable_final_content(&ctx), "");
    }

    #[test]
    fn exhausted_reasoning_output_becomes_a_concise_terminal_diagnostic() {
        let mut ctx = TurnContext::new();
        ctx.recovery.reasoning_recovery_attempts = 2;
        ctx.response.final_content = format!(
            "<think>{}</think>",
            "unchanged oversized tool result ".repeat(1_000)
        );

        assert!(terminalize_exhausted_reasoning_response(
            &mut ctx,
            &FinishReason::Length
        ));
        assert_eq!(
            ctx.response.final_content,
            super::super::recovery::reasoning_loop_final_response()
        );
        assert!(!ctx.response.final_content_persisted);
        assert_eq!(
            ctx.lifecycle.stop_reason,
            Some(lifecycle::StopReason::LoopEscalation)
        );
    }

    #[test]
    fn ordinary_length_response_is_not_rewritten_without_reasoning_recovery() {
        let mut ctx = TurnContext::new();
        ctx.response.final_content = "partial but useful response".to_string();

        assert!(!terminalize_exhausted_reasoning_response(
            &mut ctx,
            &FinishReason::Length
        ));
        assert_eq!(ctx.response.final_content, "partial but useful response");
        assert_eq!(ctx.lifecycle.stop_reason, None);
    }
}
