use super::TurnContext;
use std::hash::{DefaultHasher, Hash, Hasher};

fn malformed_call_fingerprint(raw_content: &str, calls: &[crate::tools::ToolCall]) -> String {
    let mut hasher = DefaultHasher::new();
    if calls.is_empty() {
        raw_content.trim().hash(&mut hasher);
    } else {
        for call in calls {
            call.name.hash(&mut hasher);
            call.arguments.to_string().hash(&mut hasher);
        }
    }
    format!("malformed:{:016x}", hasher.finish())
}

pub(crate) fn record_malformed_call(
    ctx: &mut TurnContext,
    raw_content: &str,
    calls: &[crate::tools::ToolCall],
) -> bool {
    let fingerprint = malformed_call_fingerprint(raw_content, calls);
    let repeated = ctx.recovery.last_malformed_call.as_deref() == Some(fingerprint.as_str());
    ctx.recovery.consecutive_malformed_calls = if repeated {
        ctx.recovery.consecutive_malformed_calls.saturating_add(1)
    } else {
        1
    };
    ctx.recovery.last_malformed_call = Some(fingerprint);
    ctx.metrics.malformed_calls = ctx.metrics.malformed_calls.saturating_add(1);
    repeated
}

pub(super) fn reasoning_loop_final_response() -> &'static str {
    "I stopped after repeated reasoning to avoid looping. Please review the current changes and continue from there."
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ResponseRecoveryOutcome {
    Continue,
    Stop,
    Proceed,
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_response_recovery(
    state: &std::sync::Arc<tokio::sync::Mutex<crate::app::AppState>>,
    ctx: &mut TurnContext,
    native_tool_calls_empty: bool,
    response_finish_reason: Option<&str>,
    turn_response_time_ms: u64,
    turn_token_usage: Option<crate::app::TokenUsage>,
    thought_time_ms: Option<u64>,
    thought_tokens: Option<u32>,
) -> ResponseRecoveryOutcome {
    use super::super::lifecycle;
    use super::super::loop_detect;
    use super::super::text::{self, strip_tool_call_syntax};
    use super::super::{
        EMPTY_RESPONSE_RECOVERY_PROMPT, LoopRecoveryAction, MAX_REASONING_RECOVERY_ROUNDS,
        REASONING_LOOP_RECOVERY_PROMPT, reasoning_loop_recovery_action,
    };
    use crate::app::{AppStatus, ChatMessage, StreamTracker};
    if ctx.response.final_content.is_empty() && native_tool_calls_empty {
        if ctx.recovery.empty_response_recovery_attempts < 1 {
            ctx.recovery.empty_response_recovery_attempts += 1;
            dbg_log!(
                "Stream returned empty content, starting recovery attempt {}/1",
                ctx.recovery.empty_response_recovery_attempts
            );
            crate::logger::operational_event(
                "turn.empty_response_recovery",
                serde_json::json!({
                    "attempt": ctx.recovery.empty_response_recovery_attempts,
                    "after_tool_round": ctx.budget.tool_rounds > 0,
                }),
            );
            let mut s = state.lock().await;
            s.history
                .push(ChatMessage::new("system", EMPTY_RESPONSE_RECOVERY_PROMPT));
            s.current_token_usage = None;
            s.clear_current_response();
            s.status = AppStatus::Streaming;
            s.stream_tracker = Some(StreamTracker::new());
            drop(s);
            ctx.budget.tool_rounds += 1;
            return ResponseRecoveryOutcome::Continue;
        }

        dbg_log!("Stream returned empty content after recovery, finishing");
        let mut s = state.lock().await;
        s.current_token_usage = None;
        return ResponseRecoveryOutcome::Stop;
    }

    let is_reasoning_loop = matches!(
        response_finish_reason,
        Some("reasoning_loop" | "reasoning_budget")
    );
    if is_reasoning_loop && !ctx.recovery.force_final {
        ctx.recovery.reasoning_loops_detected += 1;
        dbg_log!(
            "Reasoning loop detected during stream (attempt {}/{})",
            ctx.recovery.reasoning_recovery_attempts + 1,
            MAX_REASONING_RECOVERY_ROUNDS
        );
        match reasoning_loop_recovery_action(ctx.recovery.reasoning_recovery_attempts) {
            LoopRecoveryAction::Recover => {
                ctx.recovery.reasoning_recovery_attempts += 1;
                ctx.recovery.reasoning_recovery_pending = true;
                ctx.recovery.reasoning_loop_detector.reset();
                crate::logger::operational_event(
                    "turn.reasoning_loop_recovery",
                    serde_json::json!({
                        "attempt": ctx.recovery.reasoning_recovery_attempts,
                        "finish_reason": response_finish_reason,
                    }),
                );
                let mut s = state.lock().await;
                let mut msg = ChatMessage::new("assistant", &ctx.response.final_content);
                msg.response_time_ms = Some(turn_response_time_ms);
                msg.token_usage = turn_token_usage.clone();
                msg.thought_time_ms = thought_time_ms;
                msg.thought_tokens = thought_tokens;
                s.history.push(msg);
                ctx.response.final_content_persisted = true;
                s.history
                    .push(ChatMessage::new("system", REASONING_LOOP_RECOVERY_PROMPT));
                crate::config::save_history(&s.history);
                s.clear_current_response();
                s.status = AppStatus::Streaming;
                s.stream_tracker = Some(StreamTracker::new());
                drop(s);
                ctx.lifecycle.turn_machine.abandon_tool_phase();
                ctx.budget.tool_rounds += 1;
                return ResponseRecoveryOutcome::Continue;
            }
            LoopRecoveryAction::ForceFinal => {
                dbg_log!("Reasoning loop recovery exhausted — returning concise final response");
                crate::logger::operational_event(
                    loop_detect::DIAG_RECOVERY_EXHAUSTED,
                    serde_json::json!({
                        "attempt": ctx.recovery.reasoning_recovery_attempts,
                    }),
                );
                ctx.lifecycle.stop_reason = Some(lifecycle::StopReason::LoopEscalation);
                let mut s = state.lock().await;
                let mut msg = ChatMessage::new("assistant", &ctx.response.final_content);
                msg.response_time_ms = Some(turn_response_time_ms);
                msg.token_usage = turn_token_usage.clone();
                msg.thought_time_ms = thought_time_ms;
                msg.thought_tokens = thought_tokens;
                s.history.push(msg);
                ctx.response.final_content_persisted = true;
                crate::config::save_history(&s.history);
                s.clear_current_response();
                drop(s);
                ctx.lifecycle.turn_machine.abandon_tool_phase();
                ctx.response.final_content = reasoning_loop_final_response().to_string();
                ctx.response.final_content_persisted = false;
                return ResponseRecoveryOutcome::Stop;
            }
        }
    }

    if ctx.recovery.force_final {
        dbg_log!("Loop wrap-up: recording forced text answer and finishing");
        let promoted = text::promote_bare_thought_markers(&ctx.response.final_content);
        let prose = strip_tool_call_syntax(&text::strip_think_blocks(&promoted));
        let clean_prose = prose
            .lines()
            .filter(|line| {
                !line.trim().starts_with("- ")
                    && !line.contains("system directive")
                    && !line.contains("CRITICAL — you are stuck")
            })
            .collect::<Vec<_>>()
            .join("\n");
        let answer = if clean_prose.trim().is_empty() {
            "I encountered a repeating loop while running tool actions and have stopped to prevent unnecessary repetition. I was unable to complete the task automatically. Please check the current changes or re-run with a more specific prompt."
                        .to_string()
        } else {
            clean_prose.trim().to_string()
        };
        ctx.response.final_content = answer;
        return ResponseRecoveryOutcome::Stop;
    }
    ResponseRecoveryOutcome::Proceed
}
