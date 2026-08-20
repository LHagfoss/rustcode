use crate::app::{AppState, AppStatus, ChatMessage, StreamTracker};
use std::sync::Arc;
use tokio::sync::Mutex;

use super::events::{self, ToolResult};
use super::lifecycle;
use super::loop_detect;
use super::policy;
use super::runner;
use super::stream::StreamBuffer;
use super::stream_request::{estimate_token_usage, stream_request};
use super::text::{
    self, continuation_nudge_for_category, format_continuation_assistant_message,
    has_intended_tool_call, strip_tool_call_syntax,
};
use super::title::{record_prompt_to_history, spawn_title_generation};
use super::tool_exec::{execute_tool_batch, get_tool_project_root, tool_result_history_message};
use super::verification;
use super::{
    FORCE_ANSWER_PROMPT, LOOP_RECOVERY_PROMPT, LoopRecoveryAction, MAX_REASONING_RECOVERY_ROUNDS,
    REASONING_LOOP_RECOVERY_PROMPT, accumulate_tokens_used, active_todo_checkpoint,
    cached_compiler_check, call_refs_for, compiler_diagnostic_fingerprint,
    completion_block_message, completion_claims_unapplied_work, failure_replan_message,
    fetch_model_quota, is_mutating_tool, loop_recovery_action, mutation_made_progress,
    prepare_turn_request, probe_function_calling, push_or_replace_loop_warning,
    reasoning_loop_recovery_action, record_provider_error, stop_turn_for_budget,
    truncated_batch_summary, turn_budget_exceeded, unanswered_call_results,
    unanswered_call_results_with_kind, update_compiler_diagnostic_streak,
};

/// Tracks fence depth while streaming text so an incomplete ````tool block
/// can be cut mid-flight. Chunk boundaries split markers, so the tail of each
/// chunk is carried into the next comparison.
#[derive(Default)]
pub(crate) struct ToolFenceCounter {
    seen: usize,
    tail: String,
}

impl ToolFenceCounter {
    const MARKER: &'static str = "```tool";

    /// Feeds one streamed chunk and returns the running fence count.
    pub(crate) fn push(&mut self, chunk: &str) -> usize {
        if chunk.is_empty() {
            return self.seen;
        }
        let mut window = std::mem::take(&mut self.tail);
        window.push_str(chunk);
        self.seen += window.matches(Self::MARKER).count();
        let carry = Self::MARKER.len() - 1;
        let mut kept: Vec<char> = window.chars().rev().take(carry).collect();
        kept.reverse();
        self.tail = kept.into_iter().collect();
        self.seen
    }
}

pub struct TurnContext {
    pub tool_rounds: usize,
    pub max_tool_rounds: usize,
    pub oversized_batch_rejections: u8,
    pub loop_detector: loop_detect::LoopDetector,
    pub progress_ledger: loop_detect::ProgressLedger,
    pub reasoning_loop_detector: loop_detect::ReasoningLoopDetector,
    pub loop_recovery_attempts: u8,
    pub reasoning_recovery_attempts: u8,
    pub reasoning_loops_detected: usize,
    pub force_final: bool,
    pub made_edits: bool,
    pub failed_mutations: usize,
    pub completion_blocks: u8,
    pub verification_blocks: u8,
    pub verification: verification::VerificationLedger,
    pub edit_root: Option<std::path::PathBuf>,
    pub compile_dirty: bool,
    pub compile_cache: Option<(std::path::PathBuf, Option<String>)>,
    pub finish_gate_retries: u32,
    pub turn_machine: events::TurnMachine,
    pub last_sent_messages: Vec<serde_json::Value>,
    pub final_content: String,
    pub final_content_persisted: bool,
    pub streamed_call_ids: Vec<String>,
    pub task_completed: bool,
    pub turn_started_at: std::time::Instant,
    pub tokens_used: u64,
    pub consecutive_no_progress: usize,
    pub consecutive_failed_mutations: usize,
    pub consecutive_compiler_error_gates: usize,
    pub consecutive_compiler_diagnostics: usize,
    pub last_compiler_diagnostic_fingerprint: Option<String>,
    pub consecutive_malformed_calls: usize,
    pub last_malformed_call: Option<String>,
    pub budget_stopped: Option<String>,
    pub user_wait_duration: std::time::Duration,
    pub tool_calls: usize,
    pub malformed_calls: usize,
    pub no_progress_results: usize,
    pub failure_replans: usize,
    pub evidence_recoveries: usize,
    pub last_progress_reason: Option<loop_detect::ProgressReason>,
    pub provider_errors: usize,
    pub provider_429s: usize,
    pub changed_paths: std::collections::BTreeSet<String>,
    pub phase_checkpoint: Option<String>,
    pub stop_reason: Option<lifecycle::StopReason>,
}

impl TurnContext {
    pub fn new() -> Self {
        Self::with_max_tool_rounds(crate::config::DEFAULT_MAX_TOOL_ROUNDS)
    }

    pub fn with_max_tool_rounds(max_tool_rounds: usize) -> Self {
        Self {
            tool_rounds: 0,
            max_tool_rounds: max_tool_rounds.max(1),
            oversized_batch_rejections: 0,
            loop_detector: loop_detect::LoopDetector::new(6),
            progress_ledger: loop_detect::ProgressLedger::default(),
            reasoning_loop_detector: loop_detect::ReasoningLoopDetector::default(),
            loop_recovery_attempts: 0,
            reasoning_recovery_attempts: 0,
            reasoning_loops_detected: 0,
            force_final: false,
            made_edits: false,
            failed_mutations: 0,
            completion_blocks: 0,
            verification_blocks: 0,
            verification: verification::VerificationLedger::default(),
            edit_root: None,
            compile_dirty: true,
            compile_cache: None,
            finish_gate_retries: 0,
            turn_machine: events::TurnMachine::new(),
            last_sent_messages: Vec::new(),
            final_content: String::new(),
            final_content_persisted: false,
            streamed_call_ids: Vec::new(),
            task_completed: false,
            turn_started_at: std::time::Instant::now(),
            tokens_used: 0,
            consecutive_no_progress: 0,
            consecutive_failed_mutations: 0,
            consecutive_compiler_error_gates: 0,
            consecutive_compiler_diagnostics: 0,
            last_compiler_diagnostic_fingerprint: None,
            consecutive_malformed_calls: 0,
            last_malformed_call: None,
            budget_stopped: None,
            user_wait_duration: std::time::Duration::ZERO,
            tool_calls: 0,
            malformed_calls: 0,
            no_progress_results: 0,
            failure_replans: 0,
            evidence_recoveries: 0,
            last_progress_reason: None,
            provider_errors: 0,
            provider_429s: 0,
            changed_paths: std::collections::BTreeSet::new(),
            phase_checkpoint: None,
            stop_reason: None,
        }
    }

    pub fn benchmark_summary(&self) -> serde_json::Value {
        serde_json::json!({
            "tool_rounds": self.tool_rounds,
            "tool_calls": self.tool_calls,
            "tokens_used": self.tokens_used,
            "malformed_calls": self.malformed_calls,
            "no_progress_results": self.no_progress_results,
            "failure_replans": self.failure_replans,
            "evidence_recoveries": self.evidence_recoveries,
            "progress_no_information_streak": self.progress_ledger.no_progress_streak(),
            "reasoning_loops_detected": self.reasoning_loops_detected,
            "reasoning_recovery_attempts": self.reasoning_recovery_attempts,
            "last_progress_reason": self.last_progress_reason.map(|reason| reason.label()),
            "compiler_diagnostic_streak": self.consecutive_compiler_diagnostics,
            "provider_errors": self.provider_errors,
            "provider_429s": self.provider_429s,
            "changed_paths": self.changed_paths.iter().collect::<Vec<_>>(),
            "phase_checkpoint": self.phase_checkpoint,
            "stop_reason": self.stop_reason.as_ref().map(ToString::to_string),
        })
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

/// Record a tool-call protocol failure and return whether it is identical to
/// the immediately preceding malformed request. Parsed calls use a stable
/// name/arguments fingerprint; unparseable fences fall back to their bounded
/// raw text so they still consume the same recovery budget.
pub(crate) fn record_malformed_call(
    ctx: &mut TurnContext,
    raw_content: &str,
    calls: &[crate::tools::ToolCall],
) -> bool {
    let fingerprint = if calls.is_empty() {
        raw_content.trim().to_string()
    } else {
        serde_json::to_string(
            &calls
                .iter()
                .map(|call| serde_json::json!({"name": call.name, "arguments": call.arguments}))
                .collect::<Vec<_>>(),
        )
        .unwrap_or_else(|_| raw_content.trim().to_string())
    };
    let repeated = ctx.last_malformed_call.as_deref() == Some(fingerprint.as_str());
    ctx.consecutive_malformed_calls = if repeated {
        ctx.consecutive_malformed_calls.saturating_add(1)
    } else {
        1
    };
    ctx.last_malformed_call = Some(fingerprint);
    ctx.malformed_calls = ctx.malformed_calls.saturating_add(1);
    repeated
}

pub async fn run_single_turn<P: policy::TurnPolicy + 'static>(
    client: &reqwest::Client,
    state: &Arc<Mutex<AppState>>,
    cancel_token: &tokio_util::sync::CancellationToken,
    policy: &Arc<P>,
    stream_buffer: &Arc<Mutex<StreamBuffer>>,
    ctx: &mut TurnContext,
) -> bool {
    const MAX_FINISH_GATE_RETRIES: u32 = 2;
    dbg_log!("Starting agent loop round {}", ctx.tool_rounds);

    if !cancel_token.is_cancelled()
        && let Some(limit) = turn_budget_exceeded(ctx)
    {
        return stop_turn_for_budget(state, ctx, limit).await;
    }

    if !ctx.force_final {
        ctx.stop_reason = None;
    }

    let unprobed = {
        let s = state.lock().await;
        let url = s.api_base_url.clone();
        s.function_calling_unknown(&url)
            .then_some((url, s.model_name.clone()))
    };
    if let Some((url, model)) = unprobed {
        let supported = probe_function_calling(client, state, &url, &model).await;
        let mut s = state.lock().await;
        s.record_function_calling_support(&url, supported);
        dbg_log!(
            "Tool protocol for {}: {:?} (probe said supported={})",
            url,
            s.tool_protocol_for(&url),
            supported
        );
    }

    let msgs = match prepare_turn_request(client, state, ctx.tool_rounds, cancel_token).await {
        Ok(msgs) => msgs,
        Err(error) => {
            dbg_log!("Image fallback failed: {error}");
            let mut s = state.lock().await;
            ctx.stop_reason = Some(if error == "cancelled" {
                lifecycle::StopReason::Cancelled
            } else {
                lifecycle::StopReason::RecoveryFailed
            });
            let notice = if error == "cancelled" {
                "Request cancelled by user".to_string()
            } else {
                format!("Image analysis unavailable: {error}")
            };
            s.history.push(ChatMessage::new("system", notice));
            s.current_token_usage = None;
            return false;
        }
    };

    {
        let mut s = state.lock().await;
        s.current_response.clear();
        s.current_thought_time_ms = 0;
        s.current_thought_tokens = 0;
        s.current_thought_started_at = None;
    }
    stream_buffer.lock().await.reset();

    let (api_base_url, model_name) = {
        let s = state.lock().await;
        (s.api_base_url.clone(), s.model_name.clone())
    };

    let turn_start_time = std::time::Instant::now();

    dbg_log!(
        "Sending request to {} for model {}",
        api_base_url,
        model_name
    );
    ctx.last_sent_messages = msgs.clone();
    let request_client = client.clone();
    let request_state = Arc::clone(state);
    let request_cancel = cancel_token.clone();
    let request_buffer = Arc::clone(stream_buffer);
    let request_api_url = api_base_url.clone();
    let request_model = model_name.clone();
    let request_msgs = msgs.clone();
    let collected_response = match runner::collect_response(move |previous| {
        let mut current_msgs = request_msgs.clone();
        if !previous.is_empty() {
            let continuation_assistant = format_continuation_assistant_message(&previous);
            let nudge = continuation_nudge_for_category(&previous, None);
            current_msgs.push(serde_json::json!({
                "role": "assistant",
                "content": continuation_assistant
            }));
            current_msgs.push(serde_json::json!({
                "role": "user",
                "content": nudge
            }));
        }
        let request_client = request_client.clone();
        let request_state = Arc::clone(&request_state);
        let request_cancel = request_cancel.clone();
        let request_buffer = Arc::clone(&request_buffer);
        let request_api_url = request_api_url.clone();
        let request_model = request_model.clone();
        async move {
            request_buffer.lock().await.reset();
            let finish_reason = stream_request(
                &request_client,
                request_state.clone(),
                request_cancel,
                &request_api_url,
                &request_model,
                &current_msgs,
                Arc::clone(&request_buffer),
                false,
            )
            .await
            .map_err(|e| e.to_string())?;
            let buffer = request_buffer.lock().await;
            Ok(runner::ResponseChunk {
                content: buffer.content.clone(),
                finish_reason,
                has_native_tool_calls: !buffer.native_tool_calls.is_empty(),
                thought_time_ms: buffer.thought_time_ms,
                thought_tokens: buffer.thought_tokens,
            })
        }
    })
    .await
    {
        Ok(result) => result,
        Err(e) => {
            ctx.turn_machine.recover_error();
            dbg_log!("Stream request failed: {}", e);
            if e == "cancelled" {
                ctx.stop_reason = Some(lifecycle::StopReason::Cancelled);
            } else {
                record_provider_error(ctx, &e);
            }
            let mut s = state.lock().await;
            let notice = if e == "cancelled" {
                "Request cancelled by user".to_string()
            } else {
                format!("Error from LLM Provider: {e}")
            };
            s.history.push(ChatMessage::new("system", notice));
            s.current_token_usage = None;
            return false;
        }
    };
    let runner::CollectedResponse {
        content: accumulated_content,
        finish_reason: response_finish_reason,
        thought_time_ms,
        thought_tokens,
    } = collected_response;
    let thought_time_ms = accumulated_content
        .contains("<think>")
        .then_some(thought_time_ms);
    let thought_tokens = accumulated_content
        .contains("<think>")
        .then_some(thought_tokens);

    crate::logger::operational_event(
        "model.response",
        serde_json::json!({
            "round": ctx.tool_rounds,
            "finish_reason": response_finish_reason,
            "content_bytes": accumulated_content.len(),
        }),
    );
    let turn_response_time_ms = turn_start_time.elapsed().as_millis() as u64;

    let turn_token_usage = {
        let s = state.lock().await;
        if s.current_token_usage.is_some() {
            s.current_token_usage.clone()
        } else {
            drop(s);
            let est = estimate_token_usage(&ctx.last_sent_messages, &accumulated_content).await;
            let mut s = state.lock().await;
            s.current_token_usage = est.clone();
            est
        }
    };

    {
        let mut s = state.lock().await;
        s.current_response = accumulated_content.clone();
        let reported = s
            .current_token_usage
            .as_ref()
            .map(|u| u.total_tokens as u64);
        ctx.tokens_used = accumulate_tokens_used(ctx.tokens_used, reported, &accumulated_content);
    }

    if cancel_token.is_cancelled() {
        ctx.stop_reason = Some(lifecycle::StopReason::Cancelled);
        ctx.turn_machine.cancel();
        return false;
    }

    ctx.final_content = accumulated_content;
    ctx.final_content_persisted = false;
    let (native_tool_calls, streamed_call_ids) = {
        let buffer = stream_buffer.lock().await;
        (
            buffer.native_tool_calls.clone(),
            buffer.tool_call_ids.clone(),
        )
    };
    ctx.streamed_call_ids = if native_tool_calls.is_empty() {
        streamed_call_ids
    } else {
        native_tool_calls
            .iter()
            .map(|call| call.call_id.clone())
            .collect()
    };
    dbg_log!(
        "Stream completed successfully. Content length: {} chars",
        ctx.final_content.len()
    );

    if ctx.final_content.is_empty() && native_tool_calls.is_empty() {
        dbg_log!("Stream returned empty content, finishing");
        let mut s = state.lock().await;
        s.current_token_usage = None;
        return false;
    }

    let is_reasoning_loop = response_finish_reason.as_deref() == Some("reasoning_loop");
    if is_reasoning_loop && !ctx.force_final {
        ctx.reasoning_loops_detected += 1;
        dbg_log!(
            "Reasoning loop detected during stream (attempt {}/{})",
            ctx.reasoning_recovery_attempts + 1,
            MAX_REASONING_RECOVERY_ROUNDS
        );
        match reasoning_loop_recovery_action(ctx.reasoning_recovery_attempts) {
            LoopRecoveryAction::Recover => {
                ctx.reasoning_recovery_attempts += 1;
                ctx.reasoning_loop_detector.reset();
                crate::logger::operational_event(
                    "turn.reasoning_loop_recovery",
                    serde_json::json!({
                        "attempt": ctx.reasoning_recovery_attempts,
                    }),
                );
                let mut s = state.lock().await;
                let mut msg = ChatMessage::new("assistant", &ctx.final_content);
                msg.response_time_ms = Some(turn_response_time_ms);
                msg.token_usage = turn_token_usage.clone();
                msg.thought_time_ms = thought_time_ms;
                msg.thought_tokens = thought_tokens;
                s.history.push(msg);
                ctx.final_content_persisted = true;
                s.history.push(ChatMessage::new(
                    "system",
                    REASONING_LOOP_RECOVERY_PROMPT,
                ));
                crate::config::save_history(&s.history);
                s.current_response.clear();
                s.status = AppStatus::Streaming;
                s.stream_tracker = Some(StreamTracker::new());
                drop(s);
                ctx.turn_machine.abandon_tool_phase();
                ctx.tool_rounds += 1;
                return true;
            }
            LoopRecoveryAction::ForceFinal => {
                dbg_log!("Reasoning loop recovery exhausted — forcing wrap-up turn");
                crate::logger::operational_event(
                    loop_detect::DIAG_RECOVERY_EXHAUSTED,
                    serde_json::json!({
                        "attempt": ctx.reasoning_recovery_attempts,
                    }),
                );
                ctx.stop_reason = Some(lifecycle::StopReason::LoopEscalation);
                ctx.force_final = true;
                let mut s = state.lock().await;
                let mut msg = ChatMessage::new("assistant", &ctx.final_content);
                msg.response_time_ms = Some(turn_response_time_ms);
                msg.token_usage = turn_token_usage.clone();
                msg.thought_time_ms = thought_time_ms;
                msg.thought_tokens = thought_tokens;
                s.history.push(msg);
                ctx.final_content_persisted = true;
                s.history.push(ChatMessage::new(
                    "system",
                    FORCE_ANSWER_PROMPT,
                ));
                crate::config::save_history(&s.history);
                s.current_response.clear();
                drop(s);
                ctx.turn_machine.abandon_tool_phase();
                ctx.tool_rounds += 1;
                return true;
            }
        }
    }

    if ctx.force_final {
        dbg_log!("Loop wrap-up: recording forced text answer and finishing");
        let promoted = text::promote_bare_thought_markers(&ctx.final_content);
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
        ctx.final_content = answer;
        return false;
    }

    let protocol = { state.lock().await.active_tool_protocol() };
    let model_response = if matches!(protocol, crate::config::ToolProtocol::ApiNative) {
        let typed_calls = native_tool_calls
            .into_iter()
            .map(|call| crate::tools::ToolCall {
                name: call.tool_name,
                arguments: call.arguments,
                call_id: Some(call.call_id),
            })
            .collect();
        events::native_response(
            &ctx.final_content,
            response_finish_reason.as_deref(),
            typed_calls,
        )
    } else {
        events::normalize_response(
            &ctx.final_content,
            response_finish_reason.as_deref(),
            protocol,
        )
    };
    dbg_log!(
        "Model response normalized from {:?}; raw length={} chars",
        model_response.source,
        model_response.raw_content.len()
    );
    let response_events = model_response.events;
    let parsed_tool_calls = response_events
        .iter()
        .filter_map(|event| match event {
            events::AgentEvent::ToolCall(call) => Some(call.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let requested_calls = parsed_tool_calls.len();
    let (parsed_tool_calls, dropped_calls) = crate::tools::truncate_tool_batch(parsed_tool_calls);
    if dropped_calls > 0 {
        dbg_log!(
            "Oversized batch: running {} of {} requested tool calls",
            parsed_tool_calls.len(),
            requested_calls
        );
        crate::logger::operational_event(
            "tools.batch_truncated",
            serde_json::json!({
                "requested": requested_calls,
                "executed": parsed_tool_calls.len(),
                "dropped": dropped_calls,
            }),
        );
        ctx.final_content = truncated_batch_summary(&parsed_tool_calls, dropped_calls);
    }
    let oversized_batch = dropped_calls > 0;
    if let Err(reason) = crate::tools::validate_tool_calls(&parsed_tool_calls) {
        if lifecycle::is_unavailable_tool_error(&reason) {
            ctx.stop_reason = Some(lifecycle::StopReason::UnavailableTool);
        }
        let raw_content = ctx.final_content.clone();
        let repeated_malformed = record_malformed_call(ctx, &raw_content, &parsed_tool_calls);
        dbg_log!("Tool-call validation rejected response: {}", reason);
        let mut s = state.lock().await;
        let rejected_refs = call_refs_for(&parsed_tool_calls, &ctx.streamed_call_ids);
        let mut msg = ChatMessage::new("assistant", ctx.final_content.clone())
            .with_tool_calls(rejected_refs.clone());
        msg.response_time_ms = Some(turn_response_time_ms);
        msg.token_usage = turn_token_usage.clone();
        msg.thought_time_ms = thought_time_ms;
        msg.thought_tokens = thought_tokens;
        s.history.push(msg);
        ctx.final_content_persisted = true;
        for message in unanswered_call_results_with_kind(
            &rejected_refs,
            &reason,
            crate::tools::ToolErrorKind::Validation,
        ) {
            s.history.push(message);
        }
        let guidance = if oversized_batch {
            ctx.oversized_batch_rejections = ctx.oversized_batch_rejections.saturating_add(1);
            if ctx.oversized_batch_rejections >= 2 {
                ctx.force_final = true;
            }
            format!(
                " This response contained {requested_calls} separate tool calls; the leading {} were kept and the rest dropped, then the remainder failed validation, so nothing ran and nothing it claimed about their results happened. Start again from the last real tool result. Reads may be issued together; keep calls that change the workspace to at most {} per response so each one is grounded in the previous result.",
                parsed_tool_calls.len(),
                crate::tools::MAX_MUTATING_CALLS_PER_RESPONSE
            )
        } else {
            ctx.oversized_batch_rejections = 0;
            String::new()
        };
        let repeat_guidance = if repeated_malformed {
            format!(
                " This is the same invalid tool request repeated {} times. Stop retrying this exact shape; re-read the schema and re-plan, or respond with text explaining what remains.",
                ctx.consecutive_malformed_calls
            )
        } else {
            String::new()
        };
        s.history.push(ChatMessage::new(
                    "system",
                    format!(
                        "[Tool call rejected before execution: {reason}] Emit one corrected tool call.{guidance}{repeat_guidance}"
                    ),
                ));
        crate::config::save_history(&s.history);
        s.current_response.clear();
        s.status = AppStatus::Streaming;
        drop(s);
        ctx.tool_rounds += 1;
        return true;
    }
    ctx.oversized_batch_rejections = 0;
    let (tool_calls, deferred_tool_calls) =
        crate::tools::isolate_control_plane_call(parsed_tool_calls);
    let call_refs = call_refs_for(&tool_calls, &ctx.streamed_call_ids);
    let turn_action = match ctx.turn_machine.model_finished(
        cancel_token.is_cancelled(),
        ctx.force_final,
        !tool_calls.is_empty(),
        ctx.task_completed,
    ) {
        Ok(action) => action,
        Err(invalid) => {
            dbg_log!("Turn machine rejected model_finished: {invalid}");
            crate::logger::operational_event(
                "turn.invalid_transition",
                serde_json::json!({
                    "stage": "model_finished",
                    "detail": invalid.to_string(),
                }),
            );
            events::TurnAction::FinishResponse
        }
    };
    if turn_action == events::TurnAction::Cancel {
        return false;
    }
    if matches!(turn_action, events::TurnAction::ExecuteTools) {
        ctx.consecutive_malformed_calls = 0;
        ctx.last_malformed_call = None;
        dbg_log!("Parsed {} tool call requests", tool_calls.len());

        let mut loop_status = loop_detect::LoopStatus::Ok;
        let mut loop_offender: Option<String> = None;
        for call in &tool_calls {
            let (exact, category) = loop_detect::signatures(&call.name, &call.arguments);
            let s = ctx.loop_detector.check_tool(&call.name, &exact, &category);
            if s.rank() > loop_status.rank() {
                loop_status = s;
                loop_offender = Some(format!("{} ({category})", call.name));
            }
            if is_mutating_tool(&call.name) {
                if let Some(root) = get_tool_project_root(&call.name, &call.arguments) {
                    ctx.edit_root = Some(root);
                    ctx.compile_dirty = true;
                }
            }
        }
        match loop_status {
            loop_detect::LoopStatus::Abort(n) => {
                match loop_recovery_action(ctx.loop_recovery_attempts) {
                    LoopRecoveryAction::Recover => {
                        ctx.loop_recovery_attempts += 1;
                        ctx.loop_detector.reset();
                        dbg_log!(
                            "Loop detector: abort after {} repeats — allowing bounded recovery turn",
                            n
                        );
                        let mut s = state.lock().await;
                        let mut msg = ChatMessage::new("assistant", &ctx.final_content);
                        msg.response_time_ms = Some(turn_response_time_ms);
                        msg.token_usage = turn_token_usage.clone();
                        msg.thought_time_ms = thought_time_ms;
                        msg.thought_tokens = thought_tokens;
                        s.history.push(msg);
                        ctx.final_content_persisted = true;
                        s.history
                            .push(ChatMessage::new("system", LOOP_RECOVERY_PROMPT));
                        crate::config::save_history(&s.history);
                        s.current_response.clear();
                        s.status = AppStatus::Streaming;
                        s.stream_tracker = Some(StreamTracker::new());
                        drop(s);
                        ctx.turn_machine.abandon_tool_phase();
                        ctx.tool_rounds += 1;
                        return true;
                    }
                    LoopRecoveryAction::ForceFinal => {
                        dbg_log!(
                            "Loop detector: abort after {} repeats — forcing wrap-up turn",
                            n
                        );
                        let mut s = state.lock().await;
                        let mut msg = ChatMessage::new("assistant", &ctx.final_content);
                        msg.response_time_ms = Some(turn_response_time_ms);
                        msg.token_usage = turn_token_usage.clone();
                        msg.thought_time_ms = thought_time_ms;
                        msg.thought_tokens = thought_tokens;
                        s.history.push(msg);
                        ctx.final_content_persisted = true;
                        s.history
                            .push(ChatMessage::new("system", FORCE_ANSWER_PROMPT));
                        crate::config::save_history(&s.history);
                        s.current_response.clear();
                        drop(s);
                        ctx.turn_machine.abandon_tool_phase();
                        ctx.stop_reason = Some(lifecycle::StopReason::LoopEscalation);
                        ctx.force_final = true;
                        return true;
                    }
                }
            }
            loop_detect::LoopStatus::Warning(n) => {
                dbg_log!("Loop detector: warning at {} repeats", n);
                let mut s = state.lock().await;
                let action = loop_offender.as_deref().unwrap_or("the last tool action");
                let warning_text = format!(
                    "[Loop warning: '{action}' has repeated {n} times. If a tool edit or view is failing, stop retrying the same inputs — if an edit failed to match, view a wider line range or use grep to verify exact target content.]"
                );
                push_or_replace_loop_warning(&mut s.history, warning_text);
                drop(s);
            }
            loop_detect::LoopStatus::Ok => {}
        }

        if !cancel_token.is_cancelled() {
            ctx.tool_rounds += 1;

            let approved = policy.should_approve(state, &tool_calls).await;

            {
                let mut s = state.lock().await;
                s.pending_tool_confirmation = None;
                s.status = AppStatus::Streaming;
                s.stream_tracker = Some(StreamTracker::new());
                let mut msg = ChatMessage::new("assistant", &ctx.final_content)
                    .with_tool_calls(call_refs.clone());
                msg.response_time_ms = Some(turn_response_time_ms);
                msg.token_usage = turn_token_usage.clone();
                msg.thought_time_ms = thought_time_ms;
                msg.thought_tokens = thought_tokens;
                s.history.push(msg);
                ctx.final_content_persisted = true;
                if dropped_calls > 0 {
                    s.history.push(ChatMessage::new(
                                "system",
                                format!(
                                    "[{dropped_calls} of the {requested_calls} tool calls in that response were dropped; only the first {} ran. Their results follow — plan the next step from those, not from what the response predicted. Reads may be issued together; keep calls that change the workspace to at most {} per response.]",
                                    tool_calls.len(),
                                    crate::tools::MAX_MUTATING_CALLS_PER_RESPONSE
                                ),
                            ));
                }
                crate::config::save_history(&s.history);
            }

            let transition = if approved {
                ctx.turn_machine.approval_granted()
            } else {
                ctx.turn_machine.approval_denied()
            };
            if let Err(invalid) = transition {
                dbg_log!("Turn machine rejected approval transition: {invalid}");
                crate::logger::operational_event(
                    "turn.invalid_transition",
                    serde_json::json!({
                        "stage": "approval",
                        "approved": approved,
                        "detail": invalid.to_string(),
                    }),
                );
            }

            let deferred_notice = (deferred_tool_calls > 0).then(|| {
                format!(
                    "[harness: deferred {deferred_tool_calls} additional tool call(s) until the next model turn after skill loading]"
                )
            });
            let results = execute_tool_batch(
                client,
                state,
                cancel_token,
                &tool_calls,
                ctx.turn_machine.state() == events::TurnState::ExecutingTools,
                &ctx.edit_root,
                &mut ctx.compile_dirty,
                &mut ctx.compile_cache,
                &mut ctx.user_wait_duration,
                deferred_notice,
            )
            .await;

            ctx.tool_calls += results.len();
            let mutation_batch = results
                .iter()
                .any(|result| is_mutating_tool(&result.tool_name));
            if mutation_batch {
                let diagnostics = results
                    .iter()
                    .find_map(|result| compiler_diagnostic_fingerprint(&result.content));
                if diagnostics.is_some() || !ctx.compile_dirty {
                    update_compiler_diagnostic_streak(ctx, diagnostics);
                }
            }

            crate::logger::operational_event(
                "tools.batch.finish",
                serde_json::json!({
                    "count": results.len(),
                    "successes": results.iter().filter(|result| result.metadata.success).count(),
                    "failed": results.iter().filter(|result| !result.metadata.success).count(),
                    "changed_paths": results.iter().map(|result| result.metadata.changed_paths.len()).sum::<usize>(),
                }),
            );

            if cancel_token.is_cancelled() {
                dbg_log!("Orchestrator: Cancelled during tool execution");
                ctx.stop_reason = Some(lifecycle::StopReason::Cancelled);
                let mut s = state.lock().await;
                append_cancelled_batch_results(&mut s.history, results, &call_refs);
                if call_refs.is_empty() {
                    s.history
                        .push(ChatMessage::new("system", "Request cancelled by user"));
                }
                crate::config::save_history(&s.history);
                ctx.turn_machine.finish_tools_if_executing();
                return false;
            }

            let mut s = state.lock().await;
            s.status = AppStatus::Streaming;
            let mut completed = false;
            let mut stagnation = loop_detect::LoopStatus::Ok;
            let mut failure_replan = None;
            let mut evidence_recovery = None;
            let mut cross_turn_made_progress = false;
            let mut cross_turn_had_edits = false;
            let mut cross_turn_target_files = Vec::new();
            let mut cross_turn_tool_count = 0;
            let executed = results.len();
            for (position, result) in results.into_iter().enumerate() {
                let call = tool_calls.get(position);
                let answered_call = call_refs.get(position).map(|call| call.id.clone());
                let name = result.tool_name;
                let mut metadata = result.metadata.clone();
                // The provider call id is attached at the orchestration
                // boundary, after parsing, and is then carried into the
                // history message that answers that exact call.
                metadata.call_id = answered_call.clone();
                let content = result.content;
                let mut verification_command = false;
                if call.is_some_and(|call| call.name == "run_command")
                    && let Some(command) = call
                        .and_then(|call| call.arguments.get("command"))
                        .and_then(|command| command.as_str())
                {
                    ctx.verification.record_command(command, metadata.exit_code);
                    verification_command = verification::is_verification_command(command)
                        || loop_detect::is_stable_inspection_command(command);
                }
                let diff_opt = result.diff;
                dbg_log!(
                    "Tool '{}' finished with result length: {} chars",
                    name,
                    content.len()
                );
                if name == "complete_task" {
                    completed = true;
                }
                ctx.changed_paths
                    .extend(metadata.changed_paths.iter().cloned());
                if name == "todo_write" && metadata.success {
                    ctx.phase_checkpoint = active_todo_checkpoint(&s.todos);
                    if let Some(phase) = ctx.phase_checkpoint.as_deref() {
                        s.history.push(ChatMessage::new(
                            "system",
                            format!("[phase checkpoint: {phase}]"),
                        ));
                    }
                }
                let mut mutation_progress = false;
                if is_mutating_tool(&name) {
                    let failed = !metadata.success
                        || content
                            .trim_start()
                            .to_ascii_lowercase()
                            .starts_with("error");
                    let made_progress = mutation_made_progress(metadata.success, &content);
                    mutation_progress = made_progress && diff_opt.is_some();
                    if failed {
                        ctx.failed_mutations += 1;
                        ctx.consecutive_failed_mutations += 1;
                        if let Some(call) = call {
                            let (exact, category) =
                                loop_detect::signatures(&call.name, &call.arguments);
                            if let loop_detect::LoopStatus::Abort(repeats) =
                                ctx.loop_detector.record_failed_tool(&exact, &category)
                            {
                                failure_replan =
                                    Some(failure_replan_message(&call.name, &category, repeats));
                            }
                        }
                    } else {
                        ctx.made_edits = true;
                        ctx.consecutive_failed_mutations = 0;
                        if !metadata.changed_paths.is_empty() || name != "run_command" {
                            ctx.verification.record_edit();
                        }
                    }
                    if made_progress {
                        ctx.loop_detector.reset();
                        ctx.reasoning_loop_detector.reset();
                        ctx.reasoning_recovery_attempts = 0;
                    }
                }

                let no_result = loop_detect::stagnation_key(&content) == "grep:no-matches";
                let search_result = loop_detect::is_search_tool(&name)
                    || call.is_some_and(|call| {
                        let (_, category) =
                            loop_detect::signatures(&call.name, &call.arguments);
                        category.starts_with("search:")
                    });
                let changed_workspace = mutation_progress && metadata.success;
                let state_fingerprint = changed_workspace.then(|| {
                    let mut state = metadata.changed_paths.join("\n");
                    if let Some(diff) = diff_opt.as_deref() {
                        state.push('\n');
                        state.push_str(diff);
                    }
                    loop_detect::stable_hash(&state)
                });
                let output_fingerprint = compiler_diagnostic_fingerprint(&content)
                    .as_deref()
                    .map(loop_detect::stable_hash)
                    .unwrap_or_else(|| {
                        loop_detect::stable_hash(loop_detect::stagnation_key(&content))
                    });
                let action = call
                    .map(|call| loop_detect::signatures(&call.name, &call.arguments).0)
                    .unwrap_or_else(|| name.clone());
                let failure_fingerprint = (!metadata.success).then(|| {
                    loop_detect::stable_hash(&format!(
                        "{name}:{}:{}",
                        metadata.exit_code.unwrap_or_default(),
                        loop_detect::stagnation_key(&content)
                    ))
                });
                let assessment = ctx.progress_ledger.observe(
                    &loop_detect::ProgressObservation {
                        action,
                        output_fingerprint,
                        state_fingerprint,
                        failure_fingerprint,
                        changed_workspace,
                        fresh_read: loop_detect::is_read_only(&name) && !metadata.replayed,
                        search_result,
                        no_result,
                        verification: verification_command,
                        read_only: loop_detect::is_read_only(&name),
                        replayed: metadata.replayed,
                        success: metadata.success,
                    },
                );
                let target_file = call.and_then(|c| {
                    c.arguments
                        .get("path")
                        .or_else(|| c.arguments.get("target_file"))
                        .or_else(|| c.arguments.get("TargetFile"))
                        .and_then(|v| v.as_str())
                });
                cross_turn_tool_count += 1;
                // Runtime probes, browser checks, and fresh reads can provide
                // decisive evidence without changing a file. The progress
                // ledger already classifies that evidence; carry its result
                // into the cross-turn reasoning detector instead of treating
                // "no workspace diff" as "no progress".
                cross_turn_made_progress |= assessment.meaningful;
                cross_turn_had_edits |= is_mutating_tool(&name);
                if let Some(target_file) = target_file {
                    cross_turn_target_files.push(target_file.to_string());
                }
                ctx.last_progress_reason = Some(assessment.reason);
                if assessment.meaningful {
                    ctx.consecutive_no_progress = 0;
                } else if !assessment.suppress_stagnation {
                    ctx.consecutive_no_progress += 1;
                    ctx.no_progress_results += 1;
                }
                if !assessment.suppress_stagnation
                    && (assessment.reason == loop_detect::ProgressReason::Churn
                        || assessment.streak >= loop_detect::ProgressLedger::RECOVERY_STREAK)
                {
                    evidence_recovery = Some((
                        assessment.reason,
                        assessment.streak,
                        name.clone(),
                    ));
                }
                if !assessment.suppress_stagnation {
                    match ctx
                        .loop_detector
                        .record_output(loop_detect::stagnation_key(&content))
                    {
                        status @ (loop_detect::LoopStatus::Warning(n)
                        | loop_detect::LoopStatus::Abort(n)) => {
                            dbg_log!("Loop detector: output stagnation x{} for '{}'", n, name);
                            if status.rank() > stagnation.rank() {
                                stagnation = status;
                            }
                        }
                        loop_detect::LoopStatus::Ok => {}
                    }
                }
                s.history.push(tool_result_history_message(
                    ToolResult {
                        tool_name: name,
                        content,
                        diff: diff_opt,
                        file_preview: result.file_preview,
                        metadata,
                    },
                    answered_call,
                ));
            }

            // `record_turn_evidence` models a complete model turn. Record the
            // batch once after all results are processed; calling it for each
            // result makes two read-only calls from one response look like two
            // repeated turns and triggers a false cross-turn loop.
            if cross_turn_tool_count > 0 {
                let target_file_refs = cross_turn_target_files
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>();
                let cross_turn_status = ctx.reasoning_loop_detector.record_turn_evidence(
                    &loop_detect::TurnEvidence {
                        reasoning: &ctx.final_content,
                        target_files: &target_file_refs,
                        made_progress: cross_turn_made_progress,
                        had_edits: cross_turn_had_edits,
                        tool_count: cross_turn_tool_count,
                        no_progress_streak: ctx.progress_ledger.no_progress_streak(),
                    },
                );
                if let loop_detect::ReasoningLoopStatus::LoopDetected(reason) = cross_turn_status {
                    ctx.reasoning_loops_detected += 1;
                    dbg_log!("Cross-turn reasoning loop detected: {reason}");
                    crate::logger::operational_event(
                        reason,
                        serde_json::json!({
                            "round": ctx.tool_rounds,
                            "reason": reason,
                        }),
                    );
                    if evidence_recovery.is_none() {
                        evidence_recovery = Some((
                            loop_detect::ProgressReason::NoNewInformation,
                            ctx.reasoning_recovery_attempts as usize + 1,
                            format!("reasoning loop: {reason}"),
                        ));
                    }
                }
            }
            if executed < call_refs.len() {
                for message in
                    unanswered_call_results(&call_refs[executed..], "no result was produced")
                {
                    s.history.push(message);
                }
            }

            if let loop_detect::LoopStatus::Warning(n) | loop_detect::LoopStatus::Abort(n) =
                stagnation
            {
                push_or_replace_loop_warning(
                    &mut s.history,
                    format!(
                        "[Loop warning: the last {n} tool results were identical in kind (e.g. repeated \"no matches\"). Re-phrasing the same search is not progress — the answer is not where you are looking. View the relevant file directly or change approach.]"
                    ),
                );
            }

            let output_abort = matches!(stagnation, loop_detect::LoopStatus::Abort(_));
            if output_abort || evidence_recovery.is_some() {
                let (reason, streak, action) = evidence_recovery.unwrap_or((
                    loop_detect::ProgressReason::NoNewInformation,
                    match stagnation {
                        loop_detect::LoopStatus::Warning(n)
                        | loop_detect::LoopStatus::Abort(n) => n,
                        loop_detect::LoopStatus::Ok => 0,
                    },
                    "repeated tool output".to_string(),
                ));
                let evidence = format!(
                    "[Evidence-based recovery: signal={} streak={} action={}]. Use a different, evidence-producing next step; do not repeat the same unchanged read, no-result search, no-op edit, or failed command.",
                    reason.label(), streak, action
                );
                match loop_recovery_action(ctx.loop_recovery_attempts) {
                    LoopRecoveryAction::Recover => {
                        ctx.loop_recovery_attempts += 1;
                        ctx.evidence_recoveries += 1;
                        ctx.loop_detector.reset();
                        s.history.push(ChatMessage::new(
                            "system",
                            format!("{evidence}\n{LOOP_RECOVERY_PROMPT}"),
                        ));
                        crate::config::save_history(&s.history);
                        s.current_response.clear();
                        s.status = AppStatus::Streaming;
                        s.stream_tracker = Some(StreamTracker::new());
                        drop(s);
                        ctx.turn_machine.finish_tools_if_executing();
                        ctx.tool_rounds += 1;
                        return true;
                    }
                    LoopRecoveryAction::ForceFinal => {
                        crate::logger::operational_event(
                            loop_detect::DIAG_RECOVERY_EXHAUSTED,
                            serde_json::json!({
                                "recovery_attempts": ctx.loop_recovery_attempts,
                                "reason": reason.label(),
                            }),
                        );
                        s.history.push(ChatMessage::new(
                            "system",
                            format!("{evidence}\n{FORCE_ANSWER_PROMPT}"),
                        ));
                        crate::config::save_history(&s.history);
                        s.current_response.clear();
                        drop(s);
                        ctx.stop_reason = Some(lifecycle::StopReason::LoopEscalation);
                        ctx.force_final = true;
                        ctx.turn_machine.finish_tools_if_executing();
                        return true;
                    }
                }
            }

            if let Some(replan) = failure_replan {
                ctx.failure_replans += 1;
                // A replan is a recovery opportunity, not a terminal state.
                // The old path set `force_final` immediately, so the next
                // response's tool call was discarded and a recoverable pair
                // of edit mismatches ended the entire task. Reset the
                // equivalence detector and let the model inspect or choose a
                // different mutation method. The consecutive-failure budget
                // remains intact as the hard backstop.
                ctx.loop_detector.reset();
                s.history.push(ChatMessage::new("system", replan));
                crate::config::save_history(&s.history);
                s.current_response.clear();
                s.status = AppStatus::Streaming;
                s.stream_tracker = Some(StreamTracker::new());
                drop(s);
                ctx.turn_machine.finish_tools_if_executing();
                return true;
            }

            if completed
                && completion_claims_unapplied_work(
                    ctx.made_edits,
                    ctx.failed_mutations,
                    ctx.completion_blocks,
                )
            {
                ctx.completion_blocks += 1;
                dbg_log!(
                    "Completion blocked: {} failed edits, none applied",
                    ctx.failed_mutations
                );
                crate::logger::operational_event(
                    "turn.completion_blocked",
                    serde_json::json!({ "failed_mutations": ctx.failed_mutations }),
                );
                s.history.push(ChatMessage::new(
                    "system",
                    completion_block_message(ctx.failed_mutations),
                ));
                crate::config::save_history(&s.history);
                s.current_response.clear();
                drop(s);
                ctx.turn_machine.finish_tools_if_executing();
                return true;
            }

            const MAX_VERIFICATION_BLOCKS: u8 = 2;
            if completed {
                ctx.stop_reason = Some(lifecycle::StopReason::Completed);
                if ctx.made_edits
                    && !ctx.verification.has_fresh_successful_verification()
                    && ctx.verification_blocks < MAX_VERIFICATION_BLOCKS
                {
                    ctx.verification_blocks += 1;
                    let reason = ctx
                        .verification
                        .last_failure()
                        .map(|evidence| {
                            format!(
                                "The latest verification failed: {} (exit code {:?}).",
                                evidence.command, evidence.exit_code
                            )
                        })
                        .unwrap_or_else(|| {
                            "No verification command was run after the latest edit.".to_string()
                        });
                    s.history.push(ChatMessage::new(
                        "system",
                        format!(
                            "[Finish blocked — {reason} Run the relevant project verification command after the latest edit, inspect its result, then report completion.]"
                        ),
                    ));
                    crate::config::save_history(&s.history);
                    s.current_response.clear();
                    drop(s);
                    ctx.turn_machine.finish_tools_if_executing();
                    return true;
                }
                let mut build_status = if ctx.made_edits {
                    "pending"
                } else {
                    "not run (no workspace edits detected)"
                };
                if ctx.made_edits {
                    {
                        let mut s = state.lock().await;
                        s.status = AppStatus::Streaming;
                    }
                    let root = ctx
                        .edit_root
                        .clone()
                        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
                    if let Some(errors) =
                        cached_compiler_check(&root, &mut ctx.compile_dirty, &mut ctx.compile_cache)
                            .await
                    {
                        if errors.starts_with("__BUILD_UNVERIFIED__") {
                            dbg_log!("complete_task finish gate: build unverified — {errors}");
                            build_status = "unverified";
                            s.history.push(ChatMessage::new(
                                "system",
                                format!("[⚠ Build could not be verified — {errors}]"),
                            ));
                        } else {
                            dbg_log!("complete_task finish gate failed with compiler errors");
                            ctx.consecutive_compiler_error_gates += 1;
                            s.history.push(ChatMessage::new(
                                        "system",
                                        format!(
                                            "[Finish blocked — the build does not compile. You cannot report this \
                                             task as done while there are compiler errors. Fix them, then finish. \
                                             Compiler errors:\n{errors}]"
                                        ),
                                    ));
                            crate::config::save_history(&s.history);
                            s.current_response.clear();
                            drop(s);
                            ctx.turn_machine.finish_tools_if_executing();
                            return true;
                        }
                    } else {
                        build_status = "passed";
                        ctx.consecutive_compiler_error_gates = 0;
                    }
                }

                dbg_log!("complete_task accepted; finalizing the turn");
                let task_result_summary = tool_calls
                    .iter()
                    .find(|call| call.name == "complete_task")
                    .and_then(|call| call.arguments.get("result").and_then(|r| r.as_str()))
                    .map(|s| s.to_string());

                if let Some(mut summary_text) = task_result_summary
                    && !summary_text.is_empty()
                {
                    let mut changed_paths = std::collections::BTreeSet::new();
                    for message in &s.history {
                        if let Some(metadata) = &message.tool_result {
                            changed_paths.extend(metadata.changed_paths.iter().cloned());
                        }
                    }
                    let paths = if changed_paths.is_empty() {
                        "none recorded".to_string()
                    } else {
                        changed_paths.into_iter().collect::<Vec<_>>().join(", ")
                    };
                    summary_text.push_str(&format!(
                        "\n\n[harness verification: build={build_status}; tool_verification={}; changed_paths={paths}]",
                        ctx.verification.summary()
                    ));
                    if !ctx.made_edits && ctx.failed_mutations > 0 {
                        summary_text.push_str(&format!(
                            "\n[harness warning: {} edit(s) failed and none were applied — \
nothing in this summary was written to disk by this task]",
                            ctx.failed_mutations
                        ));
                    }
                    s.history.push(ChatMessage::new("assistant", summary_text));
                }
                drop(s);
                ctx.task_completed = true;
                ctx.turn_machine.finish_tools_if_executing();
                return false;
            }
            crate::config::save_history(&s.history);
            s.current_response.clear();
            drop(s);
            ctx.turn_machine.finish_tools_if_executing();
            dbg_log!("Tool round finished, looping back");
            return true;
        } else {
            dbg_log!("Tool execution cancelled");
            ctx.turn_machine.finish_tools_if_executing();
            return false;
        }
    } else if has_intended_tool_call(&ctx.final_content) {
        dbg_log!("Orchestrator: Detected malformed tool call, auto-correcting and retrying...");
        ctx.tool_rounds += 1;
        let raw_content = ctx.final_content.clone();
        let repeated_malformed = record_malformed_call(ctx, &raw_content, &[]);
        let mut s = state.lock().await;
        let mut msg = ChatMessage::new("assistant", &ctx.final_content);
        msg.response_time_ms = Some(turn_response_time_ms);
        msg.token_usage = turn_token_usage.clone();
        msg.thought_time_ms = thought_time_ms;
        msg.thought_tokens = thought_tokens;
        s.history.push(msg);
        ctx.final_content_persisted = true;

        let reason = crate::tools::diagnose_failed_tool_call(&ctx.final_content)
            .map(|r| format!("{r}\n\n"))
            .unwrap_or_default();
        let feedback = format!(
            "tool_error: The tool call block was malformed or could not be parsed. {reason}\
Please output a single, complete, valid tool call block inside a ```tool fenced block using JSON format:\n\n\
```tool\n\
{{\"name\": \"tool_name\", \"arguments\": {{...}}}}\n\
```\n\n\
Make sure keys are exactly \"name\" and \"arguments\", and do not wrap numbers/booleans in quotes if they are expected as numbers/booleans.{}",
            if repeated_malformed {
                format!(
                    " This malformed request has repeated {} times; stop emitting the same block and re-plan or answer with text.",
                    ctx.consecutive_malformed_calls
                )
            } else {
                String::new()
            }
        );

        s.history.push(ChatMessage::new("tool", feedback));
        crate::config::save_history(&s.history);
        s.current_response.clear();
        s.status = AppStatus::Streaming;
        s.stream_tracker = Some(StreamTracker::new());
        drop(s);
        let _ = ctx.turn_machine.retry_for_finish_gate();
        dbg_log!("Retrying agent loop round due to malformed tool call");
        return true;
    }

    let is_continuous = { state.lock().await.continuous_mode };
    if is_continuous && ctx.tool_rounds > 0 {
        dbg_log!(
            "Continuous mode active, assistant responded with text prose. Ending continuous mode turn."
        );
        let mut s = state.lock().await;
        s.continuous_mode = false;
    } else if is_continuous && ctx.tool_rounds == 0 {
        dbg_log!(
            "Continuous mode active, but assistant gave a plain conversational reply (no tools used). Ending turn."
        );
        let mut s = state.lock().await;
        s.continuous_mode = false;
    }

    if policy.should_verify_completion()
        && ctx.made_edits
        && !ctx.force_final
        && ctx.finish_gate_retries < MAX_FINISH_GATE_RETRIES
    {
        let root = ctx
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
            cached_compiler_check(&root, &mut ctx.compile_dirty, &mut ctx.compile_cache).await
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
                ctx.finish_gate_retries += 1;
                ctx.tool_rounds += 1;
                dbg_log!(
                    "Finish gate: build is RED, forcing a fix round ({}/{})",
                    ctx.finish_gate_retries,
                    MAX_FINISH_GATE_RETRIES
                );
                let mut s = state.lock().await;
                let mut msg = ChatMessage::new("assistant", ctx.final_content.clone());
                msg.response_time_ms = Some(turn_response_time_ms);
                msg.token_usage = turn_token_usage.clone();
                msg.thought_time_ms = thought_time_ms;
                msg.thought_tokens = thought_tokens;
                s.history.push(msg);
                ctx.final_content_persisted = true;
                s.history.push(ChatMessage::new(
                            "system",
                            format!(
                                "[Finish blocked — the build does not compile. You cannot report this \
                                 task as done while there are compiler errors. Fix them, then finish. \
                                 Compiler errors:\n{errors}]"
                            ),
                        ));
                crate::config::save_history(&s.history);
                s.current_response.clear();
                s.status = AppStatus::Streaming;
                s.stream_tracker = Some(StreamTracker::new());
                drop(s);
                if let Err(invalid) = ctx.turn_machine.retry_for_finish_gate() {
                    dbg_log!("Turn machine rejected finish-gate retry: {invalid}");
                    return false;
                }
                return true;
            }
        }
        dbg_log!("Finish gate: build is green, accepting done");
    }

    false
}

pub async fn run_agent_turn<P: policy::TurnPolicy + 'static>(
    client: &reqwest::Client,
    state: &Arc<Mutex<AppState>>,
    cancel_token: &tokio_util::sync::CancellationToken,
    policy: &Arc<P>,
    stream_buffer: &Arc<Mutex<StreamBuffer>>,
) -> TurnContext {
    let prompt_start_time = std::time::Instant::now();
    let mut turn_lifecycle = lifecycle::TurnLifecycle::new();

    let max_tool_rounds = { state.lock().await.config.max_tool_rounds };
    let mut ctx = TurnContext::with_max_tool_rounds(max_tool_rounds);
    while run_single_turn(client, state, cancel_token, policy, stream_buffer, &mut ctx).await {}

    if ctx.stop_reason.is_none() {
        ctx.stop_reason = Some(if ctx.task_completed {
            lifecycle::StopReason::Completed
        } else {
            lifecycle::StopReason::RecoveryFailed
        });
    }
    let stop_reason = ctx
        .stop_reason
        .clone()
        .expect("turn finalization always assigns a stop reason");
    if !turn_lifecycle.mark_finalized() {
        return ctx;
    }
    let had_final_content =
        !ctx.final_content_persisted && !ctx.final_content.trim().is_empty();
    let final_transcript = lifecycle::final_transcript_content(
        ctx.task_completed,
        &ctx.final_content,
        ctx.final_content_persisted,
        &stop_reason,
    );
    if let Some(content) = final_transcript.as_ref()
        && !had_final_content
    {
        ctx.final_content = content.clone();
    }
    crate::logger::operational_event(
        "turn.summary",
        serde_json::json!({
            "completed_task": ctx.task_completed,
            "metrics": ctx.benchmark_summary(),
        }),
    );

    {
        dbg_log!("Finishing agent loop, writing final transcript");
        crate::logger::operational_event(
            "turn.finish",
            serde_json::json!({
                "completed_task": ctx.task_completed,
                "tool_rounds": ctx.tool_rounds,
                "content_bytes": ctx.final_content.len(),
                "cancelled": cancel_token.is_cancelled(),
                "metrics": ctx.benchmark_summary(),
            }),
        );

        let mut s = state.lock().await;
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
                dbg_log!("Estimating token usage...");
                let est = estimate_token_usage(&ctx.last_sent_messages, &ctx.final_content).await;
                dbg_log!("Token usage estimation result: {:?}", est);
                est
            }
        };

        let mut s = state.lock().await;
        if let Some(msg) = s.history.iter_mut().rev().find(|m| m.role == "assistant") {
            if msg.token_usage.is_none() {
                msg.token_usage = usage.clone();
            }
        }

        let active_id = s.active_session_id.clone();
        crate::config::save_session_history(&active_id, &s.history);
        crate::config::flush_history_async();

        s.current_response.clear();
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
    }

    ctx
}

pub async fn process_queue_orchestrator<P: policy::TurnPolicy + 'static>(
    client: reqwest::Client,
    state: Arc<Mutex<AppState>>,
    cancel_token: tokio_util::sync::CancellationToken,
    policy: Arc<P>,
) {
    process_queue_orchestrator_inner(client, state, cancel_token, policy, None).await;
}

pub(crate) async fn process_queue_orchestrator_with_ui_events<P: policy::TurnPolicy + 'static>(
    client: reqwest::Client,
    state: Arc<Mutex<AppState>>,
    cancel_token: tokio_util::sync::CancellationToken,
    policy: Arc<P>,
    ui_events: super::ui_adapter::AgentUiEventSender,
) {
    process_queue_orchestrator_inner(client, state, cancel_token, policy, Some(ui_events)).await;
}

async fn process_queue_orchestrator_inner<P: policy::TurnPolicy + 'static>(
    client: reqwest::Client,
    state: Arc<Mutex<AppState>>,
    cancel_token: tokio_util::sync::CancellationToken,
    policy: Arc<P>,
    ui_events: Option<super::ui_adapter::AgentUiEventSender>,
) {
    dbg_log!("Orchestrator started");
    loop {
        let next_prompt = {
            let mut s = state.lock().await;
            if s.pending_queue.is_empty() {
                dbg_log!("Pending queue empty, setting status to Idle");
                s.status = AppStatus::Idle;
                s.delegation_active = false;
                s.orchestrator_running = false;
                break;
            }
            s.status = AppStatus::Streaming;
            s.generation_start_time = Some(std::time::Instant::now());
            s.stream_tracker = Some(StreamTracker::new());
            s.recent_read_calls.clear();
            s.recent_read_outputs.clear();
            s.read_file_mtimes.clear();
            let prompt = s.pending_queue.remove(0);
            dbg_log!("Popped prompt from queue: '{}'", prompt);
            prompt
        };

        let stream_buffer = Arc::new(Mutex::new(StreamBuffer::new()));
        let is_wakeup = next_prompt.starts_with("__task_wakeup__:");

        let mut is_first_prompt = false;
        if !is_wakeup {
            let s = state.lock().await;
            is_first_prompt = s.history.is_empty();
        }

        record_prompt_to_history(&state, is_wakeup, &next_prompt).await;
        crate::logger::operational_event("turn.start", serde_json::json!({"wakeup": is_wakeup}));

        if is_first_prompt {
            spawn_title_generation(&client, &state, next_prompt.clone()).await;
        }

        if let Some(sender) = ui_events.clone() {
            super::ui_adapter::run_agent_turn_with_events(
                &client,
                &state,
                &cancel_token,
                &policy,
                &stream_buffer,
                next_prompt.clone(),
                sender,
            )
            .await;
        } else {
            run_agent_turn(&client, &state, &cancel_token, &policy, &stream_buffer).await;
        }

        if cancel_token.is_cancelled() {
            dbg_log!("Cancel token is cancelled, exiting orchestrator loop");
            break;
        }
    }
    state.lock().await.orchestrator_running = false;
    dbg_log!("Orchestrator finished");
}
