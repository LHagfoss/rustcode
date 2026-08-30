use std::borrow::Cow;
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::app::{AppState, ChatMessage, TokenUsage};

use super::super::lifecycle;
use super::super::runner;
use super::super::stream::StreamBuffer;
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
    pub finish_reason: Option<String>,
    pub response_time_ms: u64,
    pub token_usage: Option<TokenUsage>,
    pub thought_time_ms: Option<u64>,
    pub thought_tokens: Option<u32>,
    pub native_tool_calls: Vec<crate::tools::ToolCallEnvelope>,
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
            crate::tools::ToolSchemaPolicy::root(s.delegation_active),
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
    let collected = runner::collect_response(move |previous| {
        let request_client = request_client.clone();
        let request_state = Arc::clone(&request_state);
        let request_cancel = request_cancel.clone();
        let request_buffer = Arc::clone(&request_buffer);
        let request_api_url = api_base_url.clone();
        let request_model = model_name.clone();
        let request_msgs = Arc::clone(&request_msgs);
        let request_session_id = request_session_id.clone();
        async move {
            request_buffer.lock().await.reset();
            let current_msgs = messages_for_response_continuation(&request_msgs, &previous);
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
    .await;
    let collected = match collected {
        Ok(result) => result,
        Err(error) => {
            ctx.lifecycle.turn_machine.recover_error();
            dbg_log!("Stream request failed: {error}");
            if error == "cancelled" {
                ctx.lifecycle.stop_reason = Some(lifecycle::StopReason::Cancelled);
            } else {
                record_provider_error(ctx, &error);
            }
            let mut s = state.lock().await;
            let notice = if error == "cancelled" {
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
        finish_reason: collected.finish_reason,
        response_time_ms: turn_start_time.elapsed().as_millis() as u64,
        token_usage,
        thought_time_ms,
        thought_tokens,
        native_tool_calls,
    })
}
