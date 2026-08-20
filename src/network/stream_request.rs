use crate::app::{AppState, TokenUsage};
use futures::StreamExt;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Mutex;
use tokio_util::io::StreamReader;

use super::retry;
use super::stream::StreamBuffer;
use super::{ToolFenceCounter, align_alternating_messages, count_tokens, parse_sse_line};

fn apply_profile_generation_options(
    payload: &mut serde_json::Value,
    profile: Option<&crate::config::ModelProfile>,
    disable_thinking: bool,
) {
    if disable_thinking || profile.is_some_and(|p| p.enable_thinking == Some(false)) {
        payload["enable_thinking"] = serde_json::json!(false);
        return;
    }
    if let Some(enable_thinking) = profile.and_then(|p| p.enable_thinking) {
        payload["enable_thinking"] = serde_json::json!(enable_thinking);
    }
    if let Some(effort) = profile.and_then(|p| p.reasoning_effort.as_ref()) {
        payload["reasoning_effort"] = serde_json::json!(effort);
    }
    if let Some(thinking_budget) = profile.and_then(|p| p.thinking_budget) {
        payload["thinking_budget"] = serde_json::json!(thinking_budget);
    }
}

fn apply_api_native_tools(
    payload: &mut serde_json::Value,
    schema: Vec<serde_json::Value>,
    allow_tools: bool,
) {
    if allow_tools && !schema.is_empty() {
        payload["tools"] = serde_json::Value::Array(schema);
        payload["tool_choice"] = serde_json::json!("auto");
    }
}

#[derive(Debug, PartialEq, Eq)]
struct BoundedReasoningChunk {
    text: String,
    estimated_tokens: u32,
    budget_exhausted: bool,
}

fn estimated_reasoning_tokens(text: &str) -> u32 {
    if text.is_empty() {
        0
    } else {
        ((text.len() as f64 * crate::app::TOKENS_PER_CHAR_APPROX).ceil() as u32).max(1)
    }
}

fn bound_reasoning_chunk(
    text: &str,
    used_tokens: u32,
    budget: Option<u32>,
) -> BoundedReasoningChunk {
    let Some(budget) = budget else {
        return BoundedReasoningChunk {
            text: text.to_string(),
            estimated_tokens: estimated_reasoning_tokens(text),
            budget_exhausted: false,
        };
    };
    let remaining = budget.saturating_sub(used_tokens);
    let full_estimate = estimated_reasoning_tokens(text);
    if full_estimate <= remaining {
        return BoundedReasoningChunk {
            text: text.to_string(),
            estimated_tokens: full_estimate,
            budget_exhausted: false,
        };
    }

    let max_bytes = ((remaining as f64) / crate::app::TOKENS_PER_CHAR_APPROX) as usize;
    let mut end = max_bytes.min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let bounded = &text[..end];
    BoundedReasoningChunk {
        text: bounded.to_string(),
        estimated_tokens: estimated_reasoning_tokens(bounded).min(remaining),
        budget_exhausted: true,
    }
}

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

    #[tokio::test(start_paused = true)]
    async fn partial_sse_bytes_reset_idle_timeout_until_line_completes() {
        let (mut writer, reader) = tokio::io::duplex(64);
        let read_task = tokio::spawn(async move {
            let mut reader = BufReader::new(reader);
            let mut line = String::new();
            let bytes = read_sse_line(&mut reader, &mut line).await?;
            Ok::<_, String>((bytes, line))
        });

        tokio::io::AsyncWriteExt::write_all(&mut writer, b"data:")
            .await
            .unwrap();
        tokio::task::yield_now().await;
        tokio::time::advance(retry::STREAM_IDLE_TIMEOUT - std::time::Duration::from_secs(1)).await;
        tokio::io::AsyncWriteExt::write_all(&mut writer, b" still")
            .await
            .unwrap();
        tokio::task::yield_now().await;
        tokio::time::advance(retry::STREAM_IDLE_TIMEOUT - std::time::Duration::from_secs(1)).await;
        tokio::io::AsyncWriteExt::write_all(&mut writer, b" alive\n")
            .await
            .unwrap();

        let (bytes, line) = read_task.await.unwrap().unwrap();
        assert_eq!(bytes, "data: still alive\n".len());
        assert_eq!(line, "data: still alive\n");
    }

    #[tokio::test(start_paused = true)]
    async fn partial_sse_bytes_then_stall_returns_idle_timeout() {
        let (mut writer, reader) = tokio::io::duplex(64);
        let read_task = tokio::spawn(async move {
            let mut reader = BufReader::new(reader);
            read_sse_line(&mut reader, &mut String::new()).await
        });

        tokio::io::AsyncWriteExt::write_all(&mut writer, b"data: partial")
            .await
            .unwrap();
        tokio::task::yield_now().await;
        tokio::time::advance(retry::STREAM_IDLE_TIMEOUT + std::time::Duration::from_millis(1))
            .await;

        let error = read_task
            .await
            .unwrap()
            .expect_err("partial line must time out after a stall");
        assert!(error.contains("idle timeout"), "{error}");
    }

    #[tokio::test]
    async fn sse_eof_returns_final_unterminated_line() {
        let (mut writer, reader) = tokio::io::duplex(64);
        tokio::io::AsyncWriteExt::write_all(&mut writer, b"data: final")
            .await
            .unwrap();
        drop(writer);
        let mut reader = BufReader::new(reader);
        let mut line = String::new();

        let bytes = read_sse_line(&mut reader, &mut line).await.unwrap();

        assert_eq!(bytes, "data: final".len());
        assert_eq!(line, "data: final");
    }

    #[test]
    fn parse_speculative_arguments_extracts_complete_and_partial_json() {
        // Complete JSON
        let complete = r#"{"TargetFile": "src/symbols.rs", "Instruction": "fix bug"}"#;
        let parsed = parse_speculative_arguments(complete);
        assert_eq!(parsed["TargetFile"], "src/symbols.rs");
        assert_eq!(parsed["Instruction"], "fix bug");

        // Partial JSON cut mid-string
        let partial_string = r#"{"TargetFile": "src/main.rs", "Instruction": "refactor"#;
        let parsed_partial = parse_speculative_arguments(partial_string);
        assert_eq!(parsed_partial["TargetFile"], "src/main.rs");

        // Early partial JSON
        let early_partial = r#"{"CommandLine": "cargo check --tests""#;
        let parsed_early = parse_speculative_arguments(early_partial);
        assert_eq!(parsed_early["CommandLine"], "cargo check --tests");

        // Grep pattern
        let grep_partial = r#"{"pattern": "Config", "path": "src/""#;
        let parsed_grep = parse_speculative_arguments(grep_partial);
        assert_eq!(parsed_grep["pattern"], "Config");
        assert_eq!(parsed_grep["path"], "src/");
    }

    #[test]
    fn parse_speculative_text_tool_call_extracts_in_flight_fences() {
        let in_flight = "Let me search the codebase:\n```tool\n{\"name\": \"grep\", \"arguments\": {\"pattern\": \"AppConfig\"";
        let (name, args) = parse_speculative_text_tool_call(in_flight).expect("should extract in-flight call");
        assert_eq!(name, "grep");
        assert_eq!(args["pattern"], "AppConfig");

        let completed = "Done:\n```tool\n{\"name\": \"grep\", \"arguments\": {}}\n```\nHere are the results:";
        assert!(parse_speculative_text_tool_call(completed).is_none(), "completed fence is not in-flight");
    }

    #[test]
    fn profile_generation_options_include_hard_thinking_budget() {
        let profile = crate::config::ModelProfile {
            enable_thinking: Some(true),
            reasoning_effort: Some("low".to_string()),
            thinking_budget: Some(4096),
            ..crate::config::ModelProfile::default()
        };
        let mut payload = serde_json::json!({});

        apply_profile_generation_options(&mut payload, Some(&profile), false);

        assert_eq!(payload["enable_thinking"], true);
        assert_eq!(payload["reasoning_effort"], "low");
        assert_eq!(payload["thinking_budget"], 4096);
    }

    #[test]
    fn absent_profile_generation_options_do_not_override_server_defaults() {
        let mut payload = serde_json::json!({});

        apply_profile_generation_options(
            &mut payload,
            Some(&crate::config::ModelProfile::default()),
            false,
        );

        assert!(payload.get("enable_thinking").is_none());
        assert!(payload.get("reasoning_effort").is_none());
        assert!(payload.get("thinking_budget").is_none());
    }

    #[tokio::test]
    async fn native_schema_tokens_are_in_prompt_and_total_usage_estimates() {
        let messages = vec![serde_json::json!({
            "role": "system",
            "content": "You are RustCode."
        })];
        let schema = vec![serde_json::json!({
            "type": "function",
            "function": {
                "name": "inspect_workspace",
                "description": "Inspect files in the workspace",
                "parameters": {"type": "object"}
            }
        })];

        let without_schema = estimate_token_usage(&messages, "reply").await.unwrap();
        let with_schema = estimate_token_usage_with_tool_schemas(&messages, "reply", &schema)
            .await
            .unwrap();
        let schema_tokens =
            crate::network::compaction::estimate_tool_schema_tokens(&schema) as u32;

        assert!(schema_tokens > 0);
        assert_eq!(with_schema.prompt_tokens, without_schema.prompt_tokens + schema_tokens);
        assert_eq!(with_schema.total_tokens, without_schema.total_tokens + schema_tokens);
        assert_eq!(with_schema.completion_tokens, without_schema.completion_tokens);
    }

    #[test]
    fn disabled_thinking_omits_reasoning_controls() {
        let profile = crate::config::ModelProfile {
            enable_thinking: Some(false),
            reasoning_effort: Some("medium".to_string()),
            thinking_budget: Some(4096),
            ..crate::config::ModelProfile::default()
        };
        let mut payload = serde_json::json!({});

        apply_profile_generation_options(&mut payload, Some(&profile), false);

        assert_eq!(payload["enable_thinking"], false);
        assert!(payload.get("reasoning_effort").is_none());
        assert!(payload.get("thinking_budget").is_none());
    }

    #[test]
    fn request_override_disables_thinking_and_omits_reasoning_controls() {
        let profile = crate::config::ModelProfile {
            enable_thinking: Some(true),
            reasoning_effort: Some("high".to_string()),
            thinking_budget: Some(8192),
            ..crate::config::ModelProfile::default()
        };
        let mut payload = serde_json::json!({});

        apply_profile_generation_options(&mut payload, Some(&profile), true);

        assert_eq!(payload["enable_thinking"], false);
        assert!(payload.get("reasoning_effort").is_none());
        assert!(payload.get("thinking_budget").is_none());
    }

    #[test]
    fn disabled_tools_omit_schema_and_tool_choice() {
        let mut payload = serde_json::json!({});
        let schema = vec![serde_json::json!({
            "type": "function",
            "function": {"name": "view_file"}
        })];

        apply_api_native_tools(&mut payload, schema, false);

        assert!(payload.get("tools").is_none());
        assert!(payload.get("tool_choice").is_none());
    }

    #[test]
    fn enabled_tools_include_schema_and_auto_choice() {
        let mut payload = serde_json::json!({});
        let schema = vec![serde_json::json!({
            "type": "function",
            "function": {"name": "view_file"}
        })];

        apply_api_native_tools(&mut payload, schema, true);

        assert_eq!(payload["tools"].as_array().map(Vec::len), Some(1));
        assert_eq!(payload["tool_choice"], "auto");
    }

    #[test]
    fn reasoning_chunks_stop_at_the_configured_client_budget() {
        let first = bound_reasoning_chunk("a".repeat(12).as_str(), 0, Some(4));
        assert_eq!(first.estimated_tokens, 3);
        assert!(!first.budget_exhausted);

        let final_chunk = bound_reasoning_chunk("b".repeat(12).as_str(), 3, Some(4));
        assert_eq!(final_chunk.estimated_tokens, 1);
        assert_eq!(final_chunk.text.len(), 4);
        assert!(final_chunk.budget_exhausted);
    }

    #[test]
    fn absent_reasoning_budget_preserves_the_whole_chunk() {
        let chunk = bound_reasoning_chunk("reasoning", 0, None);
        assert_eq!(chunk.text, "reasoning");
        assert!(!chunk.budget_exhausted);
    }
}

async fn read_sse_line<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
    line_buf: &mut String,
) -> Result<usize, String> {
    let mut bytes = Vec::new();

    loop {
        let (chunk_len, line_complete) = {
            let chunk =
                match tokio::time::timeout(retry::STREAM_IDLE_TIMEOUT, reader.fill_buf()).await {
                    Ok(Ok(chunk)) => chunk,
                    Ok(Err(error)) => return Err(format!("SSE stream read failed: {error}")),
                    Err(_) => {
                        return Err(format!(
                            "SSE stream idle timeout after {}s without provider data",
                            retry::STREAM_IDLE_TIMEOUT.as_secs()
                        ));
                    }
                };

            if chunk.is_empty() {
                if bytes.is_empty() {
                    return Ok(0);
                }
                break;
            }

            let chunk_len = chunk
                .iter()
                .position(|&byte| byte == b'\n')
                .map_or(chunk.len(), |newline| newline + 1);
            bytes.extend_from_slice(&chunk[..chunk_len]);
            (chunk_len, chunk[chunk_len - 1] == b'\n')
        };
        reader.consume(chunk_len);

        if line_complete {
            break;
        }
    }

    let line = std::str::from_utf8(&bytes)
        .map_err(|error| format!("SSE stream contained invalid UTF-8: {error}"))?;
    line_buf.push_str(line);
    Ok(bytes.len())
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

/// Speculatively parse partial JSON argument fragments emitted chunk-by-chunk
/// by the model over SSE, allowing the TUI to project tool names and targets in real time.
pub(crate) fn parse_speculative_arguments(raw: &str) -> serde_json::Value {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return serde_json::json!({});
    }

    // 1. Exact parse if complete
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
        if value.is_object() {
            return value;
        }
    }

    // 2. Synthesize closing quotes / brackets / braces for in-flight streams
    let mut repaired = trimmed.to_string();
    let mut in_quote = false;
    let mut escaped = false;
    for ch in repaired.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == '"' {
            in_quote = !in_quote;
        }
    }
    if in_quote {
        repaired.push('"');
    }
    let open_braces = repaired.chars().filter(|&c| c == '{').count();
    let close_braces = repaired.chars().filter(|&c| c == '}').count();
    for _ in close_braces..open_braces {
        repaired.push('}');
    }
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&repaired) {
        if value.is_object() {
            return value;
        }
    }

    // 3. Fallback: Extract key arguments via pattern matching from partial stream
    let mut map = serde_json::Map::new();
    let keys = [
        "TargetFile",
        "AbsolutePath",
        "path",
        "DirectoryPath",
        "SearchPath",
        "CommandLine",
        "command",
        "pattern",
        "Query",
        "query",
        "name",
        "src",
        "dest",
    ];
    for key in keys {
        let pattern = format!(r#""{}"\s*:\s*"([^"\\]*(?:\\.[^"\\]*)*)"#, key);
        if let Ok(re) = regex::Regex::new(&pattern) {
            if let Some(caps) = re.captures(trimmed) {
                if let Some(m) = caps.get(1) {
                    map.insert(
                        key.to_string(),
                        serde_json::Value::String(m.as_str().replace(r#"\""#, "\"")),
                    );
                }
            }
        }
    }
    serde_json::Value::Object(map)
}

/// Speculatively extract tool name and arguments from an in-flight text-fenced tool call (```tool ...).
pub(crate) fn parse_speculative_text_tool_call(content: &str) -> Option<(String, serde_json::Value)> {
    let pos = content.rfind("```tool\n").or_else(|| content.rfind("```tool\r\n"))?;
    let tail = &content[pos + 7..];
    if tail.contains("\n```") {
        return None;
    }
    let parsed = parse_speculative_arguments(tail);
    let name = parsed.get("name").and_then(|n| n.as_str())?.to_string();
    let args = parsed.get("arguments").cloned().unwrap_or(serde_json::json!({}));
    Some((name, args))
}

pub(crate) async fn estimate_token_usage(
    messages: &[serde_json::Value],
    reply: &str,
) -> Option<TokenUsage> {
    estimate_token_usage_with_tool_schemas(messages, reply, &[]).await
}

pub(crate) async fn estimate_token_usage_with_tool_schemas(
    messages: &[serde_json::Value],
    reply: &str,
    tool_schemas: &[serde_json::Value],
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
        if let Some(tool_calls) = msg.get("tool_calls") {
            prompt_text.push_str(&tool_calls.to_string());
            prompt_text.push('\n');
        }
        if let Some(tool_call_id) = msg.get("tool_call_id").and_then(|id| id.as_str()) {
            prompt_text.push_str(tool_call_id);
            prompt_text.push('\n');
        }
    }
    let schema_tokens = crate::network::compaction::estimate_tool_schema_tokens(tool_schemas);
    let prompt = count_tokens(&prompt_text).saturating_add(schema_tokens as u32);
    let full = prompt_text + reply + "\n";
    let total = count_tokens(&full).saturating_add(schema_tokens as u32);
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
    allow_tools: bool,
    disable_thinking: bool,
    schema_policy: crate::tools::ToolSchemaPolicy,
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
            .find(|p| {
                (p.model == model || p.name == model)
                    && (p.url == url || p.endpoint_url() == url)
            })
            .cloned()
    };
    let max_tokens = profile
        .as_ref()
        .map(|p| p.context_budget().completion_reserve)
        .unwrap_or_else(|| {
            crate::config::ModelProfile {
                name: model.to_string(),
                url: url.to_string(),
                model: model.to_string(),
                context_window: Some(crate::config::DEFAULT_CONTEXT_WINDOW),
                engine: None,
                api_key: None,
                env_key: None,
                tool_protocol: None,
                enable_thinking: None,
                reasoning_effort: None,
                thinking_budget: None,
                max_tokens: None,
                supports_vision: None,
                ..Default::default()
            }
            .context_budget()
            .completion_reserve
        });
    let thinking_budget = profile.as_ref().and_then(|p| {
        (!disable_thinking && p.enable_thinking != Some(false))
            .then_some(p.thinking_budget)
            .flatten()
    });

    let mut payload = serde_json::json!({
        "model": model,
        "messages": aligned_messages,
        "stream": true,
        "stream_options": {
            "include_usage": true
        },
        "max_tokens": max_tokens,
    });

    apply_profile_generation_options(&mut payload, profile.as_ref(), disable_thinking);

    if !url.contains("generativelanguage.googleapis.com") {
        payload["frequency_penalty"] = serde_json::json!(0.3);
    }

    let tool_protocol = { state.lock().await.active_tool_protocol() };
    let mut native_tool_schemas = Vec::new();
    if matches!(tool_protocol, crate::config::ToolProtocol::ApiNative) {
        let (selected_schema, mcp_selection) = if allow_tools {
            crate::tools::native_tools_schema_for_context(schema_policy, &aligned_messages)
        } else {
            (Vec::new(), crate::tools::McpSchemaSelectionStats::default())
        };
        native_tool_schemas = selected_schema;
        apply_api_native_tools(&mut payload, native_tool_schemas.clone(), allow_tools);
        crate::logger::operational_event(
            "mcp.native_schema_selection",
            serde_json::json!({
                "available": mcp_selection.available,
                "selected": mcp_selection.selected,
                "relevant": mcp_selection.relevant,
                "previously_used": mcp_selection.previously_used,
                "fallback": mcp_selection.fallback,
                "omitted": mcp_selection.omitted,
                "selected_names": mcp_selection.selected_names,
                "max": crate::tools::MAX_MCP_NATIVE_SCHEMAS,
                "allow_tools": allow_tools,
            }),
        );
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

    let request_start_time = std::time::Instant::now();
    let estimated_prompt_tokens = estimate_token_usage_with_tool_schemas(
        &aligned_messages,
        "",
        &native_tool_schemas,
    )
        .await
        .map(|u| u.prompt_tokens)
        .unwrap_or(0);
    let tool_schema_tokens =
        crate::network::compaction::estimate_tool_schema_tokens(&native_tool_schemas);

    crate::logger::operational_event(
        "context.request_composition",
        serde_json::json!({
            "model": model,
            "messages": message_count,
            "tools": tool_count,
            "payload_bytes": payload_bytes,
            "tool_schema_tokens": tool_schema_tokens,
            "estimated_prompt_tokens": estimated_prompt_tokens,
            "total_estimated_prompt_tokens": estimated_prompt_tokens,
            "max_tokens": max_tokens,
            "soft_context_target": profile.as_ref().map(|p| p.context_budget().soft_context_target),
            "hard_effective_limit": profile.as_ref().map(|p| p.context_budget().hard_effective_limit),
            "context_window": profile.as_ref().map(|p| p.context_budget().context_window),
        }),
    );

    crate::logger::operational_event(
        "provider.request_start",
        serde_json::json!({
            "model": model,
            "messages": message_count,
            "tools": tool_count,
            "payload_bytes": payload_bytes,
            "tool_schema_tokens": tool_schema_tokens,
            "estimated_prompt_tokens": estimated_prompt_tokens,
            "total_estimated_prompt_tokens": estimated_prompt_tokens,
        }),
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
                crate::logger::operational_event(
                    "provider.response_headers",
                    serde_json::json!({
                        "model": model,
                        "status": resp.status().as_u16(),
                        "elapsed_ms": request_start_time.elapsed().as_millis() as u64,
                    }),
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
    let mut reasoning_detector = super::loop_detect::ReasoningLoopDetector::default();
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
                                                 if !quiet && !acc.name.is_empty() {
                                                     let parsed_args = parse_speculative_arguments(&acc.arguments);
                                                     let id_opt = if acc.id.is_empty() { None } else { Some(acc.id.as_str()) };
                                                     let mut s = state.lock().await;
                                                     s.update_speculative_live_tool_call(id_opt, &acc.name, &parsed_args);
                                                 }
                                             }
                                         }

                                         let mut chunk = String::new();
                                        let mut reasoning_loop_cut = false;
                                        let mut reasoning_budget_cut = false;
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
                                            let used_tokens = buffer.lock().await.thought_tokens;
                                            let bounded = bound_reasoning_chunk(
                                                r_token,
                                                used_tokens,
                                                thinking_budget,
                                            );
                                            let thought_tokens = bounded.estimated_tokens;
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
                                            chunk.push_str(&bounded.text);
                                            if let super::loop_detect::ReasoningLoopStatus::LoopDetected(reason) =
                                                reasoning_detector.feed_chunk(&bounded.text)
                                            {
                                                dbg_log!(
                                                    "stream_request: reasoning loop detected ({reason}) — stopping stream cleanly"
                                                );
                                                crate::logger::operational_event(
                                                    "stream.reasoning_loop_cut",
                                                    serde_json::json!({ "reason": reason }),
                                                );
                                                finish_reason = Some("reasoning_loop".to_string());
                                                reasoning_loop_cut = true;
                                            }
                                            if bounded.budget_exhausted {
                                                dbg_log!(
                                                    "stream_request: client reasoning budget reached ({} estimated tokens) — stopping stream cleanly",
                                                    thinking_budget.unwrap_or_default()
                                                );
                                                crate::logger::operational_event(
                                                    "stream.reasoning_budget_cut",
                                                    serde_json::json!({
                                                        "budget": thinking_budget,
                                                        "estimated_thought_tokens": used_tokens.saturating_add(thought_tokens),
                                                    }),
                                                );
                                                finish_reason = Some("reasoning_budget".to_string());
                                                reasoning_budget_cut = true;
                                            }
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
                                            if !quiet {
                                                let buf_content = { buffer.lock().await.content.clone() };
                                                if let Some((tool_name, tool_args)) = parse_speculative_text_tool_call(&buf_content) {
                                                    let mut s = state.lock().await;
                                                    s.update_speculative_live_tool_call(None, &tool_name, &tool_args);
                                                }
                                            }
                                        }
                                        if reasoning_loop_cut || reasoning_budget_cut {
                                            line_buf.clear();
                                            break;
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

                                        let estimation_delta = if estimated_prompt_tokens > 0 {
                                            Some((p as i64) - (estimated_prompt_tokens as i64))
                                        } else {
                                            None
                                        };

                                        crate::logger::operational_event(
                                            "provider.completion",
                                            serde_json::json!({
                                                "model": model,
                                                "prompt_tokens": p,
                                                "completion_tokens": c,
                                                "total_tokens": t,
                                                "cached_tokens": cached,
                                                "requested_max_tokens": max_tokens,
                                                "thinking_budget": thinking_budget,
                                                "completion_limit_reached": c >= u64::from(max_tokens),
                                                "estimated_prompt_tokens": estimated_prompt_tokens,
                                                "tool_schema_tokens": tool_schema_tokens,
                                                "total_estimated_prompt_tokens": estimated_prompt_tokens,
                                                "estimation_delta": estimation_delta,
                                                "elapsed_ms": request_start_time.elapsed().as_millis() as u64,
                                            }),
                                        );
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
