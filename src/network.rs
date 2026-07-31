use crate::app::{AppState, AppStatus, ChatMessage, StreamTracker, TokenUsage, ToolConfirmation};
use futures_util::StreamExt;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Mutex;
use tokio_util::io::StreamReader;

#[path = "network/compaction.rs"]
pub(crate) mod compaction;

#[path = "network/retry.rs"]
pub(crate) mod retry;

#[path = "network/loop_detect.rs"]
pub(crate) mod loop_detect;

#[path = "network/helpers.rs"]
pub(crate) mod helpers;
pub(crate) use helpers::{count_tokens, classify_tool_msg, parse_sse_line};

#[path = "network/messages.rs"]
pub(crate) mod messages;
pub(crate) use messages::{RESPONSE_RESERVE_TOKENS, append_to_last_message, trim_msgs_to_budget, inject_system_reminder};

#[path = "network/text.rs"]
pub(crate) mod text;
use text::{
    cap_diff_lines, has_intended_tool_call, is_cut_off, strip_ansi_escapes, strip_leading_think,
    strip_think_blocks, strip_tool_call_syntax,
};

#[path = "network/stream.rs"]
pub(crate) mod stream;
pub(crate) use stream::StreamBuffer;

#[path = "network/output.rs"]
pub(crate) mod output;
pub(crate) use output::truncate_tool_output;

#[path = "network/events.rs"]
pub(crate) mod events;
pub(crate) use events::{ToolResult, ToolResultMetadata};

#[path = "network/history.rs"]
pub(crate) mod history;

#[path = "network/runner.rs"]
pub(crate) mod runner;

#[path = "network/policy.rs"]
pub(crate) mod policy;



/// Injected as a system directive for the final wrap-up turn after a loop is
/// detected. Disables tools and forces a prose answer so the user gets a
/// summary instead of a silently aborted session. Ported from opencode's
/// `MAX_STEPS_PROMPT`.
const FORCE_ANSWER_PROMPT: &str = "CRITICAL — you are stuck in a loop. Tools are now DISABLED for this turn. \
Do NOT emit any tool calls (no reads, writes, edits, searches). Respond with TEXT ONLY, and include: \
a short statement that you stopped to avoid looping, a summary of what you found or accomplished so far, \
any remaining tasks, and a recommendation for what to do next. This overrides all other instructions.";



/// True when a tool result has already been reduced to a stub (nothing left to prune).
fn is_fully_stubbed(m: &ChatMessage) -> bool {
    let rest = m
        .content
        .split_once(':')
        .map(|x| x.1)
        .unwrap_or("")
        .trim_start();
    rest.starts_with("[Tool output truncated")
        || rest.starts_with("[superseded")
}


/// Reduce one tool message a single notch toward a stub (full → 2 lines → fully
/// stubbed). Returns the new token count. Idempotent on already-stubbed messages.
async fn reduce_tool_msg(m: &mut ChatMessage, current_tokens: u32) -> u32 {
    let tool_name = m
        .content
        .split(':')
        .next()
        .unwrap_or("tool")
        .trim()
        .to_string();
    let rest = m
        .content
        .split_once(':')
        .map(|x| x.1)
        .unwrap_or("")
        .to_string();

    if is_fully_stubbed(m) {
        return current_tokens;
    }

    let lines: Vec<&str> = rest.lines().collect();
    if lines.len() > 2 {
        let truncated = format!("{}: {}\n{}", tool_name, lines[0], lines[1]);
        let t = count_tokens(&truncated);
        m.content = truncated;
        t
    } else {
        let stubbed = format!(
            "{}: [Tool output truncated: {} tokens pruned to maintain context window]",
            tool_name, current_tokens
        );
        count_tokens(&stubbed)
    }
}

/// Repeatedly reduce the oldest non-stubbed tool result of `class` until under budget
/// or the class is exhausted. Mutates `history`, `tokens`, and `total` in place.
async fn prune_class(
    history: &mut [ChatMessage],
    tokens: &mut [u32],
    total: &mut u32,
    budget: u32,
    class: &'static str,
) {
    while *total > budget {
        let target = history
            .iter()
            .enumerate()
            .find(|(_, m)| classify_tool_msg(m) == Some(class) && !is_fully_stubbed(m))
            .map(|(i, _)| i);
        let Some(idx) = target else { return; };
        let before = tokens[idx];
        let new_t = reduce_tool_msg(&mut history[idx], before).await;
        if new_t >= before {
            // Defensive: nothing more we can do here.
            return;
        }
        *total = total.saturating_sub(before).saturating_add(new_t);
        tokens[idx] = new_t;
    }
}

pub(crate) async fn compact_history_to_budget(history: &mut [ChatMessage], budget: u32) {
    if history.is_empty() {
        return;
    }

    // Strip <think> blocks from all assistant messages first to free up budget.
    for m in history.iter_mut() {
        if m.role == "assistant" {
            m.content = strip_think_blocks(&m.content);
        }
    }

    // Drop superseded reads of the same file before measuring tokens.
    // dedup disabled

    let mut tokens = Vec::with_capacity(history.len());
    for m in history.iter() {
        tokens.push(count_tokens(&m.content));
    }
    let mut total: u32 = tokens.iter().sum();
    if total <= budget {
        return;
    }

    dbg_log!(
        "History tokens ({}) exceed budget ({}). Compacting tool outputs by priority.",
        total,
        budget
    );

    // Prune lowest-value outputs first: throwaway snapshots, then file contents,
    // then anything else still taking space. Each class is flattened oldest-first.
    prune_class(history, &mut tokens, &mut total, budget, "throwaway").await;
    prune_class(history, &mut tokens, &mut total, budget, "file").await;
    prune_class(history, &mut tokens, &mut total, budget, "other").await;

    dbg_log!("Compact finished. New history tokens: {}", total);
    crate::logger::operational_event(
        "context.compaction",
        serde_json::json!({"history_tokens": total, "budget": budget}),
    );
}


async fn estimate_token_usage(messages: &[serde_json::Value], reply: &str) -> Option<TokenUsage> {
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

/// Extract a context length from ollama's /api/show `model_info` blob;
/// the key is architecture-prefixed, e.g. "llama.context_length".
fn context_length_from_model_info(info: &serde_json::Value) -> Option<u32> {
    info.as_object()?
        .iter()
        .find(|(k, _)| k.ends_with(".context_length"))
        .and_then(|(_, v)| v.as_u64())
        .map(|n| n as u32)
}

/// Ask an ollama server for a model's context window. Returns None for
/// non-ollama endpoints or on any error.
pub async fn fetch_context_window(
    client: &reqwest::Client,
    chat_url: &str,
    model: &str,
    engine: Option<&str>,
) -> Option<u32> {
    let base = chat_url.strip_suffix("/v1/chat/completions")?;

    if let Some(eng) = engine {
        match eng.to_lowercase().as_str() {
            "ollama" => {
                let show_url = format!("{base}/api/show");
                let resp = client
                    .post(&show_url)
                    .json(&serde_json::json!({"model": model}))
                    .send()
                    .await
                    .ok()?;
                if resp.status().is_success() {
                    let body: serde_json::Value = resp.json().await.ok()?;
                    if let Some(ctx) = context_length_from_model_info(body.get("model_info")?) {
                        return Some(ctx);
                    }
                }
            }
            "llamacpp" | "llama.cpp" | "llama" => {
                let props_url = format!("{base}/props");
                let resp = client.get(&props_url).send().await.ok()?;
                if resp.status().is_success() {
                    let body: serde_json::Value = resp.json().await.ok()?;
                    if let Some(n) = body
                        .get("default_generation_settings")
                        .and_then(|v| v.get("n_ctx"))
                        .and_then(|v| v.as_u64())
                    {
                        return Some(n as u32);
                    }
                    if let Some(n) = body.get("n_ctx").and_then(|v| v.as_u64()) {
                        return Some(n as u32);
                    }
                }
            }
            _ => {}
        }
    }

    // Fallback: try llama.cpp first, then Ollama
    let props_url = format!("{base}/props");
    if let Ok(resp) = client.get(&props_url).send().await
        && resp.status().is_success()
            && let Ok(body) = resp.json::<serde_json::Value>().await {
                if let Some(n) = body
                    .get("default_generation_settings")
                    .and_then(|v| v.get("n_ctx"))
                    .and_then(|v| v.as_u64())
                {
                    return Some(n as u32);
                }
                if let Some(n) = body.get("n_ctx").and_then(|v| v.as_u64()) {
                    return Some(n as u32);
                }
            }

    let show_url = format!("{base}/api/show");
    let resp = client
        .post(&show_url)
        .json(&serde_json::json!({"model": model}))
        .send()
        .await
        .ok()?;
    if resp.status().is_success() {
        let body: serde_json::Value = resp.json().await.ok()?;
        if let Some(ctx) = context_length_from_model_info(body.get("model_info")?) {
            return Some(ctx);
        }
    }

    None
}


/// Read-only tools whose results can be safely short-circuited by the repeat guard.
fn is_read_only_tool(name: &str) -> bool {
    matches!(
        crate::tools::tool_safety(name),
        crate::tools::ToolSafety::ReadOnly
    )
}

/// Tools that write to the filesystem — the ones whose result runs a compiler
/// check and that the finish gate cares about.
fn is_mutating_tool(name: &str) -> bool {
    matches!(
        name,
        "replace_file_content"
            | "multi_replace_file_content"
            | "write_to_file"
            | "delete_file"
            | "move_file"
            | "copy_file"
            | "spawn_agent"
            | "send_agent"
    )
}

/// True only if we have read this file before AND its mtime is unchanged since.
/// A re-read is allowed whenever the file is new, missing, or modified on disk —
/// so the agent can always refresh after a (possibly partial) edit.
fn view_file_unchanged_since_last_read(
    stored: Option<std::time::SystemTime>,
    current: Option<std::time::SystemTime>,
) -> bool {
    matches!((stored, current), (Some(a), Some(b)) if a == b)
}

/// Best-effort mtime of the resolved tool path (None if it can't be stat'd).
fn path_mtime(raw_path: &str) -> Option<std::time::SystemTime> {
    std::fs::metadata(crate::tools::resolve_tool_path(raw_path))
        .and_then(|m| m.modified())
        .ok()
}

/// A canonical key identifying "the same call" for the repeat guard.
fn tool_signature(name: &str, args: &serde_json::Value) -> String {
    let key = match name {
        // Bucket full/default reads together so paging can't bypass the guard.
        "view_file" => {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let start = args.get("start_line").and_then(|v| v.as_u64()).unwrap_or(1);
            let end_str = args
                .get("end_line")
                .and_then(|v| v.as_u64())
                .map(|e| e.to_string())
                .unwrap_or_else(|| "end".to_string());
            format!("{path}|{start}-{end_str}")
        }
        _ => serde_json::to_string(args).unwrap_or_default(),
    };
    format!("{name}:{key}")
}

fn align_alternating_messages(raw_msgs: Vec<serde_json::Value>) -> Vec<serde_json::Value> {
    if raw_msgs.is_empty() {
        return raw_msgs;
    }

    let mut msgs = Vec::new();
    let mut system_content = String::new();

    // 1. Extract and merge all system messages
    for msg in raw_msgs {
        if let Some(role) = msg.get("role").and_then(|r| r.as_str()) {
            if role == "system" {
                if let Some(content) = msg.get("content").and_then(|c| c.as_str()) {
                    if !system_content.is_empty() {
                        system_content.push_str("\n\n");
                    }
                    system_content.push_str(content);
                }
            } else {
                msgs.push(msg);
            }
        }
    }

    let mut final_msgs = Vec::new();
    if !system_content.is_empty() {
        final_msgs.push(serde_json::json!({
            "role": "system",
            "content": system_content,
        }));
    }

    if msgs.is_empty() {
        return final_msgs;
    }

    // 2. Ensure the first message is a "user" message
    let first_role = msgs[0].get("role").and_then(|r| r.as_str()).unwrap_or("user");
    if first_role != "user" {
        final_msgs.push(serde_json::json!({
            "role": "user",
            "content": "[Context initialization]",
        }));
    }

    // 3. Alternate roles, merging consecutive same-role non-tool messages
    for msg in msgs {
        let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("user");
        let content = msg.get("content").and_then(|c| c.as_str()).unwrap_or("").to_string();

        if let Some(last) = final_msgs.last_mut() {
            let last_role = last.get("role").and_then(|r| r.as_str()).unwrap_or("user");
            if last_role == role && role != "tool" {
                if let Some(last_content) = last.get_mut("content") {
                    let mut new_content = last_content.as_str().unwrap_or("").to_string();
                    new_content.push_str("\n\n");
                    new_content.push_str(&content);
                    *last_content = serde_json::Value::String(new_content);
                }
                continue;
            }
        }
        final_msgs.push(msg);
    }

    final_msgs
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
    let mut payload = serde_json::json!({
        "model": model,
        "messages": aligned_messages,
        "stream": true,
        "stream_options": {
            "include_usage": true
        },
        // Low temperature for the main agent loop: this drives structured
        // tool-calling and code edits, where 0.7 makes small models incoherent
        // and prone to token-level repetition collapse (e.g. a regex degenerating
        // into `.*?\n` repeated hundreds of times). This is sent explicitly
        // because a request value overrides the model's Modelfile PARAMETER, so
        // the server-side temperature can't be relied on. Keep it low.
        "temperature": 0.2,
        "max_tokens": 4096,
    });

    // Guard against runaway repetition even at low temperature. Google's
    // OpenAI-compat endpoint (generativelanguage.googleapis.com) rejects
    // `frequency_penalty` with a 400, so only send it to providers that accept
    // it — which is also where small open models need the repetition guard most.
    if !url.contains("generativelanguage.googleapis.com") {
        payload["frequency_penalty"] = serde_json::json!(0.3);
    }

    // ApiNative protocol: attach the tool schema so the provider returns
    // structured `tool_calls` (handled by the SSE accumulator below) instead of
    // the model writing tool calls as text. Only sent for this opt-in protocol;
    // text protocols leave the payload untouched.
    let tool_protocol = { state.lock().await.config.tool_protocol };
    if matches!(tool_protocol, crate::config::ToolProtocol::ApiNative) {
        // Served from the same PromptCache as the system prompt (built together
        // under one key), so this is a hit after prepare_turn_request ran.
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

    dbg_log!(
        "stream_request: Request payload: {}",
        serde_json::to_string_pretty(&payload).unwrap_or_default()
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

    // Establish the connection with retry/backoff on transient failures
    // (429, 5xx, network blips). We only retry here, before any SSE bytes are
    // read — retrying mid-stream would duplicate partial output.
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
        match req.send().await {
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
                let err_body = resp.text().await.unwrap_or_default();
                if retry::is_retryable_status(code) && attempt < retry::MAX_RETRIES {
                    let delay = retry::delay_for_attempt(attempt, code);
                    dbg_log!(
                        "stream_request: retryable status {} (attempt {}/{}), backing off {}ms",
                        status,
                        attempt + 1,
                        retry::MAX_RETRIES,
                        delay.as_millis()
                    );
                    tokio::time::sleep(delay).await;
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
                    tokio::time::sleep(delay).await;
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
        name: String,
        arguments: String,
    }
    let mut accumulators: Vec<ToolAccumulator> = Vec::new();

    dbg_log!("stream_request: Starting SSE stream read loop");
    loop {
        if cancel_token.is_cancelled() {
            dbg_log!("stream_request: Stream reading cancelled via token");
            return Ok(None);
        }

        tokio::select! {
            r = reader.read_line(&mut line_buf) => {
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
                                             .and_then(|d| d.get("reasoning").or_else(|| d.get("reasoning_content")))
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
                                                         name: String::new(),
                                                         arguments: String::new(),
                                                     });
                                                 }
                                                 let acc = &mut accumulators[idx];
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
                                                chunk.push_str("<think>\n");
                                            }
                                            chunk.push_str(r_token);
                                        } else if let Some(c_token) = content {
                                            if in_reasoning {
                                                in_reasoning = false;
                                                chunk.push_str("\n</think>\n\n");
                                            }
                                            chunk.push_str(c_token);
                                        }
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

    let mut translation = String::new();
    for acc in &accumulators {
        if acc.name.is_empty() {
            continue;
        }

        let args_json = parse_native_tool_arguments(&acc.arguments);

        let tool_call_obj = serde_json::json!({
            "name": acc.name,
            "arguments": args_json
        });

        translation.push_str("\n\n```tool\n");
        translation.push_str(&serde_json::to_string(&tool_call_obj).unwrap_or_default());
        translation.push_str("\n```\n");
    }

    if !translation.is_empty() {
        dbg_log!(
            "stream_request: Translating and appending native tool call: {}",
            translation
        );
        buffer.lock().await.content.push_str(&translation);
        if !quiet {
            let mut s = state.lock().await;
            s.current_response.push_str(&translation);
            if s.raw_cli_mode {
                use std::io::Write;
                print!("{translation}");
                let _ = std::io::stdout().flush();
            }
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

/// Preserve malformed native arguments for validation and model feedback
/// instead of silently turning them into an empty object. This keeps the raw
/// provider failure visible while ensuring the call cannot execute.
fn parse_native_tool_arguments(raw: &str) -> serde_json::Value {
    match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(value) if value.is_object() => value,
        Ok(value) => serde_json::json!({ "_invalid_arguments": value }),
        Err(error) => serde_json::json!({
            "_invalid_arguments": raw,
            "_parse_error": error.to_string(),
        }),
    }
}

pub(crate) fn get_diff_preview(name: &str, args: &serde_json::Value) -> Option<String> {
    if name == "replace_file_content" {
        let search_block = args
            .get("target_content")
            .and_then(|s| s.as_str())
            .unwrap_or("");
        let replace_block = args
            .get("replacement_content")
            .and_then(|s| s.as_str())
            .unwrap_or("");

        let diff = similar::TextDiff::from_lines(search_block, replace_block);
        let old_slices: Vec<&str> = diff.iter_old_slices().collect();
        let new_slices: Vec<&str> = diff.iter_new_slices().collect();

        let mut prev = String::new();
        for op in diff.ops() {
            let old_slice = &old_slices[op.old_range()];
            let new_slice = &new_slices[op.new_range()];
            match op.tag() {
                similar::DiffTag::Equal => {
                    for (o, n) in old_slice.iter().zip(new_slice.iter()) {
                        prev.push_str(&format!(" {}\x00 {}\n", o.trim_end_matches('\n').trim_end_matches('\r'), n.trim_end_matches('\n').trim_end_matches('\r')));
                    }
                }
                similar::DiffTag::Delete => {
                    for o in old_slice {
                        prev.push_str(&format!("-{}\x00~\n", o.trim_end_matches('\n').trim_end_matches('\r')));
                    }
                }
                similar::DiffTag::Insert => {
                    for n in new_slice {
                        prev.push_str(&format!("~\x00+{}\n", n.trim_end_matches('\n').trim_end_matches('\r')));
                    }
                }
                similar::DiffTag::Replace => {
                    let max_len = old_slice.len().max(new_slice.len());
                    for i in 0..max_len {
                        let o_val = old_slice.get(i);
                        let n_val = new_slice.get(i);
                        match (o_val, n_val) {
                            (Some(o), Some(n)) => {
                                prev.push_str(&format!("-{}\x00+{}\n", o.trim_end_matches('\n').trim_end_matches('\r'), n.trim_end_matches('\n').trim_end_matches('\r')));
                            }
                            (Some(o), None) => {
                                prev.push_str(&format!("-{}\x00~\n", o.trim_end_matches('\n').trim_end_matches('\r')));
                            }
                            (None, Some(n)) => {
                                prev.push_str(&format!("~\x00+{}\n", n.trim_end_matches('\n').trim_end_matches('\r')));
                            }
                            (None, None) => {}
                        }
                    }
                }
            }
        }
        Some(cap_diff_lines(prev))
    } else if name == "write_to_file" && args.get("__rustcode_legacy_write_diff").is_some() {
        let path = args.get("path").and_then(|p| p.as_str()).unwrap_or("");
        let old_content = std::fs::read_to_string(path).unwrap_or_default();
        let new_content = args.get("content").and_then(|c| c.as_str()).unwrap_or("");

        let diff = similar::TextDiff::from_lines(&old_content, new_content);
        let old_slices: Vec<&str> = diff.iter_old_slices().collect();
        let new_slices: Vec<&str> = diff.iter_new_slices().collect();

        let mut prev = String::new();
        for group in diff.grouped_ops(3) {
            for op in group {
                let old_slice = &old_slices[op.old_range()];
                let new_slice = &new_slices[op.new_range()];
                match op.tag() {
                    similar::DiffTag::Equal => {
                        for (o, n) in old_slice.iter().zip(new_slice.iter()) {
                            prev.push_str(&format!(" {}\x00 {}\n", o.trim_end_matches('\n').trim_end_matches('\r'), n.trim_end_matches('\n').trim_end_matches('\r')));
                        }
                    }
                    similar::DiffTag::Delete => {
                        for o in old_slice {
                            prev.push_str(&format!("-{}\x00~\n", o.trim_end_matches('\n').trim_end_matches('\r')));
                        }
                    }
                    similar::DiffTag::Insert => {
                        for n in new_slice {
                            prev.push_str(&format!("~\x00+{}\n", n.trim_end_matches('\n').trim_end_matches('\r')));
                        }
                    }
                    similar::DiffTag::Replace => {
                        let max_len = old_slice.len().max(new_slice.len());
                        for i in 0..max_len {
                            let o_val = old_slice.get(i);
                            let n_val = new_slice.get(i);
                            match (o_val, n_val) {
                                (Some(o), Some(n)) => {
                                    prev.push_str(&format!("-{}\x00+{}\n", o.trim_end_matches('\n').trim_end_matches('\r'), n.trim_end_matches('\n').trim_end_matches('\r')));
                                }
                                (Some(o), None) => {
                                    prev.push_str(&format!("-{}\x00~\n", o.trim_end_matches('\n').trim_end_matches('\r')));
                                }
                                (None, Some(n)) => {
                                    prev.push_str(&format!("~\x00+{}\n", n.trim_end_matches('\n').trim_end_matches('\r')));
                                }
                                (None, None) => {}
                            }
                        }
                    }
                }
            }
        }
        Some(cap_diff_lines(prev))
    } else {
        None
    }
}

fn get_file_preview(name: &str, args: &serde_json::Value) -> Option<(String, String)> {
    if name != "write_to_file" {
        return None;
    }
    Some((
        args.get("path")?.as_str()?.to_string(),
        args.get("content")?.as_str()?.to_string(),
    ))
}

fn get_tool_project_root(_name: &str, args: &serde_json::Value) -> std::path::PathBuf {
    let raw_path = if let Some(p) = args.get("path").and_then(|p| p.as_str()) {
        Some(p)
    } else if let Some(s) = args.get("src").and_then(|s| s.as_str()) {
        Some(s)
    } else { args.get("dest").and_then(|d| d.as_str()) };

    let resolved = if let Some(rp) = raw_path {
        crate::tools::resolve_tool_path(rp)
    } else {
        std::env::current_dir().unwrap_or_default()
    };

    // Find project root from resolved path
    let mut current = if resolved.is_dir() {
        resolved.clone()
    } else {
        resolved.parent().map(|p| p.to_path_buf()).unwrap_or(resolved)
    };

    loop {
        if current.join("Cargo.toml").exists() || current.join("tsconfig.json").exists() {
            // `Path::parent()` turns a relative `src/...` path into `""` at
            // the workspace root. That path works for joins but is invalid as
            // a child-process cwd, causing cargo to fail with ENOENT. Always
            // hand verification an existing absolute directory.
            return current
                .canonicalize()
                .or_else(|_| std::env::current_dir())
                .unwrap_or(current);
        }
        if let Some(parent) = current.parent() {
            current = parent.to_path_buf();
        } else {
            break;
        }
    }

    std::env::current_dir().unwrap_or_default()
}

/// Resolve a build tool to an absolute path. GUI-launched apps (and some
/// spawned environments) don't inherit the shell PATH, so a bare
/// `Command::new("cargo")` fails with ENOENT even though the tool is installed.
/// Check the canonical install locations first, then fall back to the bare
/// name (which relies on PATH) so terminal launches still work.
fn resolve_bin(name: &str) -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    let candidates = [
        format!("{home}/.cargo/bin/{name}"),
        format!("/opt/homebrew/bin/{name}"),
        format!("/usr/local/bin/{name}"),
        format!("/usr/bin/{name}"),
    ];
    for c in candidates {
        if std::path::Path::new(&c).exists() {
            return std::path::PathBuf::from(c);
        }
    }
    std::path::PathBuf::from(name)
}

/// PATH for spawned build tools. GUI/Dock launches don't inherit the shell
/// PATH, so `cargo` can't find `rustc` (and bare-name spawns fail with ENOENT)
/// even though the toolchain is installed. Prepend the canonical toolchain dirs
/// to whatever PATH we did inherit so the checker — and its own subprocesses —
/// resolve regardless of how rustcode was launched.
pub(crate) fn augmented_path() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut dirs = vec![
        format!("{home}/.cargo/bin"),
        "/opt/homebrew/bin".to_string(),
        "/usr/local/bin".to_string(),
        "/usr/bin".to_string(),
        "/bin".to_string(),
    ];
    if let Ok(existing) = std::env::var("PATH") {
        dirs.extend(existing.split(':').map(|s| s.to_string()));
    }
    dirs.join(":")
}

async fn run_compiler_check(cwd: &std::path::Path) -> Option<String> {
    if cwd.join("Cargo.toml").exists() {
        // Run through `sh -c` (like run_command) so the SHELL resolves `cargo`
        // using the augmented PATH. A bare-name direct spawn
        // (`Command::new("cargo")`) does not use the command's env PATH for
        // program lookup, so on GUI/Dock launches — where `resolve_bin`'s
        // exists() checks can't see /opt/homebrew — it fell back to "cargo" and
        // failed with ENOENT even though `cargo check` via run_command worked.
        let mut cmd = tokio::process::Command::new("/bin/sh");
        cmd.args(["-c", "cargo check --message-format=json"])
            .current_dir(cwd)
            .env("PATH", augmented_path())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                dbg_log!("Could not spawn cargo check ({e}), skipping compiler check");
                return Some(format!(
                    "__BUILD_UNVERIFIED__: could not run `cargo check` ({e}). \
                     The build was NOT verified — do not claim the task compiles."
                ));
            }
        };

        // `cargo check` on a non-trivial crate routinely exceeds a few seconds,
        // especially the first run after edits. Too short a timeout leaves the
        // agent blind to compile errors — the whole point of this check.
        let timeout_duration = std::time::Duration::from_secs(120);
        let output_res = tokio::time::timeout(timeout_duration, child.wait_with_output()).await;

        let output = match output_res {
            Ok(Ok(out)) => out,
            Ok(Err(e)) => {
                dbg_log!("cargo check failed to run ({e}), skipping compiler check");
                return Some(format!(
                    "__BUILD_UNVERIFIED__: `cargo check` failed to run ({e}). \
                     The build was NOT verified."
                ));
            }
            Err(_) => {
                dbg_log!("cargo check timed out, skipping compiler check");
                return Some(
                    "__BUILD_UNVERIFIED__: `cargo check` timed out. \
                     The build was NOT verified."
                        .to_string(),
                );
            }
        };

        let stdout_str = String::from_utf8_lossy(&output.stdout);
        let mut errors = Vec::new();

        for line in stdout_str.lines() {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(line)
                && val.get("reason").and_then(|r| r.as_str()) == Some("compiler-message")
                    && let Some(msg) = val.get("message")
                        && let Some(level) = msg.get("level").and_then(|l| l.as_str())
                            && level == "error"
                                && let Some(rendered) = msg.get("rendered").and_then(|r| r.as_str()) {
                                    errors.push(strip_ansi_escapes(rendered));
                                }
        }

        if !errors.is_empty() {
            return Some(errors.join("\n"));
        }
    } else if cwd.join("biome.json").exists() || cwd.join("biome.jsonc").exists() {
        let (runner, bin_arg) = if resolve_bin("bunx").exists() {
            (resolve_bin("bunx"), "biome")
        } else {
            (resolve_bin("npx"), "@biomejs/biome")
        };

        let mut cmd = tokio::process::Command::new(runner);
        cmd.args([bin_arg, "check", "."])
            .current_dir(cwd)
            .env("PATH", augmented_path())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                dbg_log!("Could not spawn biome check ({e}), skipping compiler check");
                return None;
            }
        };

        let timeout_duration = std::time::Duration::from_secs(60);
        let output_res = tokio::time::timeout(timeout_duration, child.wait_with_output()).await;

        let output = match output_res {
            Ok(Ok(out)) => out,
            Ok(Err(_)) | Err(_) => return None,
        };

        if !output.status.success() {
            let stdout_str = String::from_utf8_lossy(&output.stdout);
            let stderr_str = String::from_utf8_lossy(&output.stderr);
            let combined = format!("{stdout_str}\n{stderr_str}");
            let trimmed = combined.trim();
            if !trimmed.is_empty() {
                return Some(strip_ansi_escapes(trimmed));
            }
        }
    } else if cwd.join("tsconfig.json").exists() {
        let (runner, bin_arg) = if resolve_bin("bunx").exists() {
            (resolve_bin("bunx"), "tsc")
        } else {
            (resolve_bin("npx"), "tsc")
        };

        let mut cmd = tokio::process::Command::new(runner);
        cmd.args([bin_arg, "--noEmit"])
            .current_dir(cwd)
            .env("PATH", augmented_path())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                dbg_log!("Could not spawn {bin_arg} ({e}), skipping compiler check");
                return None;
            }
        };

        let timeout_duration = std::time::Duration::from_secs(60);
        let output_res = tokio::time::timeout(timeout_duration, child.wait_with_output()).await;

        let output = match output_res {
            Ok(Ok(out)) => out,
            Ok(Err(_)) | Err(_) => return None,
        };

        if !output.status.success() {
            let stdout_str = String::from_utf8_lossy(&output.stdout);
            let stderr_str = String::from_utf8_lossy(&output.stderr);
            let combined = format!("{stdout_str}\n{stderr_str}");
            let trimmed = combined.trim();
            if !trimmed.is_empty() {
                return Some(strip_ansi_escapes(trimmed));
            }
        }
    }

    None
}

/// Run a compiler check, reusing the previous result when the tree hasn't been
/// dirtied since. `cargo check` is slow, and one task can hit the check at
/// several points in a single round (inline after a tool batch, then again at
/// the finish gate). Without this, an edit-and-complete round runs `cargo check`
/// two or three times over an identical tree. `dirty` is set by the caller
/// whenever a mutating tool runs; this clears it after a fresh check.
async fn cached_compiler_check(
    root: &std::path::Path,
    dirty: &mut bool,
    cache: &mut Option<(std::path::PathBuf, Option<String>)>,
) -> Option<String> {
    if !*dirty
        && let Some((cached_root, cached_result)) = cache.as_ref()
        && cached_root == root
    {
        dbg_log!("Compiler check: reusing cached result (tree unchanged since last check)");
        return cached_result.clone();
    }
    let result = run_compiler_check(root).await;
    *cache = Some((root.to_path_buf(), result.clone()));
    *dirty = false;
    result
}

/// Handle an interactive `ask_question` tool call: show the option-picker modal
/// and block until the user chooses (or cancels / the turn is cancelled). Returns
/// the chosen option text — that becomes the tool result fed back to the model,
/// so it can continue with the user's answer.
async fn ask_user_question(
    state: &Arc<Mutex<AppState>>,
    cancel_token: &tokio_util::sync::CancellationToken,
    args: &serde_json::Value,
) -> String {
    let question = args
        .get("question")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let options: Vec<String> = args
        .get("options")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|o| o.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let is_multi_select = args
        .get("is_multi_select")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if question.is_empty() || options.is_empty() {
        return "error: ask_question requires a non-empty 'question' and 'options'".to_string();
    }

    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    {
        let mut s = state.lock().await;
        s.pending_question = Some(crate::app::PendingQuestion {
            question,
            chosen: vec![false; options.len()],
            options,
            is_multi_select,
            selected: 0,
            custom_input: None,
        });
        s.question_response = Some(tx);
        s.status = AppStatus::AwaitingQuestion;
    }
    let _ = crate::notifications::notify_pending_confirmation("ask_question");

    let answer = tokio::select! {
        _ = cancel_token.cancelled() => None,
        res = rx => res.ok(),
    };

    {
        let mut s = state.lock().await;
        s.pending_question = None;
        s.question_response = None;
        if s.status == AppStatus::AwaitingQuestion {
            let model_name = s.model_name.clone();
            s.status = AppStatus::Streaming;
            if s.config.discord_rpc_enabled {
                s.discord_rpc.set_activity("Streaming", &format!("Using model: {}", model_name));
            }
        }
    }

    match answer {
        Some(a) if !a.is_empty() => format!("User selected: {a}"),
        _ => "error: the user dismissed the question without answering".to_string(),
    }
}

/// Show the Y/N confirmation modal (when the tool requires it) and run the
/// tool. `display_name` is what the modal shows — subagent calls prefix it
/// with the agent id so the user knows who is asking.
async fn confirm_and_execute(
    state: &Arc<Mutex<AppState>>,
    cancel_token: &tokio_util::sync::CancellationToken,
    name: &str,
    args: &serde_json::Value,
    display_name: &str,
    bypass_confirm: bool,
    workspace_root: Option<std::path::PathBuf>,
) -> (String, Option<String>) {
    let (agent_mode, auto_confirm) = {
        let s = state.lock().await;
        (s.agent_mode, s.auto_confirm)
    };
    if let crate::tools::AuthorizationDecision::Deny(reason) =
        crate::tools::authorize_tool(name, agent_mode, auto_confirm, bypass_confirm)
    {
        return (format!("error: {reason}"), None);
    }

    struct ToolCleanup {
        state: Arc<Mutex<AppState>>,
        tool_name: String,
    }
    impl Drop for ToolCleanup {
        fn drop(&mut self) {
            let state = self.state.clone();
            let tool_name = self.tool_name.clone();
            tokio::spawn(async move {
                let mut s = state.lock().await;
                if let Some(pos) = s.running_tools.iter().position(|t| t == &tool_name) {
                    s.running_tools.remove(pos);
                }
            });
        }
    }

    let diff_opt = get_diff_preview(name, args);

    let needs_confirm = matches!(
        crate::tools::authorize_tool(name, agent_mode, auto_confirm, bypass_confirm),
        crate::tools::AuthorizationDecision::RequireConfirmation
    );
    let mut result = if !needs_confirm {
        dbg_log!("Executing tool '{}' immediately...", name);
        let tool_name = name.to_string();
        {
            let mut s = state.lock().await;
            s.running_tools.push(tool_name.clone());
        }
        let _cleanup = ToolCleanup {
            state: Arc::clone(state),
            tool_name,
        };

        let name_owned = name.to_string();
        let args_owned = args.clone();
        let session_id = { state.lock().await.active_session_id.clone() };
        let workspace_root_for_task = workspace_root.clone();
        let run_fut = tokio::task::spawn_blocking(move || {
            crate::tools::set_active_session_id(Some(session_id));
            crate::tools::set_active_workspace_root(workspace_root_for_task);
            let result = crate::tools::execute(&name_owned, &args_owned);
            crate::tools::set_active_workspace_root(None);
            crate::tools::set_active_session_id(None);
            result
        });

        tokio::select! {
            res = run_fut => {
                res.unwrap_or_else(|e| format!("tool panicked: {e}"))
            }
            _ = cancel_token.cancelled() => {
                dbg_log!("Tool execution cancelled during spawn_blocking await (immediate execution)");
                "error: tool execution cancelled by user".to_string()
            }
        }
    } else {
        dbg_log!("Tool '{}' requires confirmation", name);
        let path = if let Some(p) = args.get("path").and_then(|p| p.as_str()) {
            p.to_string()
        } else if let Some(cmd) = args.get("command").and_then(|c| c.as_str()) {
            cmd.to_string()
        } else if let (Some(src), Some(dest)) = (
            args.get("src").and_then(|s| s.as_str()),
            args.get("dest").and_then(|d| d.as_str()),
        ) {
            format!("{src} -> {dest}")
        } else {
            "?".to_string()
        };
        let (preview, content_bytes) = if let Some(ref d) = diff_opt {
            (d.clone(), d.len())
        } else {
            let content = args.get("content").and_then(|c| c.as_str()).unwrap_or("");
            let preview = content.lines().take(6).collect::<Vec<_>>().join("\n");
            (preview, content.len())
        };
        let (tx, rx) = tokio::sync::oneshot::channel::<bool>();
        {
            let mut s = state.lock().await;
            s.modal_scroll_row = 0;
            s.pending_tool_confirmation = Some(vec![ToolConfirmation {
                tool_name: display_name.to_string(),
                path,
                content_preview: preview,
                content_bytes,
            }]);
            s.tool_confirmation_response = Some(tx);
            s.status = AppStatus::AwaitingToolConfirmation;
        }
        // Notify the user via Ghostty / iTerm2 OSC sequence that a tool needs
        // their approval. Harmless on other terminals.
        let _ = crate::notifications::notify_pending_confirmation(name);
        dbg_log!("Awaiting user confirmation for '{}'", name);
        let res = match rx.await {
            Ok(true) => {
                dbg_log!("User approved tool call '{}', executing...", name);
                let tool_name = name.to_string();
                {
                    let mut s = state.lock().await;
                    s.pending_tool_confirmation = None;
                    let model_name = s.model_name.clone();
                    s.status = AppStatus::Streaming;
                    if s.config.discord_rpc_enabled {
                        s.discord_rpc.set_activity("Streaming", &format!("Using model: {}", model_name));
                    }
                    s.stream_tracker = Some(StreamTracker::new());
                    s.running_tools.push(tool_name.clone());
                }
                let _cleanup = ToolCleanup {
                    state: Arc::clone(state),
                    tool_name,
                };

                let name_owned = name.to_string();
                let args_owned = args.clone();
                let session_id = { state.lock().await.active_session_id.clone() };
                let workspace_root_for_task = workspace_root.clone();
                let run_fut = tokio::task::spawn_blocking(move || {
                    crate::tools::set_active_session_id(Some(session_id));
                    crate::tools::set_active_workspace_root(workspace_root_for_task);
                    let result = crate::tools::execute(&name_owned, &args_owned);
                    crate::tools::set_active_workspace_root(None);
                    crate::tools::set_active_session_id(None);
                    result
                });

                tokio::select! {
                    res = run_fut => {
                        res.unwrap_or_else(|e| format!("tool panicked: {e}"))
                    }
                    _ = cancel_token.cancelled() => {
                        dbg_log!("Tool execution cancelled during spawn_blocking await");
                        "error: tool execution cancelled by user".to_string()
                    }
                }
            }
            Ok(false) => {
                dbg_log!("User denied tool call '{}'", name);
                let _ =
                    crate::notifications::notify_finished(crate::notifications::FinishedStatus::Denied);
                "error: user denied this tool call".to_string()
            }
            Err(_) => {
                dbg_log!("Confirmation channel closed for '{}'", name);
                "error: confirmation channel closed".to_string()
            }
        };
        {
            let mut s = state.lock().await;
            s.pending_tool_confirmation = None;
            let model_name = s.model_name.clone();
            s.status = AppStatus::Streaming;
            if s.config.discord_rpc_enabled {
                s.discord_rpc.set_activity("Streaming", &format!("Using model: {}", model_name));
            }
            s.stream_tracker = Some(StreamTracker::new());
        }
        res
    };

    if matches!(
        name,
        "replace_file_content"
            | "multi_replace_file_content"
            | "write_to_file"
            | "delete_file"
            | "move_file"
            | "copy_file"
    ) && !result.starts_with("error")
    {
        let cwd = get_tool_project_root(name, args);
        if let Some(errors) = run_compiler_check(&cwd).await {
            result.push_str("\n\nCompiler errors/warnings:\n");
            result.push_str(&errors);
        }
    }

    (result, diff_opt)
}

const MAX_ACTIVE_SUBAGENTS: usize = 4;

fn push_status_line(s: &mut AppState, text: String) {
    s.history.push(ChatMessage::new("system", text));
    crate::config::save_history(&s.history);
}

/// Drop a leading <think>...</think> block so the main agent only gets the
/// subagent's actual reply, not its reasoning.
/// Run one subagent conversation until it produces a plain reply (no tool
/// call). Tokens stream quietly (not into the main chat view); tool calls
/// surface as status lines and go through the same confirmation modal as
/// the main agent. Returns the subagent's final reply or an error string.
async fn run_subagent(
    client: &reqwest::Client,
    state: &Arc<Mutex<AppState>>,
    cancel_token: &tokio_util::sync::CancellationToken,
    agent_id: u32,
) -> String {
    crate::logger::operational_event(
        "subagent.start",
        serde_json::json!({"agent_id": agent_id}),
    );
    let stream_buffer = Arc::new(Mutex::new(StreamBuffer {
        content: String::new(),
    }));
    let mut rounds = 0usize;
    let mut loop_detector = loop_detect::LoopDetector::new(6);
    loop {
        if cancel_token.is_cancelled() {
            crate::logger::operational_event(
                "subagent.finish",
                serde_json::json!({"agent_id": agent_id, "status": "cancelled"}),
            );
            return "error: cancelled".to_string();
        }
        let mut history_snapshot: Vec<ChatMessage> = {
            let s = state.lock().await;
            s.subagents
                .iter()
                .find(|a| a.id == agent_id)
                .map(|a| a.history.clone())
                .unwrap_or_default()
        };
        if history_snapshot.is_empty() {
            return format!("error: no subagent with id {agent_id}");
        }

        let budget_token_limit = { state.lock().await.get_history_token_budget() };
        compact_history_to_budget(&mut history_snapshot, budget_token_limit).await;

        let protocol = { state.lock().await.config.tool_protocol };
        let agent_mode = { state.lock().await.agent_mode };
        let delegation_contract = {
            let s = state.lock().await;
            s.subagents
                .iter()
                .find(|agent| agent.id == agent_id)
                .map(|agent| {
                    format!(
                        "Delegation contract: write_access={}, allowed_paths={:?}, verification_command={:?}.",
                        agent.write_access, agent.allowed_paths, agent.verification_command
                    )
                })
                .unwrap_or_else(|| "Delegation contract unavailable; remain read-only.".to_string())
        };
        let system_prompt = format!(
            "{}\n\nYou are subagent {agent_id}, working for a main agent in the same \
rustcode session. Complete the task you were given, then reply in plain text \
with NO tool call — that reply is returned to the main agent. Keep the final \
reply compact and information-dense. {delegation_contract}\n\n{}",
            crate::tools::tool_system_prompt(false, protocol, agent_mode),
            crate::context::environment_context()
        );
        let mut msgs: Vec<serde_json::Value> = vec![serde_json::json!({
            "role": "system",
            "content": system_prompt,
        })];
        msgs.extend(history_snapshot.iter().map(|m| {
            if m.role == "tool" {
                serde_json::json!({
                    "role": "user",
                    "content": format!("<tool_result>\n{}\n</tool_result>", m.content),
                })
            } else {
                serde_json::json!({"role": m.role, "content": m.content})
            }
        }));
        let window = { state.lock().await.active_context_window() };
        let budget = window.saturating_sub(RESPONSE_RESERVE_TOKENS).max(512);
        trim_msgs_to_budget(&mut msgs, budget);
        inject_system_reminder(&mut msgs);

        stream_buffer.lock().await.content.clear();
        let (api_base_url, model_name) = {
            let s = state.lock().await;
            let subagent = s
                .subagents
                .iter()
                .find(|a| a.id == agent_id)
                .expect("Subagent not found");
            let target_model_name = subagent.model.as_deref().unwrap_or(&s.model_name);
            if let Some(profile) = s.config.models.iter().find(|p| p.name == target_model_name) {
                (profile.url.clone(), profile.model.clone())
            } else {
                (s.api_base_url.clone(), s.model_name.clone())
            }
        };
        dbg_log!(
            "subagent {} round {}: requesting {}",
            agent_id,
            rounds,
            model_name
        );
        let request_client = client.clone();
        let request_state = Arc::clone(state);
        let request_cancel = cancel_token.clone();
        let request_buffer = Arc::clone(&stream_buffer);
        let request_api_url = api_base_url.clone();
        let request_model = model_name.clone();
        let request_msgs = msgs.clone();
        let (content, _finish_reason) = match runner::collect_response(move |previous| {
            let mut current_msgs = request_msgs.clone();
            if !previous.is_empty() {
                current_msgs.push(serde_json::json!({
                    "role": "assistant",
                    "content": previous
                }));
                current_msgs.push(serde_json::json!({
                    "role": "user",
                    "content": "continue"
                }));
            }
            let request_client = request_client.clone();
            let request_state = Arc::clone(&request_state);
            let request_cancel = request_cancel.clone();
            let request_buffer = Arc::clone(&request_buffer);
            let request_api_url = request_api_url.clone();
            let request_model = request_model.clone();
            async move {
                request_buffer.lock().await.content.clear();
                let finish_reason = stream_request(
                    &request_client,
                    request_state,
                    request_cancel,
                    &request_api_url,
                    &request_model,
                    &current_msgs,
                    Arc::clone(&request_buffer),
                    true,
                )
                .await
                .map_err(|e| e.to_string())?;
                let chunk_content = request_buffer.lock().await.content.clone();
                Ok((chunk_content, finish_reason))
            }
        })
        .await
        {
            Ok(result) => result,
            Err(e) => return format!("error: subagent request failed: {e}"),
        };

        if content.is_empty() {
            return "error: subagent returned an empty reply".to_string();
        }

        let protocol = { state.lock().await.config.tool_protocol };
        if let Some(tool_call) = crate::tools::parse_tool_call(&content, protocol) {
            let name = &tool_call.name;
            let args = &tool_call.arguments;
            if let Err(reason) = crate::tools::validate_tool_calls(std::slice::from_ref(&tool_call)) {
                let mut s = state.lock().await;
                if let Some(a) = s.subagents.iter_mut().find(|a| a.id == agent_id) {
                    a.history.push(ChatMessage::new("assistant", &content));
                    a.history.push(ChatMessage::new(
                        "tool",
                        format!("{name}: error: tool call rejected before execution: {reason}"),
                    ));
                }
                continue;
            }
            let (exact, category) = loop_detect::signatures(name, args);
            if let loop_detect::LoopStatus::Abort(repeats) =
                loop_detector.check_tool(name, &exact, &category)
            {
                return format!(
                    "error: subagent {agent_id} stopped after {repeats} repeated '{name}' actions"
                );
            }
            rounds += 1;
            let (write_access, allowed_paths) = {
                let s = state.lock().await;
                s.subagents
                    .iter()
                    .find(|agent| agent.id == agent_id)
                    .map(|agent| (agent.write_access, agent.allowed_paths.clone()))
                    .unwrap_or((false, Vec::new()))
            };
            let needs_write_access = is_mutating_tool(name) || name == "run_command";
            let path_outside_contract = args
                .get("path")
                .and_then(|value| value.as_str())
                .is_some_and(|path| {
                    !allowed_paths.iter().any(|allowed| {
                        path == allowed
                            || path.starts_with(&format!("{}/", allowed.trim_end_matches('/')))
                    })
                });
            let (result, diff_opt) = if needs_write_access && !write_access {
                (
                    "error: subagents are read-only by default; request write_access with allowed_paths explicitly".to_string(),
                    None,
                )
            } else if write_access && path_outside_contract {
                (
                    "error: requested path is outside the subagent allowed_paths contract".to_string(),
                    None,
                )
            } else if crate::tools::is_agent_tool(name) {
                (
                    "error: subagents cannot spawn or message other agents".to_string(),
                    None,
                )
            } else {
                {
                    let mut s = state.lock().await;
                    let target = args
                        .get("path")
                        .or_else(|| args.get("command"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    push_status_line(&mut s, format!("agent-{agent_id} → {name} {target}"));
                }
                confirm_and_execute(
                    state,
                    cancel_token,
                    name,
                    args,
                    &format!("agent-{agent_id} · {name}"),
                    false,
                    {
                        let s = state.lock().await;
                        s.subagents
                            .iter()
                            .find(|agent| agent.id == agent_id)
                            .and_then(|agent| agent.workspace_root.clone())
                    },
                )
                .await
            };
            let mut s = state.lock().await;
            if let Some(a) = s.subagents.iter_mut().find(|a| a.id == agent_id) {
                a.history.push(ChatMessage::new("assistant", &content));
                let truncated_result = truncate_tool_output(name, result);
                a.history.push(
                    ChatMessage::new("tool", format!("{name}: {truncated_result}")).with_diff(diff_opt),
                );
            }
            continue;
        }

        let mut s = state.lock().await;
        if let Some(a) = s.subagents.iter_mut().find(|a| a.id == agent_id) {
            a.history.push(ChatMessage::new("assistant", &content));
        }
        crate::logger::operational_event(
            "subagent.finish",
            serde_json::json!({"agent_id": agent_id, "status": "completed", "rounds": rounds}),
        );
        return strip_leading_think(&content).to_string();
    }
}

async fn set_subagent_status(
    state: &Arc<Mutex<AppState>>,
    agent_id: u32,
    status: crate::app::SubAgentStatus,
) {
    let mut s = state.lock().await;
    if let Some(agent) = s.subagents.iter_mut().find(|agent| agent.id == agent_id) {
        agent.status = status;
    }
}

/// Handle spawn_agent / send_agent from the main agent: run a nested
/// subagent conversation (the main agent waits) and return the subagent's
/// reply as the tool result.
async fn handle_agent_tool(
    client: &reqwest::Client,
    state: &Arc<Mutex<AppState>>,
    cancel_token: &tokio_util::sync::CancellationToken,
    name: &str,
    args: &serde_json::Value,
) -> String {
    match name {
        "spawn_agent" => {
            if !state.lock().await.delegation_active {
                return "error: subagents are disabled for this task. Run /delegate before starting the task.".to_string();
            }
            let Some(task) = args
                .get("task")
                .and_then(|t| t.as_str())
                .filter(|t| !t.trim().is_empty())
            else {
                return "error: missing 'task' argument".to_string();
            };
            let model = args
                .get("model")
                .and_then(|m| m.as_str())
                .map(|s| s.to_string());
            let write_access = args
                .get("write_access")
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            let allowed_paths = args
                .get("allowed_paths")
                .and_then(|value| value.as_array())
                .map(|paths| {
                    paths
                        .iter()
                        .filter_map(|path| path.as_str().map(str::to_string))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if write_access && allowed_paths.is_empty() {
                return "error: write-enabled subagents require at least one allowed_paths entry".to_string();
            }
            let verification_command = args
                .get("verification_command")
                .and_then(|value| value.as_str())
                .map(str::to_string);
            let verification_label = verification_command.as_deref().unwrap_or("none").to_string();
            let agent_id = {
                let mut s = state.lock().await;
                let active_count = s
                    .subagents
                    .iter()
                    .filter(|agent| agent.status == crate::app::SubAgentStatus::Running)
                    .count();
                if active_count >= MAX_ACTIVE_SUBAGENTS {
                    return format!(
                        "error: maximum active subagents reached ({MAX_ACTIVE_SUBAGENTS}); wait for an existing agent to finish"
                    );
                }
                let id = s.next_subagent_id;
                s.next_subagent_id += 1;
                let workspace_root = if write_access {
                    match crate::config::create_subagent_workspace(&s.active_session_id, id) {
                        Ok(path) => Some(path),
                        Err(error) => return format!("error: unable to create isolated subagent workspace: {error}"),
                    }
                } else {
                    None
                };
                s.subagents.push(crate::app::SubAgent {
                    id,
                    task: task.to_string(),
                    model,
                    history: vec![ChatMessage::new("user", task)],
                    status: crate::app::SubAgentStatus::Running,
                    write_access,
                    allowed_paths,
                    verification_command,
                    workspace_root,
                    review_manifest: None,
                });
                let brief: String = task.chars().take(60).collect();
                push_status_line(
                    &mut s,
                    format!(
                        "agent-{id} spawned: {brief} (write_access={write_access}, verify={})",
                        verification_label
                    ),
                );
                id
            };
            let reply = run_subagent(client, state, cancel_token, agent_id).await;
            let review_manifest = {
                let s = state.lock().await;
                s.subagents
                    .iter()
                    .find(|agent| agent.id == agent_id)
                    .and_then(|agent| agent.workspace_root.as_ref())
                    .and_then(|workspace| crate::config::write_subagent_review_manifest(workspace, agent_id))
            };
            if let Some(manifest) = review_manifest
                && let Some(agent) = state.lock().await.subagents.iter_mut().find(|agent| agent.id == agent_id) {
                    agent.review_manifest = Some(manifest);
                }
            set_subagent_status(
                state,
                agent_id,
                if reply.starts_with("error:") {
                    crate::app::SubAgentStatus::Failed
                } else if cancel_token.is_cancelled() {
                    crate::app::SubAgentStatus::Cancelled
                } else {
                    crate::app::SubAgentStatus::Completed
                },
            )
            .await;
            push_status_line(&mut *state.lock().await, format!("agent-{agent_id} done"));
            format!("(subagent id {agent_id} — follow up with send_agent)\n{reply}")
        }
        "send_agent" => {
            if !state.lock().await.delegation_active {
                return "error: subagents are disabled for this task. Run /delegate before starting the task.".to_string();
            }
            let id = args.get("id").and_then(|v| {
                v.as_u64()
                    .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            });
            let Some(id) = id else {
                return "error: missing or invalid 'id' argument".to_string();
            };
            let id = id as u32;
            let Some(message) = args
                .get("message")
                .and_then(|m| m.as_str())
                .filter(|m| !m.trim().is_empty())
            else {
                return "error: missing 'message' argument".to_string();
            };
            {
                let mut s = state.lock().await;
                let Some(task) = s
                    .subagents
                    .iter()
                    .find(|a| a.id == id)
                    .map(|a| a.task.chars().take(40).collect::<String>())
                else {
                    let known: Vec<String> = s.subagents.iter().map(|a| a.id.to_string()).collect();
                    return if known.is_empty() {
                        "error: no subagents exist — use spawn_agent first".to_string()
                    } else {
                        format!(
                            "error: no subagent with id {id}. Known ids: {}",
                            known.join(", ")
                        )
                    };
                };
                push_status_line(&mut s, format!("agent-{id} ← follow-up ({task})"));
                if let Some(a) = s.subagents.iter_mut().find(|a| a.id == id) {
                    if a.status == crate::app::SubAgentStatus::Failed
                        || a.status == crate::app::SubAgentStatus::Cancelled
                    {
                        return format!("error: subagent {id} is not available for follow-up");
                    }
                    a.status = crate::app::SubAgentStatus::Running;
                    a.history.push(ChatMessage::new("user", message));
                }
            }
            let reply = run_subagent(client, state, cancel_token, id).await;
            set_subagent_status(
                state,
                id,
                if reply.starts_with("error:") {
                    crate::app::SubAgentStatus::Failed
                } else if cancel_token.is_cancelled() {
                    crate::app::SubAgentStatus::Cancelled
                } else {
                    crate::app::SubAgentStatus::Completed
                },
            )
            .await;
            push_status_line(&mut *state.lock().await, format!("agent-{id} done"));
            format!("(subagent id {id})\n{reply}")
        }
        "set_goal" => {
            let goal = args.get("goal").and_then(|g| g.as_str()).unwrap_or("");
            if goal.is_empty() {
                return "error: missing 'goal' argument".to_string();
            }
            let mut s = state.lock().await;
            s.continuous_mode = true;
            s.input_buffer.clear();
            s.cursor_position = 0;
            format!("Success: Goal set to '{}'. You are now in continuous autoloop mode. Continue executing tools to complete this goal, and call the 'complete_task' tool when fully done.", goal)
        }
        "todo_write" => {
            let Some(arr) = args.get("todos").and_then(|t| t.as_array()) else {
                return "error: missing 'todos' array argument".to_string();
            };
            let mut todos = Vec::with_capacity(arr.len());
            for item in arr {
                let Some(content) = item
                    .get("content")
                    .and_then(|c| c.as_str())
                    .filter(|c| !c.trim().is_empty())
                else {
                    return "error: each todo needs a non-empty 'content'".to_string();
                };
                let status = item
                    .get("status")
                    .and_then(|s| s.as_str())
                    .unwrap_or("pending")
                    .to_string();
                let priority = item
                    .get("priority")
                    .and_then(|s| s.as_str())
                    .unwrap_or("medium")
                    .to_string();
                todos.push(crate::app::TodoItem {
                    content: content.to_string(),
                    status,
                    priority,
                });
            }
            let summary = format!(
                "Plan updated ({} item(s)): {}",
                todos.len(),
                todos
                    .iter()
                    .map(|t| format!("[{}] {}", t.status, t.content))
                    .collect::<Vec<_>>()
                    .join(" | ")
            );
            let mut s = state.lock().await;
            s.todos = todos;
            drop(s);
            summary
        }
        _ => format!("error: unknown agent tool '{name}'"),
    }
}

/// Generate a title from the first user message using the small model.
/// Returns None if the message starts with '/' (slash command).
pub async fn generate_title(
    client: &reqwest::Client,
    config: &crate::config::AppConfig,
    first_message: &str,
) -> Option<String> {
    if first_message.trim().starts_with('/') {
        return None;
    }

    let small_model_name = config.default.small();
    let (url, model) = crate::config::resolve_model_endpoint(config, small_model_name);

    let first_line = first_message.lines().next()?;
    let prompt = format!(
        "Generate a short, concise title (max 5 words) summarizing this user's coding request/intent. Do not use quotes, punctuation, or any introductory text. Return only the title itself.\n\nIntent: {}",
        first_line.trim()
    );

    let messages = vec![serde_json::json!({
        "role": "user",
        "content": prompt
    })];

    let payload = serde_json::json!({
        "model": model,
        "messages": messages,
        "max_tokens": 30,
        "temperature": 0.3,
    });

    let res = client
        .post(&url)
        .json(&payload)
        .send()
        .await
        .ok()?;

    if !res.status().is_success() {
        return None;
    }

    let json: serde_json::Value = res.json().await.ok()?;
    let title = json.get("choices")?
        .get(0)?
        .get("message")?
        .get("content")?
        .as_str()?;

    let cleaned_title = title.trim().trim_matches('"').trim().to_string();
    if cleaned_title.is_empty() {
        None
    } else {
        Some(cleaned_title)
    }
}


/// Push the incoming prompt (user message, or a background-task wakeup system
/// note) onto history, persist it, and reset the per-response scratch fields.
async fn record_prompt_to_history(
    state: &Arc<Mutex<AppState>>,
    is_wakeup: bool,
    next_prompt: &str,
) {
    let mut s = state.lock().await;
    if is_wakeup {
        let task_id = next_prompt.strip_prefix("__task_wakeup__:").unwrap_or("");
        s.history.push(ChatMessage::new(
            "system",
            format!("Task {task_id} has finished running in the background."),
        ));
    } else {
        s.history.push(ChatMessage::new("user", next_prompt.to_string()));
    }
    let active_id = s.active_session_id.clone();
    crate::config::save_session_history(&active_id, &s.history);
    s.current_response.clear();
    s.current_token_usage = None;
    s.response_time = None;
}

/// Fire-and-forget: generate a session title from the first user message.
async fn spawn_title_generation(
    client: &reqwest::Client,
    state: &Arc<Mutex<AppState>>,
    first_msg: String,
) {
    let client_clone = client.clone();
    // Captured before spawn so title reflects the session as of this prompt.
    let (config_clone, session_id) = {
        let s = state.lock().await;
        (s.config.clone(), s.active_session_id.clone())
    };
    let state_clone = Arc::clone(state);
    tokio::spawn(async move {
        if let Some(title) = generate_title(&client_clone, &config_clone, &first_msg).await {
            crate::config::save_session_title(&session_id, &title);
            let mut s = state_clone.lock().await;
            s.invalidate_session_title_cache();
            s.request_redraw();
        }
    });
}

#[allow(unused_assignments)]
/// Assemble the turn-varying context tail appended to the last message. Kept
/// separate from the static system prefix so the provider prompt cache stays
/// warm: this lists the files already in context (so the agent doesn't re-read
/// them) and re-injects the persistent task plan so work continues across turns
/// instead of re-planning from scratch.
/// Render the volatile runtime block — the "cache divider" that must sit at the
/// very end of the request payload, after the static (cacheable) prefix and the
/// conversation. Everything here changes turn-to-turn (clock, cwd, quota), so
/// keeping it strictly at the tail lets the provider's implicit prefix cache
/// cover the entire static prefix plus the stable conversation history.
fn build_volatile_context_block(
    token_usage: Option<&crate::app::TokenUsage>,
    quota_remaining: Option<f32>,
    context_window: u32,
) -> String {
    let now = chrono::Local::now();
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "(unknown)".to_string());
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "(unknown)".to_string());

    let mut b = String::from("# Runtime Context (volatile — do not rely on this being cached)\n");
    b.push_str(&format!(
        "- Current date/time: {}\n",
        now.format("%A %Y-%m-%d %H:%M:%S %Z")
    ));
    b.push_str(&format!("- Working directory: {cwd}\n"));
    b.push_str(&format!(
        "- Platform: {} {}\n",
        std::env::consts::OS,
        std::env::consts::ARCH
    ));
    b.push_str(&format!("- Shell: {shell}\n"));
    b.push_str(&format!("- Context window: {context_window} tokens\n"));
    if let Some(u) = token_usage {
        b.push_str(&format!(
            "- Last-turn token usage: prompt {} / completion {} / total {}",
            u.prompt_tokens, u.completion_tokens, u.total_tokens
        ));
        if let Some(cached) = u.cached_tokens {
            b.push_str(&format!(" (cached {cached})"));
        }
        b.push('\n');
    }
    if let Some(q) = quota_remaining {
        b.push_str(&format!("- Model quota remaining: {q:.1}%\n"));
    }
    b
}

fn build_dynamic_context_tail(
    context_section: String,
    read_files: &[String],
    todos: &[crate::app::TodoItem],
) -> String {
    let mut fragments = vec![history::ContextFragment::new("environment", context_section)];

    if !read_files.is_empty() {
        fragments.push(history::ContextFragment::new(
            "files",
            format!(
                "# Files already in context (do NOT re-read these unless they changed on disk)\n{}",
            read_files
                .iter()
                .map(|f| format!("- {f}"))
                .collect::<Vec<_>>()
                .join("\n")
            ),
        ));
    }

    if !todos.is_empty() {
        let mut plan = String::from("# Your current task plan (execute in order; update via todo_write)\n");
        for (i, t) in todos.iter().enumerate() {
            let mark = match t.status.as_str() {
                "completed" => "[x]",
                "in_progress" => "[~]",
                _ => "[ ]",
            };
            plan.push_str(&format!(
                "{}. {} {} ({})\n",
                i + 1,
                mark,
                t.content,
                t.priority
            ));
        }
        fragments.push(history::ContextFragment::new("task plan", plan));
    }

    history::render_context_fragments(&fragments)
}

/// Cheap identity fingerprint for a history message, used to tell whether the
/// prefix we snapshotted is still the same prefix after a lock has been released
/// and re-acquired. `ChatMessage` has no `PartialEq`, and hashing role +
/// timestamp + content is enough to catch a rewritten or replaced entry without
/// cloning the (potentially large) content.
fn message_identity(m: &ChatMessage) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    m.role.hash(&mut hasher);
    m.timestamp.hash(&mut hasher);
    m.content.hash(&mut hasher);
    hasher.finish()
}

/// Assemble the full provider request for one agent turn.
///
/// Runs AI compaction if the history is long enough, snapshots the eligible
/// history, then builds the message array: a STATIC system prefix (tool
/// protocol + agent mode only, so the provider prompt cache stays warm) plus
/// the conversation, with all turn-varying context (environment delta,
/// files-in-context, task plan) appended to the last message. Finally trims to
/// the context-window budget and injects the system reminder. `tool_rounds` is
/// only used to decide whether a one-time "context window full" notice is shown.
async fn prepare_turn_request(
    client: &reqwest::Client,
    state: &Arc<Mutex<AppState>>,
    tool_rounds: usize,
) -> Vec<serde_json::Value> {
    // Try AI-driven compaction if history is long enough.
    //
    // The summarizer is a network round-trip, so the AppState mutex must NOT be
    // held while it runs: the TUI draw loop locks the same mutex every frame and
    // would freeze for the whole call. Instead we take a snapshot of the history
    // under a short lock, compact the owned copy with the lock released, then
    // re-acquire and merge the result back in.
    {
        let (api_url, model_name, budget, mut working_history) = {
            let s = state.lock().await;
            (
                s.api_base_url.clone(),
                s.model_name.clone(),
                s.get_history_token_budget() as usize,
                s.history.clone(),
            )
        };
        let pre_len = working_history.len();
        let pre_identity: Vec<u64> = working_history.iter().map(message_identity).collect();

        // Lock released here: this await performs I/O.
        let compacted =
            compaction::maybe_compact(client, &api_url, &model_name, &mut working_history, budget)
                .await;

        // Merge policy for history appended while the lock was down (a tool
        // result or a user message can land mid-compaction):
        //
        //  - unchanged history  -> write the compacted copy back wholesale;
        //  - history grew, and the prefix we compacted is still intact -> keep
        //    the compacted prefix and re-append the new tail verbatim. Nothing
        //    that arrived during the call is lost;
        //  - anything else (history shrank or was rewritten underneath us, e.g.
        //    /new, /compact, a rollback) -> discard our copy entirely and log
        //    the miss. Compaction is best-effort and will retry on the next
        //    turn; clobbering the live history is not an acceptable trade.
        //
        // Note that `maybe_compact` also performs local tool-output pruning even
        // when it returns false, so the write-back is attempted regardless of
        // the return value; the flag only gates the cache invalidation below.
        let mut s = state.lock().await;
        let prefix_intact = s.history.len() >= pre_len
            && s.history
                .iter()
                .take(pre_len)
                .map(message_identity)
                .eq(pre_identity.iter().copied());
        if prefix_intact {
            if s.history.len() > pre_len {
                working_history.extend(s.history.drain(pre_len..));
            }
            s.history = working_history;
            if compacted {
                dbg_log!("History compacted via AI summarization. Clearing read/dedup cache.");
                s.recent_read_calls.clear();
                s.read_file_mtimes.clear();
                crate::config::save_history(&s.history);
            }
        } else {
            dbg_log!(
                "Skipping compaction write-back: history changed underneath the summarizer ({} messages before, {} now). Live history kept as-is.",
                pre_len,
                s.history.len()
            );
        }
        drop(s);
    }

    // Everything the request needs from AppState is read in one guarded block so
    // the lock is taken a couple of times instead of once per field. The
    // environment snapshot is captured first because it touches the filesystem.
    let current_snapshot = crate::context::ContextSnapshot::capture();
    let (
        mut history_snapshot,
        budget_token_limit,
        read_files,
        todos,
        volatile_usage,
        volatile_quota,
        volatile_window,
        context_section,
        system_prompt,
    ) = {
        let mut s = state.lock().await;
        let history_snapshot: Vec<ChatMessage> = s
            .history
            .iter()
            .filter(|m| {
                matches!(m.role.as_str(), "user" | "assistant" | "tool")
                    && !m.content.starts_with('/')
            })
            .cloned()
            .collect();
        let budget_token_limit = s.get_history_token_budget();
        let mut read_files: Vec<String> = s.read_file_mtimes.keys().cloned().collect();
        read_files.sort();
        let todos = s.todos.clone();
        let volatile_usage = s.current_token_usage.clone();
        let volatile_quota = s.model_quota_remaining;
        let volatile_window = s.active_context_window();
        let context_section = match &s.context_snapshot {
            Some(prev) => prev.diff(&current_snapshot).unwrap_or_else(|| {
                "# Environment\n(unchanged since session start)".to_string()
            }),
            None => crate::context::environment_context(),
        };
        let protocol = s.config.tool_protocol;
        let agent_mode = s.agent_mode;
        let delegation_active = s.delegation_active;
        let system_prompt = s
            .prompt_cache
            .system_prompt(delegation_active, protocol, agent_mode)
            .to_string();
        // Store the snapshot if this is the first turn.
        if s.context_snapshot.is_none() {
            s.context_snapshot = Some(current_snapshot);
        }
        (
            history_snapshot,
            budget_token_limit,
            read_files,
            todos,
            volatile_usage,
            volatile_quota,
            volatile_window,
            context_section,
            system_prompt,
        )
    };

    compact_history_to_budget(&mut history_snapshot, budget_token_limit).await;

    // The system prompt is kept STATIC across turns (it only depends on the
    // tool protocol and agent mode, which don't change mid-task). A stable
    // prefix lets the provider's automatic prompt cache stay warm — every
    // round after the first re-bills only the dynamic tail below instead of
    // the whole tool-definition block. Turn-varying context (environment
    // delta, files-in-context, task plan) is appended to the LAST message
    // instead, so it never invalidates the cached prefix.
    //
    // The static system prompt is served from AppState's PromptCache: it's only
    // rebuilt (skill scan + MCP schema serialization) when the protocol, agent
    // mode, or MCP tool set changes, not on every turn. It is read in the
    // grouped state block above along with the rest of the turn inputs.
    //
    // Build the turn-varying context tail (appended to the last message
    // after the history is assembled, to preserve the cached prefix). The
    // volatile runtime block (clock/cwd/quota) goes last, as the explicit cache
    // divider at the very end of the payload.
    let mut dynamic_context = build_dynamic_context_tail(context_section, &read_files, &todos);
    let volatile_block = build_volatile_context_block(
        volatile_usage.as_ref(),
        volatile_quota,
        volatile_window,
    );
    if !dynamic_context.is_empty() {
        dynamic_context.push_str("\n\n");
    }
    dynamic_context.push_str(&volatile_block);
    let mut msgs = history::to_messages(&history_snapshot, system_prompt.clone());

    // Attach turn-varying context to the tail so the static system prefix
    // stays cache-stable. Done before budget trimming so its size counts
    // toward the budget.
    append_to_last_message(&mut msgs, &dynamic_context);

    let budget = volatile_window
        .saturating_sub(RESPONSE_RESERVE_TOKENS)
        .max(512);
    let dropped = trim_msgs_to_budget(&mut msgs, budget);
    inject_system_reminder(&mut msgs);
    if dropped > 0 {
        dbg_log!(
            "context budget {} tokens exceeded: dropped {} oldest message(s)",
            budget,
            dropped
        );
        if tool_rounds == 0 {
            let mut s = state.lock().await;
            s.history.push(ChatMessage::new(
                "system",
                format!(
                    "context window full: dropped {} oldest message(s) from the request. Use /new to start fresh.",
                    dropped
                ),
            ));
        }
    }

    msgs
}

/// Execute a batch of tool calls and return `(name, result, diff)` per call.
///
/// When `approved` is false every call resolves to a denial message. Otherwise
/// calls execute in model order. Read-only calls could be parallelized safely,
/// but preserving one ordering rule for every batch prevents edits, commands,
/// and reads from racing each other or hiding dependencies. If any mutating
/// tool ran, a single cached compiler check is appended to the first mutating
/// tool's result so build errors surface inline.
fn stable_arguments_hash(arguments: &serde_json::Value) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    arguments.to_string().hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn extract_exit_code(content: &str) -> Option<i32> {
    let marker = "exit code";
    let start = content.find(marker)? + marker.len();
    let suffix = content[start..].trim_start_matches([':', ' ', '=']);
    let digits: String = suffix
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '-')
        .collect();
    digits.parse().ok()
}

#[allow(clippy::too_many_arguments)]
async fn execute_tool_batch(
    client: &reqwest::Client,
    state: &Arc<Mutex<AppState>>,
    cancel_token: &tokio_util::sync::CancellationToken,
    tool_calls: &[crate::tools::ToolCall],
    approved: bool,
    made_edits: bool,
    edit_root: &Option<std::path::PathBuf>,
    compile_dirty: &mut bool,
    compile_cache: &mut Option<(std::path::PathBuf, Option<String>)>,
) -> Vec<ToolResult> {
    if !approved {
        return tool_calls
            .iter()
            .map(|call| {
                ToolResult {
                    tool_name: call.name.clone(),
                    content: "error: user denied this tool call".to_string(),
                    diff: None,
                    file_preview: None,
                    metadata: ToolResultMetadata {
                        success: false,
                        ..Default::default()
                    },
                }
            })
            .collect::<Vec<_>>();
    }

    // Independent reads may run concurrently. The recursive single-call path
    // keeps all existing repeat detection, cancellation, and result shaping in
    // one place; `join_all` preserves input order for deterministic history.
    if !made_edits
        && tool_calls.len() > 1
        && tool_calls
            .iter()
            .all(|call| crate::tools::supports_parallel_execution(&call.name))
    {
        let futures = tool_calls.iter().map(|call| async {
            let mut read_dirty = false;
            let mut read_cache = None;
            execute_tool_batch(
                client,
                state,
                cancel_token,
                std::slice::from_ref(call),
                approved,
                false,
                &None,
                &mut read_dirty,
                &mut read_cache,
            )
            .await
        });
        return futures_util::future::join_all(futures)
            .await
            .into_iter()
            .flatten()
            .collect();
    }

    dbg_log!("Executing {} tool calls sequentially", tool_calls.len());
    let mut results = Vec::with_capacity(tool_calls.len());
    for call in tool_calls {
        let name = &call.name;
        let args = &call.arguments;
        let client_clone = client.clone();
        let state_clone = Arc::clone(state);
        let cancel_token_clone = cancel_token.clone();
        let name_clone = name.clone();
        let args_clone = args.clone();
        let plan_mode_denied = {
            let plan_mode = state.lock().await.agent_mode == crate::config::AgentMode::Plan;
            plan_mode && !crate::tools::allowed_in_plan_mode(name)
        };
        let (executed_name, result, diff_opt) = async move {
            let is_read_only = is_read_only_tool(&name_clone);

            // Repeat-loop guard for read-only tools. For view_file we go
            // further than a signature match: a re-read is only blocked when
            // the file is UNCHANGED on disk since the last read, so the agent
            // can always refresh after a (possibly partial) edit. Other
            // read-only tools use a signature window.
            let mut is_repeat = false;
            let mut view_path: Option<String> = None;
            let mut view_mtime: Option<std::time::SystemTime> = None;

            if is_read_only {
                if name_clone == "view_file" {
                    if let Some(p) = args_clone.get("path").and_then(|p| p.as_str()) {
                        let sig = tool_signature(&name_clone, &args_clone);
                        let already_seen = {
                            let s = state_clone.lock().await;
                            s.recent_read_calls.iter().any(|c| c == &sig)
                        };
                        if already_seen {
                            let current = path_mtime(p);
                            let stored = {
                                let s = state_clone.lock().await;
                                s.read_file_mtimes.get(p).copied()
                            };
                            is_repeat = view_file_unchanged_since_last_read(stored, current);
                        }
                        view_path = Some(p.to_string());
                        view_mtime = path_mtime(p);
                    }
                } else {
                    let sig = tool_signature(&name_clone, &args_clone);
                    is_repeat = {
                        let s = state_clone.lock().await;
                        s.recent_read_calls.iter().any(|c| c == &sig)
                    };
                }
            }

            let (result, diff_opt) = if is_repeat {
                (
                    "[Notice: This exact read tool call was previously executed with identical arguments. \
                     The previous output is available in the context above. If you need updated lines or \
                     fresh content, use different range arguments or make edits first.]"
                        .to_string(),
                    None,
                )
            } else if name_clone == "ask_question" {
                (
                    ask_user_question(&state_clone, &cancel_token_clone, &args_clone).await,
                    None,
                )
            } else if plan_mode_denied {
                (
                    "error: Plan mode is active; this tool is not permitted.".to_string(),
                    None,
                )
            } else if crate::tools::is_agent_tool(&name_clone) {
                (
                    handle_agent_tool(
                        &client_clone,
                        &state_clone,
                        &cancel_token_clone,
                        &name_clone,
                        &args_clone,
                    )
                    .await,
                    None,
                )
            } else {
                confirm_and_execute(
                    &state_clone,
                    &cancel_token_clone,
                    &name_clone,
                    &args_clone,
                    &name_clone,
                    true, // bypass confirmation
                    None,
                )
                .await
            };

            // Record this call so future identical read-only calls are caught.
            {
                let mut s = state_clone.lock().await;
                if let Some(p) = view_path
                    && !is_repeat
                {
                    if let Some(mt) = view_mtime {
                        s.read_file_mtimes.insert(p, mt);
                    } else {
                        // File couldn't be stat'd (e.g. already gone);
                        // drop any stale entry so a later read is allowed.
                        s.read_file_mtimes.remove(&p);
                    }
                }
                if is_read_only && !is_repeat {
                    let sig = tool_signature(&name_clone, &args_clone);
                    if !s.recent_read_calls.contains(&sig) {
                        s.recent_read_calls.push_back(sig);
                        while s.recent_read_calls.len() > 8 {
                            s.recent_read_calls.pop_front();
                        }
                    }
                }
            }

            (name_clone, result, diff_opt)
        }
        .await;
        let content = truncate_tool_output(&executed_name, result);
        let changed_paths = if is_mutating_tool(&executed_name) {
            args.get("path")
                .and_then(|value| value.as_str())
                .map(|path| vec![path.to_string()])
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let full_output_artifact = content
            .lines()
            .find_map(|line| line.trim().strip_prefix("Full output saved to: "))
            .map(str::to_string);
        let exit_code = extract_exit_code(&content);
        let truncated = content.contains("[Output truncated:");
        let success = !content
            .trim_start()
            .to_ascii_lowercase()
            .starts_with("error");
        results.push(ToolResult {
            tool_name: executed_name.clone(),
            content,
            diff: diff_opt,
            file_preview: get_file_preview(&executed_name, args),
            metadata: ToolResultMetadata {
                call_id: None,
                arguments_hash: stable_arguments_hash(args),
                success,
                exit_code,
                changed_paths,
                truncated,
                full_output_artifact,
            },
        });
        if cancel_token.is_cancelled() {
            break;
        }
    }
    if made_edits {
        {
            let mut s = state.lock().await;
            s.recent_read_calls.clear();
            s.read_file_mtimes.clear();
        }
        let root = edit_root
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        if let Some(compiler_errors) =
            cached_compiler_check(&root, compile_dirty, compile_cache).await
            .filter(|e| !e.starts_with("__BUILD_UNVERIFIED__"))
        {
            dbg_log!("Inline compiler check detected errors after edit");
            let mut snippet = compiler_errors;
            if snippet.len() > 3000 {
                snippet.truncate(3000);
                snippet.push_str("\n... (compiler output truncated) ...");
            }
            if let Some(result) = results
                .iter_mut()
                .find(|result| is_mutating_tool(&result.tool_name))
            {
                result
                    .content
                    .push_str("\n\nLSP/Compiler errors detected in workspace, please fix:\n");
                result.content.push_str(&snippet);
            }
        }
    }
    results
}

pub struct TurnContext {
    pub tool_rounds: usize,
    pub loop_detector: loop_detect::LoopDetector,
    pub force_final: bool,
    pub made_edits: bool,
    pub edit_root: Option<std::path::PathBuf>,
    pub compile_dirty: bool,
    pub compile_cache: Option<(std::path::PathBuf, Option<String>)>,
    pub finish_gate_retries: u32,
    pub turn_machine: events::TurnMachine,
    pub last_sent_messages: Vec<serde_json::Value>,
    pub final_content: String,
    pub task_completed: bool,
}

impl TurnContext {
    pub fn new() -> Self {
        Self {
            tool_rounds: 0,
            loop_detector: loop_detect::LoopDetector::new(6),
            force_final: false,
            made_edits: false,
            edit_root: None,
            compile_dirty: true,
            compile_cache: None,
            finish_gate_retries: 0,
            turn_machine: events::TurnMachine::new(),
            last_sent_messages: Vec::new(),
            final_content: String::new(),
            task_completed: false,
        }
    }
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

            let msgs = prepare_turn_request(client, state, ctx.tool_rounds).await;

            state.lock().await.current_response.clear();
            stream_buffer.lock().await.content.clear();

            let (api_base_url, model_name) = {
                let s = state.lock().await;
                (s.api_base_url.clone(), s.model_name.clone())
            };

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
            let (accumulated_content, response_finish_reason) = match runner::collect_response(move |previous| {
                let mut current_msgs = request_msgs.clone();
                if !previous.is_empty() {
                    current_msgs.push(serde_json::json!({
                        "role": "assistant",
                        "content": previous
                    }));
                    current_msgs.push(serde_json::json!({
                        "role": "user",
                        "content": "continue"
                    }));
                }
                let request_client = request_client.clone();
                let request_state = Arc::clone(&request_state);
                let request_cancel = request_cancel.clone();
                let request_buffer = Arc::clone(&request_buffer);
                let request_api_url = request_api_url.clone();
                let request_model = request_model.clone();
                async move {
                    request_buffer.lock().await.content.clear();
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
                    let chunk_content = request_buffer.lock().await.content.clone();
                    Ok((chunk_content, finish_reason))
                }
            })
            .await
            {
                Ok(result) => result,
                Err(e) => {
                    ctx.turn_machine.recover_error();
                    dbg_log!("Stream request failed: {}", e);
                    let mut s = state.lock().await;
                    s.history.push(ChatMessage::new(
                        "system",
                        format!("Error from LLM Provider: {e}"),
                    ));
                    crate::config::save_history(&s.history);
                    s.current_response.clear();
                    s.current_token_usage = None;
                    s.status = AppStatus::Idle;
                    return false;
                }
            };

            crate::logger::operational_event(
                "model.response",
                serde_json::json!({
                    "round": ctx.tool_rounds,
                    "finish_reason": response_finish_reason,
                    "content_bytes": accumulated_content.len(),
                }),
            );
            {
                let mut s = state.lock().await;
                s.current_response = accumulated_content.clone();
            }

            if cancel_token.is_cancelled() {
                ctx.turn_machine.cancel();
                return false;
            }

            ctx.final_content = accumulated_content;
            dbg_log!(
                "Stream completed successfully. Content length: {} chars",
                ctx.final_content.len()
            );

            if ctx.final_content.is_empty() {
                dbg_log!("Stream returned empty content, finishing");
                let mut s = state.lock().await;
                s.status = AppStatus::Idle;
                s.current_token_usage = None;
                return false;
            }

            // This is the forced wrap-up turn after a detected loop: tools were
            // disabled via the injected directive. Push whatever prose the model
            // produced and stop — never parse or execute tool calls here, or we'd
            // risk re-entering the loop we just broke out of.
            if ctx.force_final {
                dbg_log!("Loop wrap-up: recording forced text answer and finishing");
                let prose = strip_tool_call_syntax(&ctx.final_content);
                // Filter out any system prompt leak or empty content
                let clean_prose = prose
                    .lines()
                    .filter(|line| !line.trim().starts_with("- ") && !line.contains("system directive") && !line.contains("CRITICAL — you are stuck"))
                    .collect::<Vec<_>>()
                    .join("\n");
                let answer = if clean_prose.trim().is_empty() {
                    "I encountered a repeating loop while running tool actions and have stopped to prevent unnecessary repetition. I was unable to complete the task automatically. Please check the current changes or re-run with a more specific prompt."
                        .to_string()
                } else {
                    clean_prose.trim().to_string()
                };
                let mut s = state.lock().await;
                s.history.push(ChatMessage::new("assistant", &answer));
                crate::config::save_history(&s.history);
                s.current_response.clear();
                s.continuous_mode = false;
                s.status = AppStatus::Idle;
                return false;
            }

            let protocol = { state.lock().await.config.tool_protocol };
            let model_response = events::normalize_response(
                &ctx.final_content,
                response_finish_reason.as_deref(),
                protocol,
            );
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
            if let Err(reason) = crate::tools::validate_tool_calls(&parsed_tool_calls) {
                dbg_log!("Tool-call validation rejected response: {}", reason);
                let mut s = state.lock().await;
                s.history.push(ChatMessage::new("assistant", &ctx.final_content));
                s.history.push(ChatMessage::new(
                    "system",
                    format!(
                        "[Tool call rejected before execution: {reason}] Preserve the raw assistant response above for diagnostics, then emit one corrected tool call."
                    ),
                ));
                crate::config::save_history(&s.history);
                s.current_response.clear();
                s.status = AppStatus::Streaming;
                drop(s);
                ctx.tool_rounds += 1;
                return true;
            }
            let (tool_calls, deferred_tool_calls) =
                crate::tools::isolate_control_plane_call(parsed_tool_calls);
            let turn_action = match ctx.turn_machine.model_finished(
                cancel_token.is_cancelled(),
                ctx.force_final,
                !tool_calls.is_empty(),
                ctx.task_completed,
            ) {
                Ok(action) => action,
                Err(invalid) => {
                    // An illegal internal transition is a bug, not a user error.
                    // Debug builds already asserted inside the machine; in
                    // release, log it and finish the turn defensively rather
                    // than executing tools from an unexpected state.
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
                dbg_log!("Parsed {} tool call requests", tool_calls.len());

                // Loop detection: feed each requested call to the detector and
                // keep the worst status. Abort stops auto-execution; Warning
                // injects a nudge so the model changes approach.
                let mut loop_status = loop_detect::LoopStatus::Ok;
                for call in &tool_calls {
                    let (exact, category) = loop_detect::signatures(&call.name, &call.arguments);
                    let s = ctx.loop_detector.check_tool(&call.name, &exact, &category);
                    if s.rank() > loop_status.rank() {
                        loop_status = s;
                    }
                    // Remember that code was touched, and where, so the finish
                    // gate can compile-check before accepting a "done".
                    if is_mutating_tool(&call.name) {
                        ctx.made_edits = true;
                        ctx.edit_root = Some(get_tool_project_root(&call.name, &call.arguments));
                        // A mutating tool will run this round — invalidate the
                        // cached compiler result so the next check recompiles.
                        ctx.compile_dirty = true;
                    }
                }
                match loop_status {
                    loop_detect::LoopStatus::Abort(n) => {
                        dbg_log!("Loop detector: abort after {} repeats — forcing wrap-up turn", n);
                        // Don't stop silently. Record the looping turn, then inject
                        // a directive that disables tools and demands a prose
                        // summary, and run exactly one more turn (`ctx.force_final`).
                        let mut s = state.lock().await;
                        s.history
                            .push(ChatMessage::new("assistant", &ctx.final_content));
                        s.history
                            .push(ChatMessage::new("system", FORCE_ANSWER_PROMPT));
                        crate::config::save_history(&s.history);
                        s.current_response.clear();
                        drop(s);
                        ctx.force_final = true;
                        return true;
                    }
                    loop_detect::LoopStatus::Warning(n) => {
                        dbg_log!("Loop detector: warning at {} repeats", n);
                        let mut s = state.lock().await;
                        s.history.push(ChatMessage::new(
                            "system",
                            format!(
                                "[Loop warning: this action has repeated {n} times. If a tool edit or view is failing, stop retrying the same inputs — call view_file to check exact line numbers or change your approach.]"
                            ),
                        ));
                        drop(s);
                    }
                    loop_detect::LoopStatus::Ok => {}
                }

                if !cancel_token.is_cancelled() {
                    ctx.tool_rounds += 1;

                    let approved = policy.should_approve(state, &tool_calls).await;

                    // Update UI state immediately after confirmation is resolved
                    {
                        let mut s = state.lock().await;
                        s.pending_tool_confirmation = None;
                        s.status = AppStatus::Streaming;
                        s.stream_tracker = Some(StreamTracker::new());
                        s.history
                            .push(ChatMessage::new("assistant", &ctx.final_content));
                        crate::config::save_history(&s.history);
                    }

                    // Approval must be resolved on the machine BEFORE any tool
                    // runs: execution is gated on the machine reaching
                    // ExecutingTools, so a denial leaves it in AwaitingModel and
                    // nothing executes.
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

                    let results = execute_tool_batch(
                        client,
                        state,
                        cancel_token,
                        &tool_calls,
                        ctx.turn_machine.state() == events::TurnState::ExecutingTools,
                        ctx.made_edits,
                        &ctx.edit_root,
                        &mut ctx.compile_dirty,
                        &mut ctx.compile_cache,
                    )
                    .await;

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
                        let mut s = state.lock().await;
                        s.status = AppStatus::Idle;
                        ctx.turn_machine.finish_tools_if_executing();
                        return false;
                    }

                    let mut s = state.lock().await;
                    s.status = AppStatus::Streaming;
                    let mut completed = false;
                    for result in results {
                        let name = result.tool_name;
                        let metadata = result.metadata.clone();
                        let mut content = result.content;
                        if name == "use_skill" && deferred_tool_calls > 0 {
                            content.push_str(&format!(
                                "\n\n[harness: deferred {deferred_tool_calls} additional tool call(s) until the next model turn after skill loading]"
                            ));
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
                        // Progress resets the loop detector: a successful mutating
                        // tool means the agent moved the work forward, so any
                        // re-reads that follow (to verify or find the next anchor)
                        // shouldn't inherit the pre-edit read history and trip the
                        // frequency signal. Failed edits (result starts with
                        // "error") are not progress and must keep accumulating.
                        if is_mutating_tool(&name) && !content.trim_start().to_ascii_lowercase().starts_with("error")
                        {
                            ctx.loop_detector.reset();
                        }
                        // Output-stagnation signal: repeated identical results
                        // (e.g. "No matches found") despite varied commands.
                        if let loop_detect::LoopStatus::Warning(n) | loop_detect::LoopStatus::Abort(n) =
                            ctx.loop_detector.record_output(&content)
                        {
                            dbg_log!("Loop detector: output stagnation x{} for '{}'", n, name);
                        }
                        let truncated_result = truncate_tool_output(&name, content);
                        s.history.push(
                            ChatMessage::new("tool", format!("{name}: {truncated_result}"))
                                .with_diff(diff_opt)
                                .with_file_preview(result.file_preview)
                                .with_tool_result(crate::app::ToolResultRecord {
                                    tool_name: name.clone(),
                                    arguments_hash: metadata.arguments_hash,
                                    success: metadata.success,
                                    exit_code: metadata.exit_code,
                                    changed_paths: metadata.changed_paths,
                                    truncated: metadata.truncated,
                                    full_output_artifact: metadata.full_output_artifact,
                                }),
                        );
                    }
                    if completed {
                        let mut build_status = if ctx.made_edits {
                            "pending"
                        } else {
                            "not run (no workspace edits detected)"
                        };
                        // Finish gate check: verify the project builds cleanly before accepting completion
                        if ctx.made_edits {
                            let root = ctx.edit_root
                                .clone()
                                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
                            if let Some(errors) = cached_compiler_check(&root, &mut ctx.compile_dirty, &mut ctx.compile_cache).await {
                                if errors.starts_with("__BUILD_UNVERIFIED__") {
                                    // The checker itself couldn't run (missing toolchain,
                                    // timeout, …). We can't prove the build is red, so we
                                    // don't loop forever — but we must NOT let the agent
                                    // report a clean completion, so surface it loudly.
                                    dbg_log!("complete_task finish gate: build unverified — {errors}");
                                    build_status = "unverified";
                                    s.history.push(ChatMessage::new(
                                        "system",
                                        format!("[⚠ Build could not be verified — {errors}]"),
                                    ));
                                } else {
                                    dbg_log!("complete_task finish gate failed with compiler errors");
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
                            }
                        }

                        dbg_log!("complete_task called, turning off continuous mode and breaking loop immediately");
                        s.continuous_mode = false;
                        s.status = AppStatus::Idle;
                        // Extract task result text from the complete_task call
                        let task_result_summary = tool_calls
                            .iter()
                            .find(|call| call.name == "complete_task")
                            .and_then(|call| call.arguments.get("result").and_then(|r| r.as_str()))
                            .map(|s| s.to_string());

                        if let Some(mut summary_text) = task_result_summary
                            && !summary_text.is_empty() {
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
                                    "\n\n[harness verification: build={build_status}; changed_paths={paths}]"
                                ));
                                s.history.push(ChatMessage::new("assistant", summary_text));
                            }
                        crate::config::save_history(&s.history);
                        s.current_response.clear();
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
                dbg_log!(
                    "Orchestrator: Detected malformed tool call, auto-correcting and retrying..."
                );
                ctx.tool_rounds += 1;
                let mut s = state.lock().await;
                s.history
                    .push(ChatMessage::new("assistant", &ctx.final_content));

                let reason = crate::tools::diagnose_failed_tool_call(&ctx.final_content)
                    .map(|r| format!("{r}\n\n"))
                    .unwrap_or_default();
                let feedback = format!(
                    "tool_error: The tool call block was malformed or could not be parsed. {reason}\
Please output a single, complete, valid tool call block inside a ```tool fenced block using JSON format:\n\n\
```tool\n\
{{\"name\": \"tool_name\", \"arguments\": {{...}}}}\n\
```\n\n\
Make sure keys are exactly \"name\" and \"arguments\", and do not wrap numbers/booleans in quotes if they are expected as numbers/booleans."
                );

                s.history
                    .push(ChatMessage::new("tool", feedback));
                crate::config::save_history(&s.history);
                s.current_response.clear();
                s.status = AppStatus::Streaming;
                s.stream_tracker = Some(StreamTracker::new());
                drop(s);
                dbg_log!("Retrying agent loop round due to malformed tool call");
                return true;
            }

            let is_continuous = { state.lock().await.continuous_mode };
            if is_continuous && ctx.tool_rounds > 0 {
                dbg_log!("Continuous mode active, assistant responded with text prose. Ending continuous mode turn.");
                let mut s = state.lock().await;
                s.continuous_mode = false;
            } else if is_continuous && ctx.tool_rounds == 0 {
                dbg_log!("Continuous mode active, but assistant gave a plain conversational reply (no tools used). Ending turn.");
                let mut s = state.lock().await;
                s.continuous_mode = false;
            }

            // Finish gate: the model wants to stop with a prose answer. If it
            // edited code this task, don't accept "done" on a red build — run a
            // compile check and, if it fails, hand the errors back and force
            // another round. Skip on the forced wrap-up turn (tools already
            // disabled) and once the retry budget is spent, so we can't spin.
            if policy.should_verify_completion()
                && ctx.made_edits
                && !ctx.force_final
                && ctx.finish_gate_retries < MAX_FINISH_GATE_RETRIES
            {
                let root = ctx.edit_root
                    .clone()
                    .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
                dbg_log!("Finish gate: compile-checking {} before accepting done", root.display());
                if let Some(errors) = cached_compiler_check(&root, &mut ctx.compile_dirty, &mut ctx.compile_cache).await {
                    if errors.starts_with("__BUILD_UNVERIFIED__") {
                        // Checker couldn't run — surface it but accept done rather
                        // than spinning against an environment we can't fix.
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
                        dbg_log!("Finish gate: build is RED, forcing a fix round ({}/{})", ctx.finish_gate_retries, MAX_FINISH_GATE_RETRIES);
                        let mut s = state.lock().await;
                        s.history
                            .push(ChatMessage::new("assistant", ctx.final_content.clone()));
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

/// Drive one already-recorded prompt through the shared agent loop and finalize
/// it. This is the single turn-lifecycle implementation: it runs
/// [`run_single_turn`] until the turn stops (so it shares the `TurnMachine`,
/// approval/safety policy, retry behavior, compiler/build verification, and the
/// `complete_task` finish gate), then records the assistant reply, resolves and
/// tracks token usage, persists history/session, and fires completion
/// side-effects.
///
/// Both the interactive queue orchestrator and the headless raw CLI call this,
/// so `--prompt` execution and interactive execution can no longer diverge. It
/// is non-interactive by construction: interactivity lives entirely in the
/// injected [`policy::TurnPolicy`], which the raw CLI supplies as a headless,
/// non-blocking implementation. Returns the finished [`TurnContext`] for the
/// caller to inspect (final content, whether the task completed, tool rounds).
pub async fn run_agent_turn<P: policy::TurnPolicy + 'static>(
    client: &reqwest::Client,
    state: &Arc<Mutex<AppState>>,
    cancel_token: &tokio_util::sync::CancellationToken,
    policy: &Arc<P>,
    stream_buffer: &Arc<Mutex<StreamBuffer>>,
) -> TurnContext {
    let prompt_start_time = std::time::Instant::now();

    let mut ctx = TurnContext::new();
    while run_single_turn(client, state, cancel_token, policy, stream_buffer, &mut ctx).await {}

    if !ctx.final_content.is_empty() {
        dbg_log!("Finishing agent loop, writing final assistant reply");
        crate::logger::operational_event(
            "turn.finish",
            serde_json::json!({
                "completed_task": ctx.task_completed,
                "tool_rounds": ctx.tool_rounds,
                "content_bytes": ctx.final_content.len(),
                "cancelled": cancel_token.is_cancelled(),
            }),
        );

        let mut s = state.lock().await;
        s.continuous_mode = false;
        s.response_time = Some(prompt_start_time.elapsed());
        // On the complete_task path the summary was already appended; only
        // record token usage / notify below, don't duplicate the reply.
        if !ctx.task_completed {
            let mut msg = ChatMessage::new("assistant", ctx.final_content.clone());
            msg.response_time_ms = s.response_time.map(|d| d.as_millis() as u64);
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
            msg.token_usage = usage.clone();
        }

        let active_id = s.active_session_id.clone();
        crate::config::save_session_history(&active_id, &s.history);
        // Turn end: force the queued snapshot to disk, on a blocking thread so
        // the runtime keeps serving the UI.
        crate::config::flush_history_async();

        s.current_response.clear();
        s.status = AppStatus::Idle;

        if let Some(u) = &usage {
            crate::config::track_usage(u.prompt_tokens as u64, u.completion_tokens as u64);
        }
        s.current_token_usage = usage;
        drop(s);

        // Fetch live Gemini model quota from proxy endpoint
        let state_quota = Arc::clone(state);
        let client_quota = client.clone();
        tokio::spawn(async move {
            fetch_model_quota(&client_quota, &state_quota).await;
        });

        // Notify the user that the agent loop completed successfully.
        let _ = crate::notifications::notify_finished(crate::notifications::FinishedStatus::Success);
    }

    ctx
}

pub async fn process_queue_orchestrator<P: policy::TurnPolicy + 'static>(
    client: reqwest::Client,
    state: Arc<Mutex<AppState>>,
    cancel_token: tokio_util::sync::CancellationToken,
    policy: Arc<P>,
) {
    dbg_log!("Orchestrator started");
    loop {
        let next_prompt = {
            let mut s = state.lock().await;
            if s.pending_queue.is_empty() {
                dbg_log!("Pending queue empty, setting status to Idle");
                s.status = AppStatus::Idle;
                s.delegation_active = false;
                // Clear the single-flight guard under the same lock that saw the
                // queue empty, so an enqueue racing this exit either lands before
                // (queue non-empty here → we keep going) or after (guard clear →
                // the enqueuer spawns a fresh orchestrator). No lost wakeups, no
                // second concurrent orchestrator.
                s.orchestrator_running = false;
                if s.config.discord_rpc_enabled {
                    s.discord_rpc.clear_activity();
                }
                break;
            }
            let model_name = s.model_name.clone();
            s.status = AppStatus::Streaming;
            if s.config.discord_rpc_enabled {
                s.discord_rpc.set_activity("Streaming", &format!("Using model: {}", model_name));
            }
            s.generation_start_time = Some(std::time::Instant::now());
            s.stream_tracker = Some(StreamTracker::new());
            s.recent_read_calls.clear();
            s.read_file_mtimes.clear();
            let prompt = s.pending_queue.remove(0);
            dbg_log!("Popped prompt from queue: '{}'", prompt);
            prompt
        };

        let stream_buffer = Arc::new(Mutex::new(StreamBuffer {
            content: String::new(),
        }));
        let is_wakeup = next_prompt.starts_with("__task_wakeup__:");

        let mut is_first_prompt = false;
        if !is_wakeup {
            let s = state.lock().await;
            is_first_prompt = s.history.is_empty();
        }

        record_prompt_to_history(&state, is_wakeup, &next_prompt).await;
        crate::logger::operational_event(
            "turn.start",
            serde_json::json!({"wakeup": is_wakeup}),
        );

        if is_first_prompt {
            spawn_title_generation(&client, &state, next_prompt.clone()).await;
        }

        run_agent_turn(&client, &state, &cancel_token, &policy, &stream_buffer).await;

        if cancel_token.is_cancelled() {
            dbg_log!("Cancel token is cancelled, exiting orchestrator loop");
            // Best-effort: notify the user that a cancellation happened.
            let _ = crate::notifications::notify_finished(
                crate::notifications::FinishedStatus::Cancelled,
            );
            break;
        }
    }
    // Safety net: every loop exit that isn't the queue-empty branch (stream
    // error, cancel, empty content) lands here — always release the guard so a
    // future turn can start.
    state.lock().await.orchestrator_running = false;
    dbg_log!("Orchestrator finished");
}

pub async fn fetch_model_quota(client: &reqwest::Client, state: &Arc<Mutex<AppState>>) {
    let (url, model_name, api_key_opt) = {
        let s = state.lock().await;
        let active_url = s.api_base_url.clone();
        let key = s
            .config
            .models
            .iter()
            .find(|m| m.url == active_url || m.model == s.model_name)
            .and_then(|m| m.api_key.clone());
        (active_url, s.model_name.clone(), key)
    };

    if !url.contains("localhost:3000")
        && !url.contains("127.0.0.1:3000")
        && !url.contains("127.0.0.1:10531")
        && !url.contains("localhost:10531")
    {
        return;
    }

    // Construct proxy base URL (remove /v1/chat/completions or trailing slashes)
    let base_url = if let Some(idx) = url.find("/v1") {
        &url[..idx]
    } else {
        url.trim_end_matches('/')
    };
    let status_url = format!("{}/auth/status", base_url);

    let mut req = client.get(&status_url);
    if let Some(key) = api_key_opt {
        req = req.header("Authorization", format!("Bearer {key}"));
    }

    let Ok(res) = req.send().await else {
        return;
    };
    let Ok(json) = res.json::<serde_json::Value>().await else {
        return;
    };

    let quota_obj = json.get("quota");
    let buckets_arr = quota_obj
        .and_then(|q| q.get("buckets").or_else(|| q.get("quotaBuckets")))
        .and_then(|b| b.as_array());

    if let Some(quota_buckets) = buckets_arr {
        let mut matched_pct = None;
        for bucket in quota_buckets {
            if let Some(model_id) = bucket.get("modelId").and_then(|m| m.as_str())
                && let Some(fraction) = bucket.get("remainingFraction").and_then(|f| f.as_f64()) {
                    let pct = (fraction * 100.0) as f32;
                    if matched_pct.is_none() {
                        matched_pct = Some(pct);
                    }
                    if model_id == model_name || model_name.contains(model_id) || model_id.contains(&model_name) {
                        matched_pct = Some(pct);
                        break;
                    }
                }
        }
        if let Some(pct) = matched_pct {
            let mut s = state.lock().await;
            s.model_quota_remaining = Some(pct);
            s.request_redraw();
        }
        return;
    }

    // The ChatGPT/Codex usage response reports account-wide rate limits rather
    // than per-model Gemini-style buckets. Use the primary window for the
    // footer quota indicator; /status and /quota display both windows.
    let primary_window = json
        .get("rate_limits")
        .and_then(|r| r.get("primary"))
        .or_else(|| json.get("rate_limit").and_then(|r| r.get("primary_window")));
    if let Some(used_percent) = primary_window
        .and_then(|p| p.get("used_percent"))
        .and_then(|v| v.as_f64())
    {
        let mut s = state.lock().await;
        s.model_quota_remaining = Some((100.0 - used_percent).clamp(0.0, 100.0) as f32);
        s.request_redraw();
    }
}

pub fn parse_multimodal_content(text: &str) -> serde_json::Value {
    if !text.contains("![image](file://") {
        return serde_json::Value::String(text.to_string());
    }

    let mut parts: Vec<serde_json::Value> = Vec::new();
    let mut remaining = text;

    while let Some(start_idx) = remaining.find("![image](file://") {
        let text_part = &remaining[..start_idx];
        if !text_part.is_empty() {
            parts.push(serde_json::json!({
                "type": "text",
                "text": text_part.to_string(),
            }));
        }

        let path_start = start_idx + "![image](file://".len();
        let rest = &remaining[path_start..];
        if let Some(end_idx) = rest.find(')') {
            let path_str = &rest[..end_idx];
            if let Ok(bytes) = std::fs::read(path_str) {
                use base64::{Engine as _, engine::general_purpose};
                let base64_str = general_purpose::STANDARD.encode(bytes);
                let mime = if path_str.ends_with(".jpg") || path_str.ends_with(".jpeg") {
                    "image/jpeg"
                } else if path_str.ends_with(".gif") {
                    "image/gif"
                } else if path_str.ends_with(".webp") {
                    "image/webp"
                } else {
                    "image/png"
                };
                parts.push(serde_json::json!({
                    "type": "image_url",
                    "image_url": {
                        "url": format!("data:{};base64,{}", mime, base64_str),
                    }
                }));
            } else {
                parts.push(serde_json::json!({
                    "type": "text",
                    "text": format!("![image](file://{})", path_str),
                }));
            }
            remaining = &rest[end_idx + 1..];
        } else {
            break;
        }
    }

    if !remaining.is_empty() {
        parts.push(serde_json::json!({
            "type": "text",
            "text": remaining.to_string(),
        }));
    }

    serde_json::Value::Array(parts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_native_arguments_are_preserved_for_validation() {
        let value = parse_native_tool_arguments("{\"pattern\":");
        assert!(value.get("_invalid_arguments").is_some());
        assert!(value.get("_parse_error").is_some());
    }

    #[test]
    fn test_context_length_from_model_info() {
        let info = serde_json::json!({
            "general.architecture": "llama",
            "llama.context_length": 262144,
            "llama.embedding_length": 8192,
        });
        assert_eq!(context_length_from_model_info(&info), Some(262144));
        assert_eq!(context_length_from_model_info(&serde_json::json!({})), None);
    }

    #[test]
    fn test_trim_msgs_keeps_system_and_latest() {
        let big = "x".repeat(4000); // ~1000 tokens
        let mut msgs: Vec<serde_json::Value> = vec![
            serde_json::json!({"role": "system", "content": "sys"}),
            serde_json::json!({"role": "user", "content": big.clone()}),
            serde_json::json!({"role": "assistant", "content": big.clone()}),
            serde_json::json!({"role": "user", "content": big.clone()}),
        ];
        // budget fits only ~1 big message
        let dropped = trim_msgs_to_budget(&mut msgs, 1100);
        assert_eq!(dropped, 1);
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0]["role"], "system");
        // huge budget: nothing dropped
        let mut msgs2: Vec<serde_json::Value> = vec![
            serde_json::json!({"role": "system", "content": "sys"}),
            serde_json::json!({"role": "user", "content": "hi"}),
        ];
        assert_eq!(trim_msgs_to_budget(&mut msgs2, 8192), 0);
        assert_eq!(msgs2.len(), 2);
    }

    #[test]
    fn test_inject_system_reminder_logic() {
        // Less than 4 messages: no reminder injected
        let mut msgs: Vec<serde_json::Value> = vec![
            serde_json::json!({"role": "system", "content": "sys"}),
            serde_json::json!({"role": "user", "content": "hello"}),
            serde_json::json!({"role": "assistant", "content": "hi"}),
        ];
        inject_system_reminder(&mut msgs);
        assert_eq!(msgs.len(), 3);

        // 4 or more messages: reminder is appended to the last message
        let mut msgs2: Vec<serde_json::Value> = vec![
            serde_json::json!({"role": "system", "content": "sys"}),
            serde_json::json!({"role": "user", "content": "hello"}),
            serde_json::json!({"role": "assistant", "content": "hi"}),
            serde_json::json!({"role": "user", "content": "tell me a story"}),
        ];
        inject_system_reminder(&mut msgs2);
        assert_eq!(msgs2.len(), 4);
        assert!(msgs2[3]["content"]
            .as_str()
            .unwrap()
            .contains("REMINDER: Follow the configured tool protocol"));
        assert!(msgs2[3]["content"].as_str().unwrap().contains("tell me a story"));
    }

    #[test]
    fn test_parse_multimodal_content_plain() {
        let val = parse_multimodal_content("Hello world");
        assert_eq!(val, serde_json::Value::String("Hello world".to_string()));
    }

    #[test]
    fn test_parse_multimodal_content_with_image_nonexistent() {
        let val = parse_multimodal_content(
            "Look at this: ![image](file:///nonexistent/path.png) interesting!",
        );
        assert!(val.is_array());
        let arr = val.as_array().unwrap();
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[0]["type"], "text");
        assert_eq!(arr[0]["text"], "Look at this: ");
        assert_eq!(arr[1]["type"], "text");
        assert_eq!(arr[1]["text"], "![image](file:///nonexistent/path.png)");
        assert_eq!(arr[2]["type"], "text");
        assert_eq!(arr[2]["text"], " interesting!");
    }

    #[tokio::test]
    async fn test_confirm_and_execute_bypassed() {
        let state = Arc::new(Mutex::new(AppState::new()));
        state.lock().await.agent_mode = crate::config::AgentMode::Build;
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let args = serde_json::json!({
            "path": "sandbox/test_bypass.txt",
            "content": "bypassed content",
            "overwrite": true
        });

        let (result, _) = confirm_and_execute(
            &state,
            &cancel_token,
            "write_to_file",
            &args,
            "write_to_file",
            true,
            None,
        )
        .await;
        assert!(
            result.contains("wrote")
                || result.contains("created")
                || result.contains("test_bypass.txt"),
            "got result: {result}"
        );

        let _ = std::fs::remove_file("sandbox/test_bypass.txt");
    }

    #[tokio::test]
    async fn test_compact_history_strips_thinking_blocks() {
        let mut history = vec![
            crate::app::ChatMessage::new("assistant", "<think>\nThinking about files...\n</think>\nHere is the answer"),
            crate::app::ChatMessage::new("tool", "tool output"),
        ];
        compact_history_to_budget(&mut history, 5000).await;
        assert_eq!(history[0].content, "\nHere is the answer");
        assert_eq!(history[1].content, "tool output");
    }

    #[test]
    fn test_classify_tool_msg() {
        assert_eq!(
            classify_tool_msg(&ChatMessage::new("tool", "run_command: done")),
            Some("throwaway")
        );
        assert_eq!(
            classify_tool_msg(&ChatMessage::new("tool", "grep: match")),
            Some("throwaway")
        );
        assert_eq!(
            classify_tool_msg(&ChatMessage::new("tool", "view_file: [File: x]")),
            Some("file")
        );
        assert_eq!(
            classify_tool_msg(&ChatMessage::new("tool", "get_weather: sunny")),
            Some("other")
        );
        assert_eq!(classify_tool_msg(&ChatMessage::new("assistant", "hi")), None);
    }



    #[test]
    fn test_tool_signature_buckets_full_reads() {
        let full_default = serde_json::json!({"path": "src/main.rs"});
        let full_start1 = serde_json::json!({"path": "src/main.rs", "start_line": 1});
        let paged = serde_json::json!({"path": "src/main.rs", "start_line": 500, "end_line": 1000});
        let other = serde_json::json!({"path": "src/other.rs"});
        // Two full/default reads of the same file collapse to one signature.
        assert_eq!(
            tool_signature("view_file", &full_default),
            tool_signature("view_file", &full_start1)
        );
        // A distinct explicit page is its own signature.
        assert_ne!(
            tool_signature("view_file", &full_default),
            tool_signature("view_file", &paged)
        );
        assert_ne!(
            tool_signature("view_file", &full_default),
            tool_signature("view_file", &other)
        );
    }

    #[test]
    fn test_is_read_only_tool() {
        assert!(is_read_only_tool("view_file"));
        assert!(is_read_only_tool("grep"));
        assert!(!is_read_only_tool("write_to_file"));
        assert!(!is_read_only_tool("run_command"));
        assert!(!is_read_only_tool("todo_write"));
    }

    #[test]
    fn test_delegation_is_checked_as_potentially_mutating() {
        assert!(is_mutating_tool("spawn_agent"));
        assert!(is_mutating_tool("send_agent"));
        assert!(!is_mutating_tool("todo_write"));
    }

    #[test]
    fn test_view_file_repeat_is_mtime_aware() {
        let t0 = std::time::SystemTime::now();
        let t1 = t0 + std::time::Duration::from_secs(30);
        // Never read before -> not a repeat (allow the first read).
        assert!(!view_file_unchanged_since_last_read(None, Some(t0)));
        // Read before, unchanged -> repeat (block redundant re-read).
        assert!(view_file_unchanged_since_last_read(Some(t0), Some(t0)));
        // Read before, file changed on disk -> not a repeat (allow refresh).
        assert!(!view_file_unchanged_since_last_read(Some(t0), Some(t1)));
        // File gone/unstatable after a read -> not a repeat (let it proceed/error naturally).
        assert!(!view_file_unchanged_since_last_read(Some(t0), None));
    }

    #[tokio::test]
    async fn test_compact_prunes_throwaway_before_file_contents() {
        // Large throwaway command output + small file contents.
        let big_cmd = format!(
            "run_command: {}",
            (0..60)
                .map(|i| format!("output line number {i}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        let file = "view_file: [File: src/main.rs, Lines 1 to 5 of 5]\n1: a\n2: b\n3: c\n4: d\n5: e";
        let file_original = file.to_string();
        let mut history = vec![
            ChatMessage::new("tool", big_cmd.clone()), // throwaway, oldest
            ChatMessage::new("tool", file.to_string()), // file contents, newer
        ];
        // Budget forces compaction; the throwaway must absorb the cut so the file
        // contents the agent is actively working on survive intact.
        compact_history_to_budget(&mut history, 80).await;
        assert_eq!(history[1].content, file_original, "file contents preserved");
        assert_ne!(history[0].content, big_cmd, "throwaway was reduced");
        assert!(
            !history[0].content.contains("line number 59"),
            "throwaway truncated: {}",
            history[0].content
        );
    }

    #[tokio::test]
    async fn test_run_compiler_check_success() {
        let cwd = std::env::current_dir().unwrap();
        let check = run_compiler_check(&cwd).await;
        assert!(check.is_none());
    }

    #[test]
    fn project_root_from_relative_file_is_a_real_directory() {
        let root = get_tool_project_root(
            "delete_file",
            &serde_json::json!({"path": "src/temp.rs"}),
        );
        assert!(root.is_absolute());
        assert!(root.is_dir());
        assert!(root.join("Cargo.toml").exists());
    }

    #[test]
    fn test_align_alternating_messages() {
        let raw = vec![
            serde_json::json!({"role": "system", "content": "Prompt"}),
            serde_json::json!({"role": "system", "content": "Summary"}),
            serde_json::json!({"role": "assistant", "content": "Grep"}),
            serde_json::json!({"role": "user", "content": "Result"}),
        ];
        let aligned = align_alternating_messages(raw);
        assert_eq!(aligned.len(), 4);
        assert_eq!(aligned[0]["role"], "system");
        assert_eq!(aligned[0]["content"], "Prompt\n\nSummary");
        assert_eq!(aligned[1]["role"], "user");
        assert_eq!(aligned[1]["content"], "[Context initialization]");
        assert_eq!(aligned[2]["role"], "assistant");
        assert_eq!(aligned[3]["role"], "user");
    }

    #[test]
    fn test_build_dynamic_context_tail() {
        let todo = |content: &str, status: &str| crate::app::TodoItem {
            content: content.to_string(),
            status: status.to_string(),
            priority: "high".to_string(),
        };

        // No files and no todos: the context section is returned untouched.
        assert_eq!(
            build_dynamic_context_tail("# Env".to_string(), &[], &[]),
            "# Env"
        );

        // Files-in-context section lists each file as a bullet.
        let with_files = build_dynamic_context_tail(
            "# Env".to_string(),
            &["src/a.rs".to_string(), "src/b.rs".to_string()],
            &[],
        );
        assert!(with_files.contains("# Files already in context"));
        assert!(with_files.contains("- src/a.rs"));
        assert!(with_files.contains("- src/b.rs"));

        // Task plan renders status markers and 1-based ordering.
        let with_todos = build_dynamic_context_tail(
            String::new(),
            &[],
            &[
                todo("done thing", "completed"),
                todo("active thing", "in_progress"),
                todo("later thing", "pending"),
            ],
        );
        assert!(with_todos.contains("# Your current task plan"));
        assert!(with_todos.contains("1. [x] done thing (high)"));
        assert!(with_todos.contains("2. [~] active thing (high)"));
        assert!(with_todos.contains("3. [ ] later thing (high)"));
    }
}
