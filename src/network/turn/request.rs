use std::borrow::Cow;
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::app::{AppState, ChatMessage, TokenUsage};

use super::super::lifecycle;
use super::super::runner;
use super::super::stream::{FinalAnswerBoundary, ProviderFinalAnswerState, StreamBuffer};
use super::super::stream_request::{estimate_token_usage, stream_request};
use super::super::{
    accumulate_tokens_used, prepare_turn_request_with_checkpoint_and_prefix_cache,
    probe_function_calling, record_provider_error,
};
use super::TurnContext;

use crate::network::text::{
    continuation_nudge_for_category, format_continuation_assistant_message,
};

const MAX_STREAM_RECOVERY_CHECKPOINT_BYTES: usize = 16 * 1024;
const MAX_STREAM_RECOVERY_ERROR_BYTES: usize = 512;
const MAX_STREAM_RECOVERY_ATTEMPTS: u8 = 1;

fn is_recovery_request(ctx: &TurnContext) -> bool {
    ctx.recovery.force_final
        || ctx.recovery.reasoning_recovery_pending
        || ctx.recovery.loop_recovery_attempts > 0
        || ctx.recovery.reasoning_recovery_attempts > 0
        || ctx.recovery.empty_response_recovery_attempts > 0
        || ctx.recovery.completion_blocks > 0
        || ctx.recovery.consecutive_malformed_calls > 0
        || ctx.recovery.oversized_batch_rejections > 0
        || ctx.recovery.laya_pending_recovery_advisory.is_some()
}

fn prepare_request_steerability(state: &mut AppState, ctx: &TurnContext, turn_session_id: &str) {
    if is_recovery_request(ctx) {
        super::clear_turn_steerability_for_session(state, turn_session_id);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RoundCollectionError {
    Stop,
}

pub(super) fn messages_for_response_continuation<'a>(
    base: &'a [serde_json::Value],
    previous: &str,
) -> Cow<'a, [serde_json::Value]> {
    if previous.is_empty() {
        return Cow::Borrowed(base);
    }
    let mut messages = Vec::with_capacity(base.len() + 2);
    messages.extend(base.iter().cloned());
    messages.push(serde_json::json!({
        "role": "assistant",
        "content": format_continuation_assistant_message(previous),
    }));
    messages.push(serde_json::json!({
        "role": "user",
        "content": continuation_nudge_for_category(previous, None),
    }));
    Cow::Owned(messages)
}

pub(super) struct RoundResponse {
    pub content: String,
    pub final_answer_boundary: FinalAnswerBoundary,
    pub provider_final_answer_state: ProviderFinalAnswerState,
    pub finish_reason: Option<String>,
    pub stream_termination: Option<lifecycle::StreamTermination>,
    pub response_time_ms: u64,
    pub token_usage: Option<TokenUsage>,
    pub thought_time_ms: Option<u64>,
    pub thought_tokens: Option<u32>,
    pub native_tool_calls: Vec<crate::tools::ToolCallEnvelope>,
}

fn retryable_stream_failure(message: &str) -> bool {
    matches!(
        lifecycle::stream_failure_kind_from_message(message),
        Some(
            lifecycle::StreamFailureKind::FirstEventTimeout
                | lifecycle::StreamFailureKind::StreamIdleTimeout
                | lifecycle::StreamFailureKind::PrematureEof
                | lifecycle::StreamFailureKind::ResponseBodyDecode
        )
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StreamOutputPhase {
    BeforeOutput,
    TextOutput,
    ToolCall,
}

impl StreamOutputPhase {
    fn label(self) -> &'static str {
        match self {
            Self::BeforeOutput => "before_output",
            Self::TextOutput => "text_output",
            Self::ToolCall => "tool_call",
        }
    }
}

fn stream_output_phase(error: &runner::ResponseError) -> StreamOutputPhase {
    if !error.partial_native_tool_calls.is_empty()
        || crate::network::text::has_intended_tool_call(&error.partial_content)
    {
        StreamOutputPhase::ToolCall
    } else if error.partial_content.is_empty() {
        StreamOutputPhase::BeforeOutput
    } else {
        StreamOutputPhase::TextOutput
    }
}

fn should_retry_stream_transport(error: &runner::ResponseError, attempts: usize) -> bool {
    attempts < usize::from(MAX_STREAM_RECOVERY_ATTEMPTS)
        && retryable_stream_failure(&error.to_string())
        && matches!(stream_output_phase(error), StreamOutputPhase::BeforeOutput)
}

fn bounded_stream_recovery_checkpoint(content: &str) -> String {
    if content.len() <= MAX_STREAM_RECOVERY_CHECKPOINT_BYTES {
        return content.to_owned();
    }

    const MARKER_PREFIX: &str = "\n[partial provider response truncated by harness; ";
    const MARKER_SUFFIX: &str = " bytes omitted]";
    let preview_limit = MAX_STREAM_RECOVERY_CHECKPOINT_BYTES
        .saturating_sub(MARKER_PREFIX.len() + MARKER_SUFFIX.len() + 20);
    let preview_end = content.floor_char_boundary(preview_limit.min(content.len()));
    let marker = format!(
        "{MARKER_PREFIX}{}{}",
        content.len().saturating_sub(preview_end),
        MARKER_SUFFIX
    );
    format!("{}{}", &content[..preview_end], marker)
}

fn recoverable_textual_stream_failure(content: &str) -> bool {
    if content.trim().is_empty() || !crate::network::text::has_intended_tool_call(content) {
        return false;
    }

    // A transport failure happens before the turn parser/dispatcher sees the
    // response. Preserve both incomplete and apparently complete textual
    // envelopes as unexecuted checkpoints: a stream can fail just after the
    // closing brace, and replaying that text as a completed tool call would be
    // indistinguishable from a mutation that actually ran.
    crate::tools::has_incomplete_actionable_tool_call(content)
        || !crate::tools::parse_tool_calls(content, crate::config::ToolProtocol::Native).is_empty()
        || !crate::tools::parse_tool_calls(content, crate::config::ToolProtocol::Json).is_empty()
}

fn bounded_error_detail(error: &runner::ResponseError) -> String {
    let detail = error.to_string();
    let end = detail.floor_char_boundary(MAX_STREAM_RECOVERY_ERROR_BYTES.min(detail.len()));
    if end == detail.len() {
        detail
    } else {
        format!("{}…", &detail[..end])
    }
}

fn native_stream_checkpoint_content(
    checkpoints: &[super::super::stream::NativeToolCallCheckpoint],
    error: &runner::ResponseError,
) -> String {
    let mut content = String::from(
        "[Partial ApiNative tool-call checkpoint: no tool from this failed stream was executed. This is diagnostic state only; do not dispatch or replay it as a completed call.]",
    );
    for (position, checkpoint) in checkpoints.iter().enumerate() {
        use std::fmt::Write;
        let _ = write!(
            content,
            "\n- call {}: id={}, tool={}, arguments_received={}, complete={}, overflowed={}, fingerprint={}",
            checkpoint.index.unwrap_or(position),
            checkpoint.call_id.as_deref().unwrap_or("missing"),
            if checkpoint.tool_name.is_empty() {
                "missing"
            } else {
                checkpoint.tool_name.as_str()
            },
            checkpoint.argument_bytes,
            checkpoint.arguments_complete,
            checkpoint.arguments_overflowed,
            checkpoint.argument_fingerprint,
        );
        if !checkpoint.diagnostic.is_empty() {
            let _ = write!(content, ", diagnostic={}", checkpoint.diagnostic);
        }
    }
    let _ = std::fmt::Write::write_fmt(
        &mut content,
        format_args!("\nProvider detail: {}", bounded_error_detail(error)),
    );
    bounded_stream_recovery_checkpoint(&content)
}

async fn checkpoint_native_stream_recovery(
    state: &Arc<Mutex<AppState>>,
    checkpoints: &[super::super::stream::NativeToolCallCheckpoint],
    error: &runner::ResponseError,
) {
    let content = native_stream_checkpoint_content(checkpoints, error);
    let notice = format!(
        "[Recoverable provider interruption: the ApiNative tool-call stream failed during a tool call. No tool ran and the bounded call identity/diagnostic checkpoint was saved. It will not be replayed. To continue safely, send `continue` or run `rustcode --resume`, then issue a fresh complete tool call if the action is still needed. Provider detail: {}]",
        bounded_error_detail(error)
    );
    let mut s = state.lock().await;
    s.replace_current_response(content.clone());
    s.history
        .push(ChatMessage::new("assistant", content).as_unexecuted_tool_call_checkpoint());
    s.history.push(ChatMessage::new("system", notice));
    let active_id = s.active_session_id.clone();
    crate::config::save_session_history(&active_id, &s.history);
    s.current_token_usage = None;
    s.status = crate::app::AppStatus::Streaming;
    s.stream_tracker = Some(crate::app::StreamTracker::new());
}

async fn checkpoint_stream_recovery(
    state: &Arc<Mutex<AppState>>,
    content: String,
    error: &runner::ResponseError,
) {
    let content = bounded_stream_recovery_checkpoint(&content);
    let notice = format!(
        "[Recoverable provider interruption: the response stream failed during a partial textual tool call. The partial response was saved as an unexecuted checkpoint; no tool call from it ran and it will not be replayed. To continue safely, send `continue` or run `rustcode --resume`, then issue a fresh complete tool call if the action is still needed. Provider detail: {}]",
        bounded_error_detail(error)
    );
    let mut s = state.lock().await;
    s.replace_current_response(content.clone());
    s.history
        .push(ChatMessage::new("assistant", content).as_unexecuted_tool_call_checkpoint());
    s.history.push(ChatMessage::new("system", notice));
    let active_id = s.active_session_id.clone();
    crate::config::save_session_history(&active_id, &s.history);
    s.current_token_usage = None;
    s.status = crate::app::AppStatus::Streaming;
    s.stream_tracker = Some(crate::app::StreamTracker::new());
}

async fn checkpoint_text_stream_recovery(
    state: &Arc<Mutex<AppState>>,
    content: String,
    error: &runner::ResponseError,
) {
    let content = bounded_stream_recovery_checkpoint(&content);
    let notice = format!(
        "[Recoverable provider interruption: the response stream failed during text output. The partial response was saved, no tool ran, and RustCode will not replay the emitted bytes. To continue from this checkpoint, send `continue` or run `rustcode --resume`. Provider detail: {}]",
        bounded_error_detail(error)
    );
    let mut s = state.lock().await;
    s.replace_current_response(content.clone());
    s.history.push(ChatMessage::new("assistant", content));
    s.history.push(ChatMessage::new("system", notice));
    let active_id = s.active_session_id.clone();
    crate::config::save_session_history(&active_id, &s.history);
    s.current_token_usage = None;
    s.status = crate::app::AppStatus::Streaming;
    s.stream_tracker = Some(crate::app::StreamTracker::new());
}

fn stream_interruption_notice(
    error: &runner::ResponseError,
    phase: StreamOutputPhase,
    retry_used: bool,
) -> String {
    let action = if retry_used {
        "RustCode already used its one safe automatic retry. Send `continue` or run `rustcode --resume` to try again."
    } else {
        "Send `continue` or run `rustcode --resume` to try again."
    };
    format!(
        "[Recoverable provider interruption: the SSE stream failed {phase} before a complete response was available. No tool from this failed stream ran; emitted bytes will not be replayed. {action} Provider detail: {}]",
        bounded_error_detail(error),
        phase = phase.label(),
    )
}

pub(super) async fn collect_round(
    client: &reqwest::Client,
    state: &Arc<Mutex<AppState>>,
    cancel_token: &tokio_util::sync::CancellationToken,
    stream_buffer: &Arc<Mutex<StreamBuffer>>,
    ctx: &mut TurnContext,
    turn_session_id: &str,
) -> Result<RoundResponse, RoundCollectionError> {
    {
        let mut s = state.lock().await;
        prepare_request_steerability(&mut s, ctx, turn_session_id);
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

    let checkpoint = ctx.context_checkpoint();
    let msgs = match prepare_turn_request_with_checkpoint_and_prefix_cache(
        client,
        state,
        ctx.budget.tool_rounds,
        cancel_token,
        checkpoint,
        Some(&mut ctx.request_prefix_cache),
    )
    .await
    {
        Ok(msgs) => msgs,
        Err(error) => {
            dbg_log!("Image fallback failed: {error}");
            let mut s = state.lock().await;
            if error.starts_with(crate::network::CONTEXT_PREFLIGHT_STOP_PREFIX) {
                let notice = error
                    .trim_start_matches(crate::network::CONTEXT_PREFLIGHT_STOP_PREFIX)
                    .trim()
                    .to_owned();
                ctx.response.final_content = notice;
                ctx.response.final_content_persisted = true;
                ctx.lifecycle.stop_reason = Some(lifecycle::StopReason::BudgetExceeded(
                    "context preflight exceeded".to_owned(),
                ));
                s.current_token_usage = None;
                return Err(RoundCollectionError::Stop);
            }
            ctx.lifecycle.stop_reason = Some(if error == "cancelled" {
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
            return Err(RoundCollectionError::Stop);
        }
    };

    {
        let mut s = state.lock().await;
        s.clear_current_response();
        // Usage belongs to one provider request. Clear the previous round's
        // value before a new request so an omitted usage footer cannot be
        // mistaken for a repeated report and counted twice.
        s.current_token_usage = None;
        s.current_thought_time_ms = 0;
        s.current_thought_tokens = 0;
        s.current_thought_started_at = None;
    }
    stream_buffer.lock().await.reset();
    let (api_base_url, model_name, request_schema_policy, request_session_id) = {
        let s = state.lock().await;
        let protocol = s.active_tool_protocol();
        let local_model = s.active_model_is_local();
        let compact_tool_prompt = s
            .active_model_profile()
            .as_ref()
            .map(|profile| profile.compact_tool_prompt.unwrap_or(local_model))
            .unwrap_or(local_model)
            && !matches!(protocol, crate::config::ToolProtocol::ApiNative);
        (
            s.api_base_url.clone(),
            s.model_name.clone(),
            crate::tools::ToolSchemaPolicy::root_for_mode_with_compact_prompt(
                s.delegation_active,
                s.agent_mode,
                compact_tool_prompt,
            ),
            s.active_session_id.clone(),
        )
    };
    let turn_start_time = std::time::Instant::now();
    dbg_log!(
        "Sending request to {} for model {}",
        api_base_url,
        model_name
    );
    ctx.response.last_token_usage = None;
    let request_msgs: Arc<[serde_json::Value]> = msgs.into();
    let token_estimate_messages = Arc::clone(&request_msgs);
    let request_client = client.clone();
    let request_state = Arc::clone(state);
    let request_cancel = cancel_token.clone();
    let request_buffer = Arc::clone(stream_buffer);
    let request_allow_tools = !ctx.recovery.force_final;
    let request_thinking_mode = if ctx.recovery.force_final {
        super::super::stream_request::ThinkingMode::Disabled
    } else if std::mem::take(&mut ctx.recovery.reasoning_recovery_pending) {
        super::super::stream_request::ThinkingMode::BoundedRecovery
    } else {
        super::super::stream_request::ThinkingMode::Normal
    };
    // A body stall can happen after bytes have arrived, so the request-level
    // retry policy cannot safely replay it. Retry once from the last coherent
    // history checkpoint, after clearing speculative UI output. No tool has
    // executed yet at this point, so this cannot duplicate a mutation.
    let (adaptive_tool_output_limit, hard_context_limit, max_continuations) = {
        let s = request_state.lock().await;
        let profile = s
            .config
            .models
            .iter()
            .find(|profile| profile.matches_request(&api_base_url, &model_name));
        (
            profile
                .filter(|profile| {
                    request_allow_tools
                        && request_thinking_mode
                            == super::super::stream_request::ThinkingMode::Normal
                        && profile.verified_tool_output_ceiling().is_some()
                })
                .map(|profile| profile.tool_output_ceiling()),
            profile.map(|profile| profile.context_budget().hard_effective_limit),
            profile
                .map(|profile| profile.max_tool_continuations())
                .unwrap_or(crate::config::DEFAULT_MAX_TOOL_CONTINUATIONS),
        )
    };
    let base_prompt_tokens = estimate_token_usage(&request_msgs, "")
        .await
        .map(|usage| usage.prompt_tokens)
        .unwrap_or(0);
    // Native schemas and continuation framing are selected inside
    // stream_request. Reserve a conservative allowance so adaptive output is
    // disabled rather than risking context overflow.
    let context_output_limit = hard_context_limit.map(|limit| {
        limit
            .saturating_sub(base_prompt_tokens)
            .saturating_sub(8_192)
    });
    let continuation_policy = runner::ContinuationPolicy {
        adaptive_tool_output_limit,
        context_output_limit,
        max_total_output_tokens: 32_768,
        max_continuations,
    };
    let mut transport_retry_attempts = 0usize;
    let collected = loop {
        let attempt_client = request_client.clone();
        let attempt_state = Arc::clone(&request_state);
        let attempt_cancel = request_cancel.clone();
        let attempt_buffer = Arc::clone(&request_buffer);
        let attempt_api_url = api_base_url.clone();
        let attempt_model = model_name.clone();
        let attempt_msgs = Arc::clone(&request_msgs);
        let attempt_session_id = request_session_id.clone();
        let attempt = runner::collect_response(continuation_policy.clone(), move |request| {
            let request_client = attempt_client.clone();
            let request_state = Arc::clone(&attempt_state);
            let request_cancel = attempt_cancel.clone();
            let request_buffer = Arc::clone(&attempt_buffer);
            let request_api_url = attempt_api_url.clone();
            let request_model = attempt_model.clone();
            let request_msgs = Arc::clone(&attempt_msgs);
            let request_session_id = attempt_session_id.clone();
            async move {
                request_buffer.lock().await.reset();
                let current_msgs =
                    messages_for_response_continuation(&request_msgs, &request.previous);
                let stream_result = stream_request(
                    &request_client,
                    Arc::clone(&request_state),
                    request_cancel,
                    &request_api_url,
                    &request_model,
                    current_msgs.into_owned(),
                    Arc::clone(&request_buffer),
                    false,
                    request_allow_tools,
                    request_thinking_mode,
                    request_schema_policy,
                    Some(request_session_id.as_str()),
                    request.output_token_limit,
                )
                .await;
                let finish_reason = match stream_result {
                    Ok(finish_reason) => finish_reason,
                    Err(error) => {
                        let (partial_content, partial_native_tool_calls) = {
                            let buffer = request_buffer.lock().await;
                            (
                                buffer.content.clone(),
                                buffer.native_tool_call_checkpoint.clone(),
                            )
                        };
                        return Err(runner::ResponseError::with_partial_native(
                            error.to_string(),
                            partial_content,
                            partial_native_tool_calls,
                        ));
                    }
                };
                let buffer = request_buffer.lock().await;
                Ok(runner::ResponseChunk {
                    content: buffer.content.clone(),
                    final_answer_boundary: buffer.final_answer_boundary,
                    provider_final_answer_state: buffer.provider_final_answer_state,
                    finish_reason,
                    has_native_tool_calls: !buffer.native_tool_calls.is_empty(),
                    output_token_limit: buffer.output_token_limit,
                    thought_time_ms: buffer.thought_time_ms,
                    thought_tokens: buffer.thought_tokens,
                    token_usage: {
                        let state = request_state.lock().await;
                        state.current_token_usage.clone()
                    },
                })
            }
        })
        .await;
        match attempt {
            Err(error)
                if should_retry_stream_transport(&error, transport_retry_attempts)
                    && !request_cancel.is_cancelled() =>
            {
                transport_retry_attempts += 1;
                crate::logger::operational_event(
                    "turn.stream_retry",
                    serde_json::json!({
                        "session_id": request_session_id,
                        "attempt": transport_retry_attempts,
                        "output_phase": stream_output_phase(&error).label(),
                        "reason": lifecycle::stream_failure_kind_from_message(&error.to_string())
                            .map(|kind| kind.to_string()),
                        "error": error.to_string(),
                    }),
                );
                let mut s = request_state.lock().await;
                s.clear_current_response();
                s.clear_live_tool_calls();
                s.current_token_usage = None;
                s.status = crate::app::AppStatus::Streaming;
                s.stream_tracker = Some(crate::app::StreamTracker::new());
                drop(s);
                continue;
            }
            other => break other,
        }
    };
    let collected = match collected {
        Ok(result) => result,
        Err(error) => {
            if !ctx.lifecycle.task_completed {
                ctx.lifecycle.turn_machine.recover_error();
            }
            dbg_log!("Stream request failed: {error}");
            let error_message = error.to_string();
            if error_message.contains(crate::network::CONTEXT_PREFLIGHT_STOP_PREFIX) {
                let notice = "[Context checkpoint: the continuation request was not sent because replayed response output exhausted the effective context budget. The completed transcript is preserved. Run /compact or continue with a narrower request.]";
                let mut s = state.lock().await;
                if s.active_session_id == request_session_id {
                    s.history.push(ChatMessage::new("system", notice));
                    crate::config::save_session_history(&request_session_id, &s.history);
                }
                ctx.response.final_content_persisted = true;
                ctx.lifecycle.stop_reason = Some(lifecycle::StopReason::BudgetExceeded(
                    "context continuation preflight exceeded".to_owned(),
                ));
                s.current_token_usage = None;
                return Err(RoundCollectionError::Stop);
            }
            let stream_failure_kind = lifecycle::stream_failure_kind_from_message(&error_message);
            ctx.response.last_stream_termination =
                stream_failure_kind.map(lifecycle::StreamTermination::from_failure);
            if ctx.lifecycle.task_completed {
                // Required verification already latched completion. A later
                // optional continuation must not turn an otherwise successful
                // task into recovery_failed; retain the evidence and expose
                // the transport phase as a warning in the terminal status.
                let kind =
                    stream_failure_kind.unwrap_or(lifecycle::StreamFailureKind::ProviderError);
                ctx.lifecycle.stop_reason =
                    Some(lifecycle::stop_reason_for_stream_failure(true, kind));
                crate::logger::operational_event(
                    "turn.completed_transport_warning",
                    serde_json::json!({
                        "session_id": request_session_id,
                        "kind": kind.to_string(),
                        "error": error_message,
                    }),
                );
            } else if error_message == "cancelled"
                || stream_failure_kind == Some(lifecycle::StreamFailureKind::Cancelled)
            {
                ctx.lifecycle.stop_reason = Some(lifecycle::StopReason::Cancelled);
            } else if let Some(kind) = stream_failure_kind
                && kind != lifecycle::StreamFailureKind::ProviderError
            {
                ctx.lifecycle.stop_reason =
                    Some(lifecycle::stop_reason_for_stream_failure(false, kind));
                ctx.metrics.provider_errors = ctx.metrics.provider_errors.saturating_add(1);
                crate::logger::operational_event(
                    "turn.stream_failure",
                    serde_json::json!({
                        "session_id": request_session_id,
                        "kind": kind.to_string(),
                        "error": error_message,
                    }),
                );
            } else {
                record_provider_error(ctx, &error.to_string());
            }
            if !cancel_token.is_cancelled() && !error.partial_native_tool_calls.is_empty() {
                ctx.response.final_content_persisted = true;
                crate::logger::operational_event(
                    "turn.native_stream_checkpoint",
                    serde_json::json!({
                        "session_id": request_session_id,
                        "call_count": error.partial_native_tool_calls.len(),
                        "outcome": "saved_unexecuted_checkpoint",
                    }),
                );
                checkpoint_native_stream_recovery(state, &error.partial_native_tool_calls, &error)
                    .await;
                return Err(RoundCollectionError::Stop);
            }
            if !cancel_token.is_cancelled()
                && ctx.recovery.stream_recovery_attempts < MAX_STREAM_RECOVERY_ATTEMPTS
                && recoverable_textual_stream_failure(&error.partial_content)
            {
                ctx.recovery.stream_recovery_attempts =
                    ctx.recovery.stream_recovery_attempts.saturating_add(1);
                ctx.response.final_content =
                    bounded_stream_recovery_checkpoint(&error.partial_content);
                ctx.response.final_content_persisted = true;
                crate::logger::operational_event(
                    "turn.stream_recovery",
                    serde_json::json!({
                        "session_id": request_session_id,
                        "attempt": ctx.recovery.stream_recovery_attempts,
                        "kind": stream_failure_kind.map(|kind| kind.to_string()),
                        "partial_content_bytes": error.partial_content.len(),
                        "outcome": "continuation_requested",
                    }),
                );
                checkpoint_stream_recovery(state, error.partial_content.clone(), &error).await;
                return Err(RoundCollectionError::Stop);
            }
            if !cancel_token.is_cancelled() && !error.partial_content.is_empty() {
                ctx.response.final_content_persisted = true;
                crate::logger::operational_event(
                    "turn.stream_checkpoint",
                    serde_json::json!({
                        "session_id": request_session_id,
                        "kind": stream_failure_kind.map(|kind| kind.to_string()),
                        "output_phase": stream_output_phase(&error).label(),
                        "partial_content_bytes": error.partial_content.len(),
                        "outcome": "saved_text_checkpoint",
                    }),
                );
                checkpoint_text_stream_recovery(state, error.partial_content.clone(), &error).await;
                return Err(RoundCollectionError::Stop);
            }
            let mut s = state.lock().await;
            let notice = if error_message == "cancelled"
                || stream_failure_kind == Some(lifecycle::StreamFailureKind::Cancelled)
            {
                "Request cancelled by user".to_string()
            } else if retryable_stream_failure(&error_message) {
                stream_interruption_notice(
                    &error,
                    stream_output_phase(&error),
                    transport_retry_attempts >= usize::from(MAX_STREAM_RECOVERY_ATTEMPTS),
                )
            } else {
                format!("Error from LLM Provider: {error_message}")
            };
            s.history.push(ChatMessage::new("system", notice));
            s.current_token_usage = None;
            return Err(RoundCollectionError::Stop);
        }
    };
    let content = collected.content;
    let thought_time_ms = content
        .contains("<think>")
        .then_some(collected.thought_time_ms);
    let thought_tokens = content
        .contains("<think>")
        .then_some(collected.thought_tokens);
    let stream_termination = stream_buffer.lock().await.termination;
    crate::logger::operational_event(
        "model.response",
        serde_json::json!({
            "session_id": request_session_id,
            "round": ctx.budget.tool_rounds,
            "finish_reason": collected.finish_reason,
            "stream_termination":
                stream_termination.map(|termination| termination.to_string()),
            "content_bytes": content.len(),
        }),
    );
    let latest_token_usage = {
        let s = state.lock().await;
        if s.current_token_usage.is_some() {
            s.current_token_usage.clone()
        } else {
            drop(s);
            let estimate = estimate_token_usage(&token_estimate_messages, &content).await;
            state.lock().await.current_token_usage = estimate.clone();
            estimate
        }
    };
    let token_usage = collected.token_usage.or_else(|| latest_token_usage.clone());
    ctx.response.last_token_usage = latest_token_usage;
    ctx.record_token_usage(token_usage.as_ref());
    {
        let mut s = state.lock().await;
        s.replace_current_response(content.clone());
        let reported = s
            .current_token_usage
            .as_ref()
            .map(|u| u.total_tokens as u64);
        ctx.budget.tokens_used = accumulate_tokens_used(ctx.budget.tokens_used, reported, &content);
    }
    if cancel_token.is_cancelled() {
        ctx.lifecycle.stop_reason = Some(lifecycle::StopReason::Cancelled);
        ctx.lifecycle.turn_machine.cancel();
        return Err(RoundCollectionError::Stop);
    }
    let buffer = stream_buffer.lock().await;
    let native_tool_calls = buffer.native_tool_calls.clone();
    ctx.response.streamed_call_ids = if native_tool_calls.is_empty() {
        buffer.tool_call_ids.clone()
    } else {
        native_tool_calls
            .iter()
            .map(|call| call.call_id.clone())
            .collect()
    };
    Ok(RoundResponse {
        content,
        final_answer_boundary: collected.final_answer_boundary,
        provider_final_answer_state: collected.provider_final_answer_state,
        finish_reason: collected.finish_reason,
        stream_termination,
        response_time_ms: turn_start_time.elapsed().as_millis() as u64,
        token_usage,
        thought_time_ms,
        thought_tokens,
        native_tool_calls,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        StreamOutputPhase, TurnContext, native_stream_checkpoint_content,
        prepare_request_steerability, recoverable_textual_stream_failure, retryable_stream_failure,
        should_retry_stream_transport, stream_interruption_notice, stream_output_phase,
    };

    #[test]
    fn recovery_request_entry_clears_the_marker_but_regular_rounds_retain_it() {
        let mut state = crate::app::AppState::new();
        state.status = crate::app::AppStatus::Streaming;
        let turn_session_id = state.active_session_id.clone();
        state.active_turn_steerable_session = Some(turn_session_id.clone());
        let mut ctx = TurnContext::new();

        prepare_request_steerability(&mut state, &ctx, &turn_session_id);
        assert!(state.can_accept_steer());

        ctx.recovery.loop_recovery_attempts = 1;
        prepare_request_steerability(&mut state, &ctx, &turn_session_id);
        assert_eq!(state.active_turn_steerable_session, None);
        assert!(!state.can_accept_steer());
    }

    #[test]
    fn stale_recovery_request_cannot_clear_a_replacement_session_marker() {
        let mut state = crate::app::AppState::new();
        state.status = crate::app::AppStatus::Streaming;
        let replacement_session_id = state.active_session_id.clone();
        state.active_turn_steerable_session = Some(replacement_session_id.clone());
        let mut ctx = TurnContext::new();
        ctx.recovery.loop_recovery_attempts = 1;

        prepare_request_steerability(&mut state, &ctx, "old-turn-session");

        assert_eq!(
            state.active_turn_steerable_session.as_deref(),
            Some(replacement_session_id.as_str())
        );
    }

    #[test]
    fn textual_tool_call_stream_failures_are_checkpointed_before_dispatch() {
        assert!(recoverable_textual_stream_failure(
            "[TOOL_CALLS]write_to_file[ARGS]{\"path\":\"x\",\"content\":\"partial"
        ));
        assert!(recoverable_textual_stream_failure(
            "[TOOL_CALLS]write_to_file[ARGS]{\"path\":\"x\",\"content\":\"complete\"}"
        ));
        assert!(!recoverable_textual_stream_failure(
            "ordinary partial prose"
        ));
        assert!(recoverable_textual_stream_failure(
            "[TOOL_CALLS]get_time[ARGS]{}"
        ));
    }

    #[test]
    fn only_transport_phase_failures_are_safe_to_replay() {
        assert!(retryable_stream_failure(
            "stream_failure:first_event_timeout status=none bytes_received=0 events_received=0 partial_event_bytes=0"
        ));
        assert!(!retryable_stream_failure(
            "stream_failure:header_timeout status=none bytes_received=0 events_received=0 partial_event_bytes=0"
        ));
        assert!(!retryable_stream_failure(
            "stream_failure:connect_timeout status=none bytes_received=0 events_received=0 partial_event_bytes=0"
        ));
        assert!(retryable_stream_failure(
            "stream_failure:premature_eof status=none bytes_received=32 events_received=1 partial_event_bytes=0"
        ));
        assert!(retryable_stream_failure(
            "stream_failure:stream_idle_timeout status=none bytes_received=32 events_received=1 partial_event_bytes=4"
        ));
        assert!(retryable_stream_failure(
            "stream_failure:response_body_decode status=none bytes_received=65211 events_received=335 partial_event_bytes=0 detail=SSE stream read failed: error decoding response body"
        ));
        assert!(!retryable_stream_failure(
            "stream_failure:malformed_sse status=none bytes_received=32 events_received=1 partial_event_bytes=0"
        ));
        assert!(!retryable_stream_failure(
            "stream_failure:cancelled status=none"
        ));
    }

    #[test]
    fn zero_byte_transport_failure_gets_one_retry_but_emitted_output_does_not() {
        let empty = crate::network::runner::ResponseError::with_partial(
            "stream_failure:response_body_decode status=200 bytes_received=0 events_received=0 partial_event_bytes=0",
            String::new(),
        );
        assert_eq!(stream_output_phase(&empty), StreamOutputPhase::BeforeOutput);
        assert!(should_retry_stream_transport(&empty, 0));
        assert!(!should_retry_stream_transport(&empty, 1));

        let partial = crate::network::runner::ResponseError::with_partial(
            "stream_failure:response_body_decode status=200 bytes_received=32 events_received=1 partial_event_bytes=0",
            "partial answer".to_owned(),
        );
        assert_eq!(stream_output_phase(&partial), StreamOutputPhase::TextOutput);
        assert!(!should_retry_stream_transport(&partial, 0));
    }

    #[test]
    fn ambiguous_tool_output_is_never_eligible_for_transport_replay() {
        let partial = crate::network::runner::ResponseError::with_partial(
            "stream_failure:premature_eof status=200 bytes_received=64 events_received=2 partial_event_bytes=0",
            "[TOOL_CALLS]write_to_file[ARGS]{\"path\":\"x\",\"content\":\"partial".to_owned(),
        );
        assert_eq!(stream_output_phase(&partial), StreamOutputPhase::ToolCall);
        assert!(!should_retry_stream_transport(&partial, 0));
        let notice = stream_interruption_notice(&partial, stream_output_phase(&partial), false);
        assert!(notice.contains("before a complete response was available"));
        assert!(notice.contains("continue"));
        assert!(notice.contains("will not be replayed"));
    }

    #[test]
    fn native_checkpoint_is_actionable_but_contains_no_partial_arguments() {
        let error = crate::network::runner::ResponseError::with_partial_native(
            "stream_failure:provider_error status=200 events_received=2",
            String::new(),
            vec![crate::network::stream::NativeToolCallCheckpoint {
                index: Some(1),
                call_id: Some("call-write".to_owned()),
                tool_name: "write_to_file".to_owned(),
                argument_bytes: 32,
                arguments_complete: false,
                arguments_overflowed: false,
                argument_fingerprint: "fingerprint".to_owned(),
                diagnostic: "unexpected end of json".to_owned(),
            }],
        );

        let content = native_stream_checkpoint_content(&error.partial_native_tool_calls, &error);
        assert!(content.contains("call-write"));
        assert!(content.contains("write_to_file"));
        assert!(content.contains("unexpected end of json"));
        assert!(content.contains("do not dispatch or replay"));
        assert!(!content.contains("partial arguments"));
    }
}
