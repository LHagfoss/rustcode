use super::TurnContext;
use crate::app::ChatMessage;
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

/// Whether the latest user request explicitly asks the agent to change the
/// workspace. Restrict this to the latest user message so prior conversation
/// turns cannot accidentally turn a read-only investigation into a mutation
/// mandate.
fn explicit_workspace_change_request(prompt: &str) -> bool {
    let normalized = prompt.to_ascii_lowercase();
    if [
        "do not edit",
        "don't edit",
        "do not change",
        "don't change",
        "do not modify",
        "don't modify",
        "without changing",
        "without editing",
        "read-only",
        "read only",
        "review only",
        "only inspect",
        "just inspect",
        "just review",
    ]
    .iter()
    .any(|phrase| normalized.contains(phrase))
        || normalized.contains("how do i")
        || normalized.contains("how can i")
        || normalized.contains("how should i")
        || normalized.contains("what should")
        || normalized.contains("should we")
    {
        return false;
    }

    normalized
        .split(|c: char| !c.is_ascii_alphabetic())
        .filter(|word| !word.is_empty())
        .any(|word| {
            matches!(
                word,
                "add"
                    | "apply"
                    | "create"
                    | "delete"
                    | "edit"
                    | "fix"
                    | "implement"
                    | "modify"
                    | "move"
                    | "patch"
                    | "refactor"
                    | "remove"
                    | "rename"
                    | "replace"
                    | "rework"
                    | "rewrite"
                    | "update"
                    | "write"
                    | "change"
                    | "changes"
            )
        })
}

const OUTSTANDING_ACTION_LOOP_RECOVERY_PROMPT: &str = "The user explicitly requested an external action, and the transcript does not show that action succeeding. Stop researching: do not search, query, browse, or gather more evidence. Use the evidence already gathered and take exactly one next step toward the requested action with the appropriate available tool. Preserve all normal safety, permission, and confirmation requirements; this recovery instruction does not authorize a side effect the user did not request. If required details are missing or the action cannot be completed safely, ask one focused question or explain the blocker instead of calling more research tools.";

fn explicit_external_action_request(prompt: &str) -> bool {
    let normalized = prompt.to_ascii_lowercase();
    if [
        "do not send",
        "don't send",
        "do not email",
        "don't email",
        "draft only",
        "do not post",
        "don't post",
        "how do i",
        "how can i",
        "how should i",
    ]
    .iter()
    .any(|phrase| normalized.contains(phrase))
    {
        return false;
    }

    let words = normalized
        .split(|c: char| !c.is_ascii_alphabetic())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>();
    let direct_mail_action = words.iter().enumerate().any(|(index, word)| {
        if !matches!(*word, "mail" | "email") {
            return false;
        }
        let previous = index.checked_sub(1).and_then(|i| words.get(i));
        !matches!(
            previous.copied(),
            Some("check" | "read" | "search" | "list" | "open" | "my" | "your")
        ) && !matches!(
            words.get(index + 1).copied(),
            Some("mcp" | "tool" | "tools" | "address" | "inbox" | "messages")
        )
    });
    let action_verb = words.iter().any(|word| {
        matches!(
            *word,
            "send"
                | "post"
                | "publish"
                | "upload"
                | "invite"
                | "schedule"
                | "book"
                | "purchase"
                | "buy"
        )
    });
    direct_mail_action || action_verb
}

fn is_external_action_tool(tool_name: &str) -> bool {
    let normalized = tool_name.to_ascii_lowercase();
    let name = normalized.rsplit("__").next().unwrap_or(&normalized);
    [
        "send_email",
        "reply_email",
        "post_message",
        "publish",
        "upload",
        "invite",
        "schedule",
        "book",
        "purchase",
        "buy",
    ]
    .iter()
    .any(|prefix| name == *prefix || name.starts_with(&format!("{prefix}_")))
}

fn successful_external_action_after(history: &[ChatMessage], user_index: usize) -> bool {
    history[user_index + 1..].iter().any(|message| {
        let Some(result) = message.tool_result.as_ref() else {
            return false;
        };
        if !result.success || result.pending {
            return false;
        }
        is_external_action_tool(&result.tool_name)
    })
}

/// Select the recovery policy from the task, while preserving the ordinary
/// recovery path for read-only work and compiler debugging. Compiler state is
/// supplied by the caller because diagnostics can be discovered by a tool
/// result rather than by the user's wording.
pub(super) fn loop_recovery_prompt(
    history: &[ChatMessage],
    made_edits: bool,
    compiler_debugging: bool,
) -> &'static str {
    task_aware_recovery_prompt(
        history,
        made_edits,
        compiler_debugging,
        super::super::LOOP_RECOVERY_PROMPT,
    )
}

fn reasoning_loop_recovery_prompt(
    history: &[ChatMessage],
    made_edits: bool,
    compiler_debugging: bool,
) -> &'static str {
    task_aware_recovery_prompt(
        history,
        made_edits,
        compiler_debugging,
        super::super::REASONING_LOOP_RECOVERY_PROMPT,
    )
}

fn task_aware_recovery_prompt(
    history: &[ChatMessage],
    made_edits: bool,
    compiler_debugging: bool,
    ordinary_prompt: &'static str,
) -> &'static str {
    let latest_user = history
        .iter()
        .enumerate()
        .rev()
        .find(|(_, message)| message.role == "user");
    let explicit_change = latest_user
        .as_ref()
        .is_some_and(|(_, message)| explicit_workspace_change_request(&message.content));
    let outstanding_external_action = latest_user.is_some_and(|(index, message)| {
        explicit_external_action_request(&message.content)
            && !successful_external_action_after(history, index)
    });
    if outstanding_external_action {
        OUTSTANDING_ACTION_LOOP_RECOVERY_PROMPT
    } else if explicit_change && !made_edits && !compiler_debugging {
        super::super::WORKSPACE_CHANGE_LOOP_RECOVERY_PROMPT
    } else {
        ordinary_prompt
    }
}

pub(super) fn completed_inspection_synthesis(
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
        EMPTY_RESPONSE_RECOVERY_PROMPT, LoopRecoveryAction, reasoning_loop_recovery_action,
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
            "Reasoning loop detected during stream (recovery attempt {})",
            ctx.recovery.reasoning_recovery_attempts + 1,
        );
        match reasoning_loop_recovery_action(ctx.recovery.reasoning_recovery_attempts) {
            LoopRecoveryAction::Recover => {
                ctx.recovery.reasoning_recovery_attempts =
                    ctx.recovery.reasoning_recovery_attempts.saturating_add(1);
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
                let recovery_prompt = reasoning_loop_recovery_prompt(
                    &s.history,
                    ctx.progress.made_edits,
                    ctx.compiler.consecutive_diagnostics > 0
                        || ctx.compiler.consecutive_error_gates > 0,
                );
                s.history.push(ChatMessage::new("system", recovery_prompt));
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
    use super::{
        completed_inspection_synthesis, loop_recovery_prompt, reasoning_loop_final_response,
        reasoning_loop_recovery_prompt,
    };
    use crate::app::ChatMessage;
    use crate::app::ToolResultRecord;
    use crate::network::TurnContext;
    use crate::network::stream::{FinalAnswerBoundary, ProviderFinalAnswerState};
    use crate::network::{LOOP_RECOVERY_PROMPT, REASONING_LOOP_RECOVERY_PROMPT};

    fn completed_inspection_context(content: &str) -> TurnContext {
        let mut ctx = TurnContext::new();
        ctx.progress.complete_inspection_results = 4;
        ctx.response.final_content = content.to_string();
        ctx
    }

    #[test]
    fn explicit_change_recovery_requires_mutation_or_focused_exit() {
        let history = vec![ChatMessage::new(
            "user",
            "The current state is clear; maybe we should rework stuff now.",
        )];
        let prompt = loop_recovery_prompt(&history, false, false);
        assert!(prompt.contains("exactly one concrete, safe mutating tool call"));
        assert!(prompt.contains("one focused question"));
        assert!(prompt.contains("Do not inspect, search, reread"));
    }

    #[test]
    fn read_only_and_compiler_recovery_keep_existing_guidance() {
        let read_only_history = vec![ChatMessage::new(
            "user",
            "Inspect the codebase and report the current architecture.",
        )];
        assert_eq!(
            loop_recovery_prompt(&read_only_history, false, false),
            LOOP_RECOVERY_PROMPT
        );

        let question_history = vec![ChatMessage::new(
            "user",
            "What should I change to improve this implementation?",
        )];
        assert_eq!(
            loop_recovery_prompt(&question_history, false, false),
            LOOP_RECOVERY_PROMPT
        );

        assert_eq!(
            reasoning_loop_recovery_prompt(&read_only_history, false, false),
            REASONING_LOOP_RECOVERY_PROMPT
        );

        let change_history = vec![ChatMessage::new("user", "Please rework the parser.")];
        assert_eq!(
            loop_recovery_prompt(&change_history, false, true),
            LOOP_RECOVERY_PROMPT
        );
        assert_eq!(
            loop_recovery_prompt(&change_history, true, false),
            LOOP_RECOVERY_PROMPT
        );
    }

    #[test]
    fn outstanding_external_action_stops_redundant_research() {
        let history = vec![
            ChatMessage::new(
                "user",
                "Check the weather, find a matching beverage, and email the recommendation to Pat.",
            ),
            ChatMessage::new("tool", "weather evidence").with_tool_result(ToolResultRecord {
                tool_name: "search_web".into(),
                success: true,
                ..ToolResultRecord::default()
            }),
            ChatMessage::new("tool", "catalog evidence").with_tool_result(ToolResultRecord {
                tool_name: "sql".into(),
                success: true,
                ..ToolResultRecord::default()
            }),
        ];

        let prompt = loop_recovery_prompt(&history, false, false);
        assert!(prompt.contains("external action"));
        assert!(prompt.contains("Stop researching"));
        assert!(prompt.contains("normal safety, permission, and confirmation"));
    }

    #[test]
    fn completed_external_action_and_read_only_email_question_use_ordinary_recovery() {
        let completed = vec![
            ChatMessage::new("user", "Email Pat the recommendation."),
            ChatMessage::new("tool", "sent").with_tool_result(ToolResultRecord {
                tool_name: "send_email".into(),
                success: true,
                ..ToolResultRecord::default()
            }),
        ];
        assert_eq!(
            loop_recovery_prompt(&completed, false, false),
            LOOP_RECOVERY_PROMPT
        );

        let read_only = vec![ChatMessage::new(
            "user",
            "How do I send email with the available tools?",
        )];
        assert_eq!(
            loop_recovery_prompt(&read_only, false, false),
            LOOP_RECOVERY_PROMPT
        );

        let read_email = vec![
            ChatMessage::new("user", "Read my email and summarize anything urgent."),
            ChatMessage::new("tool", "inbox").with_tool_result(ToolResultRecord {
                tool_name: "read_email".into(),
                success: true,
                ..ToolResultRecord::default()
            }),
        ];
        assert_eq!(
            loop_recovery_prompt(&read_email, false, false),
            LOOP_RECOVERY_PROMPT
        );
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
