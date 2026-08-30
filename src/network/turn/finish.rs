use std::sync::Arc;

use tokio::sync::Mutex;

use crate::app::{AppState, AppStatus, ChatMessage, StreamTracker};

use super::super::fetch_model_quota;
use super::super::lifecycle;
use super::super::policy;
use super::super::stream::StreamBuffer;
use super::{TurnContext, run_single_turn};

pub async fn run_agent_turn<P: policy::TurnPolicy + 'static>(
    client: &reqwest::Client,
    state: &Arc<Mutex<AppState>>,
    cancel_token: &tokio_util::sync::CancellationToken,
    policy: &Arc<P>,
    stream_buffer: &Arc<Mutex<StreamBuffer>>,
) -> TurnContext {
    let max_tool_rounds = { state.lock().await.config.max_tool_rounds };
    run_agent_turn_with_context(
        client,
        state,
        cancel_token,
        policy,
        stream_buffer,
        TurnContext::with_max_tool_rounds(max_tool_rounds),
    )
    .await
}

pub(crate) async fn run_agent_turn_with_context<P: policy::TurnPolicy + 'static>(
    client: &reqwest::Client,
    state: &Arc<Mutex<AppState>>,
    cancel_token: &tokio_util::sync::CancellationToken,
    policy: &Arc<P>,
    stream_buffer: &Arc<Mutex<StreamBuffer>>,
    mut ctx: TurnContext,
) -> TurnContext {
    let turn_session_id = state.lock().await.active_session_id.clone();
    let prompt_start_time = std::time::Instant::now();
    let mut turn_lifecycle = lifecycle::TurnLifecycle::new();
    while run_single_turn(client, state, cancel_token, policy, stream_buffer, &mut ctx).await {}

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
            "completed_task": ctx.lifecycle.task_completed,
            "metrics": ctx.benchmark_summary(),
        }),
    );

    dbg_log!("Finishing agent loop, writing final transcript");
    crate::logger::operational_event(
        "turn.finish",
        serde_json::json!({
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
        return ctx;
    }
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
        s.history.push(msg);
    }
    drop(s);

    let usage = {
        let s = state.lock().await;
        if s.current_token_usage.is_some() {
            s.current_token_usage.clone()
        } else {
            drop(s);
            ctx.response.last_token_usage.clone()
        }
    };

    let mut s = state.lock().await;
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
    s.status = AppStatus::Idle;
    s.request_redraw();
    if let Some(u) = &usage {
        crate::config::track_usage(u.prompt_tokens as u64, u.completion_tokens as u64);
    }
    s.current_token_usage = usage;
    drop(s);

    let state_quota = Arc::clone(state);
    let client_quota = client.clone();
    tokio::spawn(async move {
        fetch_model_quota(&client_quota, &state_quota).await;
    });
    let notification = if matches!(stop_reason, lifecycle::StopReason::Cancelled) {
        crate::notifications::FinishedStatus::Cancelled
    } else {
        crate::notifications::FinishedStatus::Success
    };
    let _ = crate::notifications::notify_finished(notification);
    ctx
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FinishGateOutcome {
    Continue,
    Stop,
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_plain_response_finish<P: policy::TurnPolicy + 'static>(
    state: &Arc<Mutex<AppState>>,
    policy: &Arc<P>,
    ctx: &mut TurnContext,
    turn_response_time_ms: u64,
    turn_token_usage: Option<crate::app::TokenUsage>,
    thought_time_ms: Option<u64>,
    thought_tokens: Option<u32>,
) -> FinishGateOutcome {
    const MAX_FINISH_GATE_RETRIES: u32 = 2;
    use super::super::cached_compiler_check;
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
        if let Some(errors) =
            cached_compiler_check(&root, &mut ctx.compiler.dirty, &mut ctx.compiler.cache).await
        {
            if errors.starts_with("__BUILD_UNVERIFIED__") {
                dbg_log!("Finish gate: build unverified — {errors}");
                let mut s = state.lock().await;
                s.history.push(ChatMessage::new(
                    "system",
                    format!("[⚠ Build could not be verified — {errors}]"),
                ));
                crate::config::save_history(&s.history);
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
                crate::config::save_history(&s.history);
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
        }
        dbg_log!("Finish gate: build is green, accepting done");
    }

    FinishGateOutcome::Stop
}
