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

fn completed_inspection_synthesis(
    ctx: &TurnContext,
    content: &str,
    native_tool_calls_empty: bool,
    final_answer_boundary: super::super::stream::FinalAnswerBoundary,
    provider_final_answer_state: super::super::stream::ProviderFinalAnswerState,
) -> Option<String> {
    // A reasoning/content transition only identifies where answer text starts.
    // Promotion also requires the provider's terminal state; otherwise a
    // loop/budget stop can carry plausible-looking prose through this path.
    if !native_tool_calls_empty
        || final_answer_boundary != super::super::stream::FinalAnswerBoundary::ReasoningClosed
        || provider_final_answer_state != super::super::stream::ProviderFinalAnswerState::Terminal
        || ctx.progress.made_edits
        || ctx.progress.failed_mutations > 0
        || ctx.progress.complete_inspection_results == 0
        || ctx.progress.incomplete_inspection_results > 0
        || crate::network::text::has_intended_tool_call(content)
    {
        return None;
    }

    let candidate = crate::network::text::strip_tool_call_syntax(
        &crate::network::text::strip_think_blocks(content),
    );
    let candidate = candidate.trim();
    (!candidate.is_empty()).then(|| candidate.to_string())
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
    final_answer_boundary: super::super::stream::FinalAnswerBoundary,
    provider_final_answer_state: super::super::stream::ProviderFinalAnswerState,
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
        if let Some(summary) = completed_inspection_synthesis(
            ctx,
            &ctx.response.final_content,
            native_tool_calls_empty,
            final_answer_boundary,
            provider_final_answer_state,
        ) {
            dbg_log!(
                "Reasoning loop followed complete read-only inspection; preserving final synthesis"
            );
            let mut s = state.lock().await;
            let mut message = ChatMessage::new("assistant", &summary);
            message.response_time_ms = Some(turn_response_time_ms);
            message.token_usage = turn_token_usage;
            message.thought_time_ms = thought_time_ms;
            message.thought_tokens = thought_tokens;
            s.history.push(message);
            ctx.response.final_content = summary;
            ctx.response.final_content_persisted = true;
            ctx.lifecycle.task_completed = true;
            ctx.lifecycle.stop_reason = Some(lifecycle::StopReason::Completed);
            crate::config::save_history(&s.history);
            s.clear_current_response();
            drop(s);
            ctx.lifecycle.turn_machine.abandon_tool_phase();
            return ResponseRecoveryOutcome::Stop;
        }
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

#[cfg(test)]
mod tests {
    use super::{completed_inspection_synthesis, reasoning_loop_final_response};
    use crate::network::TurnContext;
    use crate::network::stream::{FinalAnswerBoundary, ProviderFinalAnswerState};

    fn completed_inspection_context(content: &str) -> TurnContext {
        let mut ctx = TurnContext::new();
        ctx.progress.complete_inspection_results = 4;
        ctx.response.final_content = content.to_string();
        ctx
    }

    #[test]
    fn completed_inspection_preserves_final_synthesis_after_reasoning_loop() {
        let ctx = completed_inspection_context(
            "<think>Reviewed the complete source tree.</think>Findings: src/app.ts has an unchecked export input and src/db.ts lacks a transaction around the write.",
        );
        let summary = completed_inspection_synthesis(
            &ctx,
            &ctx.response.final_content,
            true,
            FinalAnswerBoundary::ReasoningClosed,
            ProviderFinalAnswerState::Terminal,
        )
        .expect("complete inspection should yield a usable review");
        assert!(summary.contains("Findings:"));
        assert!(!summary.contains(reasoning_loop_final_response()));
    }

    #[test]
    fn incomplete_inspection_never_promotes_loop_synthesis() {
        let mut ctx = completed_inspection_context(
            "<think>Reviewed the source tree. Findings: the final file is still truncated and needs another inspection before a safe review.</think>",
        );
        ctx.progress.incomplete_inspection_results = 1;
        assert!(
            completed_inspection_synthesis(
                &ctx,
                &ctx.response.final_content,
                true,
                FinalAnswerBoundary::None,
                ProviderFinalAnswerState::None,
            )
            .is_none()
        );
    }

    #[test]
    fn synthesis_with_tool_calls_never_finishes_the_turn() {
        let ctx = completed_inspection_context(
            "<think>Findings are ready.</think>\n```tool\n{\"name\":\"view_file\"}\n```",
        );
        assert!(
            completed_inspection_synthesis(
                &ctx,
                &ctx.response.final_content,
                false,
                FinalAnswerBoundary::ReasoningClosed,
                ProviderFinalAnswerState::Terminal,
            )
            .is_none()
        );
    }

    #[test]
    fn arbitrary_long_prose_is_not_a_final_synthesis() {
        let ctx = completed_inspection_context(
            "I reviewed the project carefully and considered the available evidence before deciding that more thought would be useful before presenting anything to the user.",
        );
        assert!(
            completed_inspection_synthesis(
                &ctx,
                &ctx.response.final_content,
                true,
                FinalAnswerBoundary::None,
                ProviderFinalAnswerState::None,
            )
            .is_none()
        );
    }

    #[test]
    fn loop_stop_reasoning_is_not_a_final_synthesis() {
        let ctx = completed_inspection_context(
            "The review is complete for src/app.ts. The model stopped after repeated reasoning because the reasoning became repetitive and did not produce a trustworthy final report.",
        );
        assert!(
            completed_inspection_synthesis(
                &ctx,
                &ctx.response.final_content,
                true,
                FinalAnswerBoundary::None,
                ProviderFinalAnswerState::None,
            )
            .is_none()
        );
    }

    #[test]
    fn marker_and_path_do_not_rescue_explicit_non_result_prose() {
        let ctx = completed_inspection_context(
            "Findings: src/app.ts was inspected. Review complete. I kept reconsidering the same conclusion, then halted without producing a trustworthy report. No changes were made and no actionable result was produced.",
        );
        assert!(
            completed_inspection_synthesis(
                &ctx,
                &ctx.response.final_content,
                true,
                FinalAnswerBoundary::None,
                ProviderFinalAnswerState::None,
            )
            .is_none()
        );
    }

    #[test]
    fn concise_actionable_findings_still_pass() {
        let ctx = completed_inspection_context(
            "Findings: src/app.ts has an unchecked export input; src/db.ts lacks transaction handling.",
        );
        let summary = completed_inspection_synthesis(
            &ctx,
            &ctx.response.final_content,
            true,
            FinalAnswerBoundary::ReasoningClosed,
            ProviderFinalAnswerState::Terminal,
        );
        assert!(summary.is_some());
    }

    #[test]
    fn failed_mutation_blocks_read_only_review_completion() {
        let mut ctx = completed_inspection_context(
            "<think>Reviewed source.</think>Findings: src/app.ts has an unchecked export input and the review supports this conclusion.",
        );
        ctx.progress.failed_mutations = 1;
        assert!(
            completed_inspection_synthesis(
                &ctx,
                &ctx.response.final_content,
                true,
                FinalAnswerBoundary::ReasoningClosed,
                ProviderFinalAnswerState::Terminal,
            )
            .is_none()
        );
    }

    #[test]
    fn marker_path_and_actionable_words_without_terminal_provider_state_do_not_finish() {
        let ctx = completed_inspection_context(
            "Findings: src/app.ts has no issue. I got stuck in a loop and stopped.",
        );
        assert!(
            completed_inspection_synthesis(
                &ctx,
                &ctx.response.final_content,
                true,
                FinalAnswerBoundary::ReasoningClosed,
                ProviderFinalAnswerState::None,
            )
            .is_none()
        );
    }

    #[test]
    fn valid_concise_review_requires_final_boundary() {
        let ctx = completed_inspection_context(
            "Findings: src/app.ts has no issue after checking its input validation and error handling.",
        );
        assert!(
            completed_inspection_synthesis(
                &ctx,
                &ctx.response.final_content,
                true,
                FinalAnswerBoundary::ReasoningClosed,
                ProviderFinalAnswerState::Terminal,
            )
            .is_some()
        );
    }
}
