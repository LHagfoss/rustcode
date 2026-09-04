use std::borrow::Cow;
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::app::{AppState, ChatMessage, TokenUsage};

use super::super::lifecycle;
use super::super::runner;
use super::super::stream::{FinalAnswerBoundary, ProviderFinalAnswerState, StreamBuffer};
use super::super::stream_request::{estimate_token_usage, stream_request};
use super::super::{
    accumulate_tokens_used, prepare_turn_request, probe_function_calling, record_provider_error,
};
use super::TurnContext;

use crate::network::text::{
    continuation_nudge_for_category, format_continuation_assistant_message,
};

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
        )
    )
}

pub(super) async fn collect_round(
    client: &reqwest::Client,
    state: &Arc<Mutex<AppState>>,
    cancel_token: &tokio_util::sync::CancellationToken,
    stream_buffer: &Arc<Mutex<StreamBuffer>>,
    ctx: &mut TurnContext,
) -> Result<RoundResponse, ()> {
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

    let msgs = match prepare_turn_request(client, state, ctx.budget.tool_rounds, cancel_token).await
    {
        Ok(msgs) => msgs,
        Err(error) => {
            dbg_log!("Image fallback failed: {error}");
            let mut s = state.lock().await;
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
            return Err(());
        }
    };

    {
        let mut s = state.lock().await;
        s.clear_current_response();
        s.current_thought_time_ms = 0;
        s.current_thought_tokens = 0;
        s.current_thought_started_at = None;
    }
    stream_buffer.lock().await.reset();
    let (api_base_url, model_name, request_schema_policy, request_session_id) = {
        let s = state.lock().await;
        (
            s.api_base_url.clone(),
            s.model_name.clone(),
            crate::tools::ToolSchemaPolicy::root_for_mode(s.delegation_active, s.agent_mode),
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
    let (adaptive_tool_output_limit, hard_context_limit) = {
        let s = request_state.lock().await;
        let profile = s.config.models.iter().find(|profile| {
            (profile.model == model_name || profile.name == model_name)
                && (profile.url == api_base_url || profile.endpoint_url() == api_base_url)
        });
        (
            profile
                .filter(|profile| {
                    request_allow_tools
                        && request_thinking_mode
                            == super::super::stream_request::ThinkingMode::Normal
                        && profile.tool_max_tokens.is_some()
                })
                .map(|profile| profile.tool_output_ceiling()),
            profile.map(|profile| profile.context_budget().hard_effective_limit),
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
                let finish_reason = stream_request(
                    &request_client,
                    request_state,
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
                .await
                .map_err(|e| e.to_string())?;
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
                })
            }
        })
        .await;
        match attempt {
            Err(error)
                if transport_retry_attempts < 1
                    && retryable_stream_failure(&error)
                    && !request_cancel.is_cancelled() =>
            {
                transport_retry_attempts += 1;
                crate::logger::operational_event(
                    "turn.stream_retry",
                    serde_json::json!({
                        "attempt": transport_retry_attempts,
                        "reason": lifecycle::stream_failure_kind_from_message(&error)
                            .map(|kind| kind.to_string()),
                        "error": error,
                    }),
                );
                let mut s = request_state.lock().await;
                s.clear_current_response();
                s.clear_live_tool_calls();
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
            let stream_failure_kind = lifecycle::stream_failure_kind_from_message(&error);
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
                        "kind": kind.to_string(),
                        "error": error,
                    }),
                );
            } else if error == "cancelled"
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
                        "kind": kind.to_string(),
                        "error": error,
                    }),
                );
            } else {
                record_provider_error(ctx, &error);
            }
            let mut s = state.lock().await;
            let notice = if error == "cancelled"
                || stream_failure_kind == Some(lifecycle::StreamFailureKind::Cancelled)
            {
                "Request cancelled by user".to_string()
            } else {
                format!("Error from LLM Provider: {error}")
            };
            s.history.push(ChatMessage::new("system", notice));
            s.current_token_usage = None;
            return Err(());
        }
    };
    let content = collected.content;
    let thought_time_ms = content
        .contains("<think>")
        .then_some(collected.thought_time_ms);
    let thought_tokens = content
        .contains("<think>")
        .then_some(collected.thought_tokens);
    crate::logger::operational_event(
        "model.response",
        serde_json::json!({
            "round": ctx.budget.tool_rounds,
            "finish_reason": collected.finish_reason,
            "content_bytes": content.len(),
        }),
    );
    let token_usage = {
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
    ctx.response.last_token_usage = token_usage.clone();
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
        return Err(());
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
        response_time_ms: turn_start_time.elapsed().as_millis() as u64,
        token_usage,
        thought_time_ms,
        thought_tokens,
        native_tool_calls,
    })
}

#[cfg(test)]
mod tests {
    use super::retryable_stream_failure;

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
        assert!(!retryable_stream_failure(
            "stream_failure:malformed_sse status=none bytes_received=32 events_received=1 partial_event_bytes=0"
        ));
        assert!(!retryable_stream_failure(
            "stream_failure:cancelled status=none"
        ));
    }
}
