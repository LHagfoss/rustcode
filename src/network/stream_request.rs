use crate::app::{AppState, TokenUsage};
use futures::StreamExt;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Mutex;
use tokio_util::io::StreamReader;

use super::retry;
use super::stream::StreamBuffer;
use super::{ToolFenceCounter, align_alternating_messages, count_tokens, parse_sse_line};

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn silent_sse_stream_returns_after_idle_timeout() {
        let (_writer, reader) = tokio::io::duplex(64);
        let mut reader = BufReader::new(reader);
        let mut line = String::new();
        let read = read_sse_line(&mut reader, &mut line);
        tokio::pin!(read);

        tokio::task::yield_now().await;
        tokio::time::advance(retry::STREAM_IDLE_TIMEOUT + std::time::Duration::from_millis(1))
            .await;

        let error = read.await.expect_err("silent stream must not hang forever");
        assert!(error.contains("idle timeout"), "{error}");
    }

    #[tokio::test]
    async fn sse_data_before_idle_timeout_is_returned_normally() {
        let (mut writer, reader) = tokio::io::duplex(64);
        let mut reader = BufReader::new(reader);
        let writer_task = tokio::spawn(async move {
            tokio::io::AsyncWriteExt::write_all(&mut writer, b"data: ready\n")
                .await
                .unwrap();
        });
        let mut line = String::new();

        let bytes = read_sse_line(&mut reader, &mut line)
            .await
            .expect("data should arrive before the idle timeout");

        assert_eq!(bytes, "data: ready\n".len());
        assert_eq!(line, "data: ready\n");
        writer_task.await.unwrap();
    }
}

async fn read_sse_line<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
    line_buf: &mut String,
) -> Result<usize, String> {
    match tokio::time::timeout(retry::STREAM_IDLE_TIMEOUT, reader.read_line(line_buf)).await {
        Ok(Ok(bytes)) => Ok(bytes),
        Ok(Err(error)) => Err(format!("SSE stream read failed: {error}")),
        Err(_) => Err(format!(
            "SSE stream idle timeout after {}s without provider data",
            retry::STREAM_IDLE_TIMEOUT.as_secs()
        )),
    }
}

/// Metadata-only summary of an outbound chat-completion request: round shape
/// and size, not content. This is what gets written to debug.log by default
/// in place of the full serialized payload (see `request_debug_log_line`).
pub(crate) fn request_log_summary(
    model: &str,
    message_count: usize,
    tool_count: usize,
    payload_bytes: usize,
) -> String {
    format!(
        "stream_request: sending model={model} messages={message_count} tools={tool_count} payload_bytes={payload_bytes}"
    )
}

/// Choose what to write to the debug log for an outbound request: the cheap
/// structured `summary` by default, or the full serialized `payload`
/// (pretty-printed, exactly as it goes over the wire) only when
/// `verbose` (`config.debug_verbose_network_logging`) is explicitly set.
/// Kept pure/separate from the call site so both paths are unit-testable
/// without an app state, a request, or a file write.
pub(crate) fn request_debug_log_line(
    verbose: bool,
    summary: &str,
    payload: &serde_json::Value,
) -> String {
    if verbose {
        format!(
            "stream_request: Request payload: {}",
            serde_json::to_string_pretty(payload).unwrap_or_default()
        )
    } else {
        summary.to_string()
    }
}

/// Preserve malformed native arguments for validation and model feedback
/// instead of silently turning them into an empty object. This keeps the raw
/// provider failure visible while ensuring the call cannot execute.
pub(crate) fn parse_native_tool_arguments(raw: &str) -> serde_json::Value {
    match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(value) if value.is_object() => value,
        Ok(value) => serde_json::json!({ "_invalid_arguments": value }),
        Err(error) => serde_json::json!({
            "_invalid_arguments": raw,
            "_parse_error": error.to_string(),
        }),
    }
}

pub(crate) async fn estimate_token_usage(
    messages: &[serde_json::Value],
    reply: &str,
) -> Option<TokenUsage> {
    let mut prompt_text = String::new();
    for msg in messages {
        if let Some(content) = msg.get("content") {
            if let Some(s) = content.as_str() {
                prompt_text.push_str(s);
                prompt_text.push('\n');
            } else if content.is_array() {
                if let Some(arr) = content.as_array() {
                    for item in arr {
                        if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                            prompt_text.push_str(text);
                            prompt_text.push('\n');
                        }
                    }
                }
            } else {
                prompt_text.push_str(&content.to_string());
                prompt_text.push('\n');
            }
        }
    }
    let prompt = count_tokens(&prompt_text);
    let full = prompt_text + reply + "\n";
    let total = count_tokens(&full);
    Some(TokenUsage {
        prompt_tokens: prompt,
        completion_tokens: total.saturating_sub(prompt),
        total_tokens: total,
        cached_tokens: None,
    })
}

#[allow(clippy::too_many_arguments)]
pub async fn stream_request(
    client: &reqwest::Client,
    state: Arc<Mutex<AppState>>,
    cancel_token: tokio_util::sync::CancellationToken,
    url: &str,
    model: &str,
    messages: &[serde_json::Value],
    buffer: Arc<Mutex<StreamBuffer>>,
    quiet: bool,
) -> Result<Option<String>, String> {
    let aligned_messages = align_alternating_messages(messages.to_vec());
    let message_count = aligned_messages.len();

    let profile = {
        state
            .lock()
            .await
            .config
            .models
            .iter()
            .find(|p| p.model == model && p.endpoint_url() == url)
            .cloned()
    };
    let max_tokens = profile
        .as_ref()
        .and_then(|p| p.max_tokens)
        .unwrap_or(crate::config::DEFAULT_REQUEST_MAX_TOKENS);

    let mut payload = serde_json::json!({
        "model": model,
        "messages": aligned_messages,
        "stream": true,
        "stream_options": {
            "include_usage": true
        },
        "max_tokens": max_tokens,
    });

    if let Some(enable_thinking) = profile.as_ref().and_then(|p| p.enable_thinking) {
        payload["enable_thinking"] = serde_json::json!(enable_thinking);
    }

    if !url.contains("generativelanguage.googleapis.com") {
        payload["frequency_penalty"] = serde_json::json!(0.3);
    }

    let tool_protocol = { state.lock().await.active_tool_protocol() };
    if matches!(tool_protocol, crate::config::ToolProtocol::ApiNative) {
        let delegation_active = { state.lock().await.delegation_active };
        let schema = {
            let mut s = state.lock().await;
            let agent_mode = s.agent_mode;
            s.prompt_cache
                .native_schema(delegation_active, tool_protocol, agent_mode)
                .to_vec()
        };
        if !schema.is_empty() {
            payload["tools"] = serde_json::Value::Array(schema);
            payload["tool_choice"] = serde_json::json!("auto");
        }
    }

    let tool_count = payload
        .get("tools")
        .and_then(|t| t.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    let payload_bytes = serde_json::to_vec(&payload).map(|v| v.len()).unwrap_or(0);
    let verbose_network_logging = { state.lock().await.config.debug_verbose_network_logging };
    dbg_log!(
        "{}",
        request_debug_log_line(
            verbose_network_logging,
            &request_log_summary(model, message_count, tool_count, payload_bytes),
            &payload,
        )
    );

    let resolved_url = {
        let trimmed = url.trim_end_matches('/');
        if trimmed.ends_with("/chat/completions") || trimmed.ends_with("/chats/completion") {
            trimmed.to_string()
        } else {
            format!("{trimmed}/chat/completions")
        }
    };

    let api_key = {
        let s = state.lock().await;
        s.config
            .models
            .iter()
            .find(|m| m.url == url || m.name == s.model_name || m.endpoint_url() == resolved_url)
            .and_then(|m| m.resolved_api_key())
    };

    let mut attempt = 0usize;
    let response = loop {
        if cancel_token.is_cancelled() {
            return Err("cancelled".to_string());
        }
        let mut req = client.post(&resolved_url).json(&payload);
        if let Some(ref key) = api_key {
            req = req
                .header("Authorization", format!("Bearer {key}"))
                .header("X-Api-Key", key);
        }
        let send_result = retry::race_cancellable(
            tokio::time::timeout(retry::HEADER_TIMEOUT, req.send()),
            &cancel_token,
        )
        .await;
        let send_result = match send_result {
            None => return Err("cancelled".to_string()),
            Some(Err(_elapsed)) => {
                if attempt < retry::MAX_RETRIES {
                    let delay = retry::delay_for_attempt(attempt, 0);
                    dbg_log!(
                        "stream_request: timed out waiting for response headers (attempt {}/{}), backing off {}ms",
                        attempt + 1,
                        retry::MAX_RETRIES,
                        delay.as_millis()
                    );
                    if retry::race_cancellable(tokio::time::sleep(delay), &cancel_token)
                        .await
                        .is_none()
                    {
                        return Err("cancelled".to_string());
                    }
                    attempt += 1;
                    continue;
                }
                return Err(format!(
                    "timed out waiting for response headers after {}s",
                    retry::HEADER_TIMEOUT.as_secs()
                ));
            }
            Some(Ok(r)) => r,
        };
        match send_result {
            Ok(resp) if resp.status().is_success() => {
                dbg_log!(
                    "stream_request: Received response status: {}",
                    resp.status()
                );
                break resp;
            }
            Ok(resp) => {
                let status = resp.status();
                let code = status.as_u16();
                let err_body = match retry::race_cancellable(resp.text(), &cancel_token).await {
                    None => return Err("cancelled".to_string()),
                    Some(body) => body.unwrap_or_default(),
                };
                if retry::is_retryable_status(code) && attempt < retry::MAX_RETRIES {
                    let delay = retry::delay_for_attempt(attempt, code);
                    dbg_log!(
                        "stream_request: retryable status {} (attempt {}/{}), backing off {}ms",
                        status,
                        attempt + 1,
                        retry::MAX_RETRIES,
                        delay.as_millis()
                    );
                    if retry::race_cancellable(tokio::time::sleep(delay), &cancel_token)
                        .await
                        .is_none()
                    {
                        return Err("cancelled".to_string());
                    }
                    attempt += 1;
                    continue;
                }
                dbg_log!(
                    "stream_request: Request failed with status {}. Body: {}",
                    status,
                    err_body
                );
                return Err(format!("{status} - {err_body}"));
            }
            Err(e) => {
                if retry::is_retryable_transport(&e) && attempt < retry::MAX_RETRIES {
                    let delay = retry::delay_for_attempt(attempt, 0);
                    dbg_log!(
                        "stream_request: transient network error (attempt {}/{}), backing off {}ms: {}",
                        attempt + 1,
                        retry::MAX_RETRIES,
                        delay.as_millis(),
                        e
                    );
                    if retry::race_cancellable(tokio::time::sleep(delay), &cancel_token)
                        .await
                        .is_none()
                    {
                        return Err("cancelled".to_string());
                    }
                    attempt += 1;
                    continue;
                }
                let mut msg = format!("Request failed: {e}");
                let mut src = std::error::Error::source(&e);
                while let Some(cause) = src {
                    msg.push_str(&format!(": {cause}"));
                    src = cause.source();
                }
                return Err(msg);
            }
        }
    };

    let stream = response
        .bytes_stream()
        .map(|r| r.map_err(std::io::Error::other));
    let wrapped = StreamReader::new(stream);
    let mut reader = BufReader::with_capacity(4096, wrapped);
    let mut line_buf = String::with_capacity(4096);
    let mut in_reasoning = false;
    let mut finish_reason: Option<String> = None;

    #[derive(Debug)]
    struct ToolAccumulator {
        id: String,
        name: String,
        arguments: String,
    }
    let mut accumulators: Vec<ToolAccumulator> = Vec::new();
    let mut fences = ToolFenceCounter::default();
    let runaway_limit = crate::tools::MAX_TOOL_CALLS_PER_RESPONSE;

    dbg_log!("stream_request: Starting SSE stream read loop");
    loop {
        if cancel_token.is_cancelled() {
            dbg_log!("stream_request: Stream reading cancelled via token");
            return Ok(None);
        }

        tokio::select! {
            r = read_sse_line(&mut reader, &mut line_buf) => {
                match r {
                    Ok(0) => {
                        dbg_log!("stream_request: SSE stream read EOF (0 bytes)");
                        break;
                    }
                    Ok(_) => {
                        let trimmed = line_buf.trim();
                        if let Some(json_str) = parse_sse_line(trimmed) {
                            if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_str) {
                                if let Some(choices) = val.get("choices").and_then(|c| c.as_array())
                                    && !choices.is_empty() {
                                        if let Some(fr) = choices[0].get("finish_reason").and_then(|f| f.as_str()) {
                                            finish_reason = Some(fr.to_string());
                                        }
                                         let delta = choices[0].get("delta");
                                         let reasoning = delta
                                             .and_then(|d| {
                                                 d.get("reasoning")
                                                     .or_else(|| d.get("reasoning_content"))
                                                     .or_else(|| d.get("thought"))
                                                     .or_else(|| d.get("thinking"))
                                             })
                                             .and_then(|r| r.as_str());
                                         let content = delta
                                             .and_then(|d| d.get("content").or_else(|| d.get("text")))
                                             .and_then(|c| c.as_str());

                                         if let Some(tool_calls) = delta.and_then(|d| d.get("tool_calls")).and_then(|t| t.as_array()) {
                                             const MAX_TOOL_CALL_INDEX: usize = 127;
                                             for tc in tool_calls {
                                                 let mut idx = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                                                 if idx > MAX_TOOL_CALL_INDEX {
                                                     eprintln!("Warning: tool call index {} exceeds max allowed ({}), clamping.", idx, MAX_TOOL_CALL_INDEX);
                                                     idx = idx.min(MAX_TOOL_CALL_INDEX);
                                                 }
                                                 while accumulators.len() <= idx {
                                                     accumulators.push(ToolAccumulator {
                                                         id: String::new(),
                                                         name: String::new(),
                                                         arguments: String::new(),
                                                     });
                                                 }
                                                 let acc = &mut accumulators[idx];
                                                 if let Some(id) = tc.get("id").and_then(|i| i.as_str()) {
                                                     acc.id.push_str(id);
                                                 }
                                                 if let Some(name) = tc.get("function").and_then(|f| f.get("name")).and_then(|n| n.as_str()) {
                                                     acc.name.push_str(name);
                                                 }
                                                 if let Some(args) = tc.get("function").and_then(|f| f.get("arguments")).and_then(|a| a.as_str()) {
                                                     acc.arguments.push_str(args);
                                                 }
                                             }
                                         }

                                         let mut chunk = String::new();
                                        if let Some(r_token) = reasoning {
                                            if !in_reasoning {
                                                in_reasoning = true;
                                                let started = std::time::Instant::now();
                                                buffer.lock().await.thought_started_at = Some(started);
                                                if !quiet {
                                                    state.lock().await.current_thought_started_at = Some(started);
                                                }
                                                chunk.push_str("<think>\n");
                                            }
                                            let thought_tokens = (r_token.len() as f64
                                                * crate::app::TOKENS_PER_CHAR_APPROX)
                                                as u32;
                                            {
                                                let mut buffer = buffer.lock().await;
                                                buffer.thought_tokens = buffer
                                                    .thought_tokens
                                                    .saturating_add(thought_tokens);
                                            }
                                            if !quiet {
                                                let mut s = state.lock().await;
                                                s.current_thought_tokens = s
                                                    .current_thought_tokens
                                                    .saturating_add(thought_tokens);
                                            }
                                            chunk.push_str(r_token);
                                        } else if let Some(c_token) = content {
                                            if in_reasoning {
                                                in_reasoning = false;
                                                buffer.lock().await.finish_thought();
                                                if !quiet {
                                                    let mut s = state.lock().await;
                                                    if let Some(started) = s.current_thought_started_at.take() {
                                                        s.current_thought_time_ms = s
                                                            .current_thought_time_ms
                                                            .saturating_add(started.elapsed().as_millis() as u64);
                                                    }
                                                }
                                                chunk.push_str("\n</think>\n\n");
                                            }
                                            chunk.push_str(c_token);
                                        }
                                        let runaway = fences.push(&chunk) > runaway_limit
                                            || accumulators
                                                .iter()
                                                .filter(|acc| !acc.name.is_empty())
                                                .count()
                                                > runaway_limit;
                                        if !chunk.is_empty() {
                                            let tokens = (chunk.len() as f64 * crate::app::TOKENS_PER_CHAR_APPROX) as u32;
                                            if let Some(ref mut tracker) = state.lock().await.stream_tracker {
                                                tracker.tokens_so_far += tokens;
                                                tracker.record_chunk();
                                            }

                                            buffer.lock().await.content.push_str(&chunk);
                                            if !quiet {
                                                let mut s = state.lock().await;
                                                s.current_response.push_str(&chunk);
                                                if s.raw_cli_mode {
                                                    use std::io::Write;
                                                    print!("{chunk}");
                                                    let _ = std::io::stdout().flush();
                                                }
                                            }
                                        }
                                        if runaway {
                                            dbg_log!(
                                                "stream_request: past {} tool calls in one response — cutting the stream",
                                                runaway_limit
                                            );
                                            crate::logger::operational_event(
                                                "stream.runaway_cut",
                                                serde_json::json!({ "limit": runaway_limit }),
                                            );
                                            accumulators.truncate(runaway_limit);
                                            line_buf.clear();
                                            break;
                                        }
                                    }
                                if let Some(usage) = val.get("usage").filter(|_| !quiet)
                                    && let (Some(p), Some(c), Some(t)) = (
                                        usage.get("prompt_tokens").and_then(|v| v.as_u64()),
                                        usage.get("completion_tokens").and_then(|v| v.as_u64()),
                                        usage.get("total_tokens").and_then(|v| v.as_u64()),
                                    ) {
                                        let cached = usage.get("prompt_tokens_details")
                                            .and_then(|details| details.get("cached_tokens"))
                                            .and_then(|v| v.as_u64())
                                            .or_else(|| usage.get("cached_tokens").and_then(|v| v.as_u64()))
                                            .map(|n| n as u32);

                                        state.lock().await.current_token_usage = Some(TokenUsage {
                                            prompt_tokens: p as u32,
                                            completion_tokens: c as u32,
                                            total_tokens: t as u32,
                                            cached_tokens: cached,
                                        });
                                    }
                            } else {
                                dbg_log!("stream_request: Failed to parse JSON from data payload: '{}'", json_str);
                            }
                        }
                        line_buf.clear();
                    }
                    Err(e) => {
                        dbg_log!("stream_request: SSE read error: {}", e);
                        if e.contains("idle timeout") {
                            return Err(e);
                        }
                        break;
                    }
                }
            }
            _ = cancel_token.cancelled() => {
                dbg_log!("stream_request: Cancelled via select branch");
                return Ok(None);
            }
        }
    }

    if in_reasoning {
        buffer.lock().await.finish_thought();
        if !quiet {
            let mut s = state.lock().await;
            if let Some(started) = s.current_thought_started_at.take() {
                s.current_thought_time_ms = s
                    .current_thought_time_ms
                    .saturating_add(started.elapsed().as_millis() as u64);
            }
        }
        buffer.lock().await.content.push_str("\n</think>\n\n");
        if !quiet {
            let mut s = state.lock().await;
            s.current_response.push_str("\n</think>\n\n");
            if s.raw_cli_mode {
                use std::io::Write;
                print!("\n</think>\n\n");
                let _ = std::io::stdout().flush();
            }
        }
    }

    let mut streamed_call_ids: Vec<String> = Vec::new();
    let mut native_tool_calls: Vec<crate::tools::ToolCallEnvelope> = Vec::new();
    for (position, acc) in accumulators.iter().enumerate() {
        if acc.name.is_empty() {
            continue;
        }

        let args_json = parse_native_tool_arguments(&acc.arguments);

        let call_id = if acc.id.is_empty() {
            format!("call_{position}")
        } else {
            acc.id.clone()
        };
        streamed_call_ids.push(call_id.clone());
        native_tool_calls.push(crate::tools::ToolCallEnvelope {
            call_id,
            tool_name: acc.name.clone(),
            arguments: args_json,
        });
    }

    if !native_tool_calls.is_empty() {
        dbg_log!(
            "stream_request: preserving {} native tool call envelope(s)",
            native_tool_calls.len()
        );
        {
            let mut buf = buffer.lock().await;
            buf.tool_call_ids = streamed_call_ids;
            buf.native_tool_calls = native_tool_calls;
        }
    }

    let mut buf = buffer.lock().await;
    buf.content = buf
        .content
        .trim_end_matches(char::is_whitespace)
        .to_string();
    dbg_log!(
        "stream_request: Stream request loop ended. Total content: {} chars",
        buf.content.len()
    );
    Ok(finish_reason)
}
