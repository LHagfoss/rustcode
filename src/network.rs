use crate::app::{AppState, AppStatus, ChatMessage, StreamTracker, TokenUsage, ToolConfirmation};
use futures_util::{StreamExt, future::join_all};
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

macro_rules! dbg_log {
    ($($arg:tt)*) => {{
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/rustcode-debug.log")
        {
            let now = chrono::Local::now().format("%H:%M:%S%.3f");
            let _ = writeln!(f, "[{now}] {}", format!($($arg)*));
        }
    }};
}

/// Fast token estimate (~4 chars per token). Single source of truth for token
/// sizing, shared with compaction. Deliberately approximate and synchronous —
/// compaction and usage display only need a cheap size signal, not exact counts,
/// so we skip the `fm token-count` subprocess (which added latency per message).
fn count_tokens(text: &str) -> u32 {
    compaction::estimate_tokens(text) as u32
}

/// Classify a stored tool result for compaction priority.
/// Returns `None` for non-tool messages. Tool results are bucketed into:
/// "throwaway" (run_command, grep, glob, list_directory, get_time,
/// find_symbol, get_project_map, search_web) — pruned first; "file"
/// (view_file contents) — pruned last; and "other".
fn classify_tool_msg(m: &ChatMessage) -> Option<&'static str> {
    if m.role != "tool" {
        return None;
    }
    let name = m.content.split(':').next().unwrap_or("").trim();
    Some(match name {
        "run_command" | "grep" | "glob" | "list_directory" | "get_time"
        | "find_symbol" | "get_project_map" | "search_web" => "throwaway",
        "view_file" => "file",
        _ => "other",
    })
}

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

const MAX_TOOL_OUTPUT_BYTES: usize = 50 * 1024;
const MAX_TOOL_OUTPUT_LINES: usize = 1000;

/// Truncate tool output at execution time if it exceeds size limits.
/// Full output is saved to a temp file so the agent can still access it.
fn truncate_tool_output(name: &str, result: String) -> String {
    let bytes = result.len();
    let lines: Vec<&str> = result.lines().collect();
    let line_count = lines.len();

    if bytes <= MAX_TOOL_OUTPUT_BYTES && line_count <= MAX_TOOL_OUTPUT_LINES {
        return result;
    }

    // Save full output to temp file
    let saved_path = save_full_tool_output(name, &result);

    // Head+tail truncation: keep first 30% and last 30% of allowed lines
    let max_lines = MAX_TOOL_OUTPUT_LINES.min(line_count);
    let head_count = (max_lines * 3) / 10;
    let tail_count = (max_lines * 3) / 10;

    let head: String = lines[..head_count.min(line_count)].join("\n");
    let tail: String = if tail_count > 0 && line_count > head_count + tail_count {
        lines[line_count - tail_count..].join("\n")
    } else {
        String::new()
    };

    let omitted_lines = line_count.saturating_sub(head_count + tail_count);
    let omitted_bytes = bytes.saturating_sub(head.len() + tail.len());

    let path_note = match saved_path {
        Some(p) => format!(" Full output saved to: {}\nUse grep to search the full content or view_file with line offsets to read specific sections.", p),
        None => String::new(),
    };

    format!(
        "{}\n\n... [{} lines / {} bytes truncated] ...\n\n{}{}",
        head, omitted_lines, omitted_bytes, tail, 
        format!("\n\n[Output truncated: {} bytes total, {} lines.{}]", bytes, line_count, path_note)
    )
}

/// Save full tool output to a temp file, returning the path on success.
fn save_full_tool_output(name: &str, content: &str) -> Option<String> {
    let dir = crate::config::get_config_dir()?.join("tool_output");
    let _ = std::fs::create_dir_all(&dir);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or(std::time::Duration::from_secs(0))
        .as_millis();
    let safe_name: String = name.chars().filter(|c| c.is_alphanumeric() || *c == '_').collect();
    let path = dir.join(format!("{ts}_{safe_name}.txt"));
    match std::fs::write(&path, content) {
        Ok(_) => Some(path.to_string_lossy().to_string()),
        Err(_) => None,
    }
}

/// Extract the path from an intact `view_file: [File: <path>, ...]` result.
fn view_file_path_from_tool_msg(content: &str) -> Option<String> {
    let rest = content.strip_prefix("view_file:")?;
    let after = rest.split("[File:").nth(1)?;
    let path = after.split(',').next()?.trim();
    if path.is_empty() {
        return None;
    }
    Some(path.to_string())
}

/// Collapse repeated reads of the same file: keep only the newest intact result per
/// path and stub the older ones. Stops the read → prune → re-read cycle at its source.
fn dedupe_view_file_reads(history: &mut [ChatMessage]) {
    use std::collections::HashMap;
    let mut keep_index: HashMap<String, usize> = HashMap::new();
    for (idx, m) in history.iter().enumerate() {
        if let Some(path) = view_file_path_from_tool_msg(&m.content) {
            keep_index.insert(path, idx); // newest wins
        }
    }
    for (idx, m) in history.iter_mut().enumerate() {
        if let Some(path) = view_file_path_from_tool_msg(&m.content)
            && keep_index.get(&path) != Some(&idx)
        {
            m.content = format!("view_file: [superseded by a later read of {path}]");
        }
    }
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
    dedupe_view_file_reads(history);

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
    if let Ok(resp) = client.get(&props_url).send().await {
        if resp.status().is_success() {
            if let Ok(body) = resp.json::<serde_json::Value>().await {
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

pub const RESPONSE_RESERVE_TOKENS: u32 = 1024;

fn estimate_msg_chars(msg: &serde_json::Value) -> usize {
    match msg.get("content") {
        Some(serde_json::Value::String(s)) => s.len(),
        Some(other) => other.to_string().len(),
        None => 0,
    }
}

/// Drop the oldest non-system messages until the payload fits the token
/// budget (~4 chars/token), keeping the system prompt and the latest
/// exchange. Returns how many messages were dropped.
pub fn trim_msgs_to_budget(msgs: &mut Vec<serde_json::Value>, budget_tokens: u32) -> usize {
    let budget_chars = budget_tokens as usize * 4;
    let mut total: usize = msgs.iter().map(estimate_msg_chars).sum();
    let mut dropped = 0;
    while total > budget_chars && msgs.len() > 3 {
        total -= estimate_msg_chars(&msgs[1]);
        msgs.remove(1);
        dropped += 1;
    }
    dropped
}

/// If the message history has grown long (e.g. >= 4 messages), inject a brief
/// system reminder right before the latest user message or tool result. This
/// prevents the model from forgetting the core guidelines and tool formats
/// due to attention dilution in long contexts.
pub fn inject_system_reminder(msgs: &mut Vec<serde_json::Value>) {
    if msgs.len() >= 4 {
        let reminder_text = "REMINDER: You are rustcode. Always follow your core instructions:\n\
            - Be extremely concise and direct. No filler or preamble.\n\
            - To call a tool, output exactly one fenced `tool` block containing a single JSON object. Do not output any conversational text or narration before or after the block.\n\
            - Available tools: view_file, replace_file_content, multi_replace_file_content, write_to_file, delete_file, move_file, copy_file, list_directory, grep, glob, run_command, search_web, find_symbol, get_project_map.";
        
        if let Some(last_msg) = msgs.last_mut() {
            if let Some(content) = last_msg.get_mut("content") {
                match content {
                    serde_json::Value::String(s) => {
                        *s = format!("{}\n\n{}", s, reminder_text);
                    }
                    serde_json::Value::Array(arr) => {
                        arr.push(serde_json::json!({
                            "type": "text",
                            "text": format!("\n\n{}", reminder_text)
                        }));
                    }
                    _ => {}
                }
            }
        }
    }
}

/// Read-only tools whose results can be safely short-circuited by the repeat guard.
fn is_read_only_tool(name: &str) -> bool {
    matches!(
        name,
        "view_file" | "list_directory" | "grep" | "glob" | "get_time"
            | "find_symbol" | "get_project_map" | "search_web"
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

fn parse_sse_line(line: &str) -> Option<&str> {
    if let Some(s) = line.strip_prefix("data: ") {
        if s == "[DONE]" || s.is_empty() {
            return None;
        }
        Some(s)
    } else {
        None
    }
}

pub struct StreamBuffer {
    pub content: String,
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
    let payload = serde_json::json!({
        "model": model,
        "messages": aligned_messages,
        "stream": true,
        "stream_options": {
            "include_usage": true
        },
        "temperature": 0.7,
        "max_tokens": 4096,
    });
    dbg_log!(
        "stream_request: Request payload: {}",
        serde_json::to_string_pretty(&payload).unwrap_or_default()
    );

    // Establish the connection with retry/backoff on transient failures
    // (429, 5xx, network blips). We only retry here, before any SSE bytes are
    // read — retrying mid-stream would duplicate partial output.
    let mut attempt = 0usize;
    let response = loop {
        if cancel_token.is_cancelled() {
            return Err("cancelled".to_string());
        }
        match client.post(url).json(&payload).send().await {
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
                                if let Some(choices) = val.get("choices").and_then(|c| c.as_array()) {
                                    if !choices.is_empty() {
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
                                             for tc in tool_calls {
                                                 let idx = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
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
                                }
                                if let Some(usage) = val.get("usage").filter(|_| !quiet) {
                                    if let (Some(p), Some(c), Some(t)) = (
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

        let args_json: serde_json::Value = serde_json::from_str(&acc.arguments)
            .unwrap_or(serde_json::Value::Object(Default::default()));

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

fn has_intended_tool_call(content: &str) -> bool {
    let lower = content.to_lowercase();
    lower.contains("```tool") || lower.contains("```json") || lower.contains("[tool_calls]")
}

fn is_cut_off(content: &str, finish_reason: Option<&str>) -> bool {
    // If the model already produced a valid tool call, we don't need to continue text generation.
    // We should execute the tool and get its output first.
    if !crate::tools::parse_tool_calls(content, crate::config::ToolProtocol::Native).is_empty() {
        return false;
    }

    if finish_reason == Some("length") {
        return true;
    }

    // Check for unclosed <think> tag
    let has_think = content.contains("<think>");
    let has_think_end = content.contains("</think>");
    if has_think && !has_think_end {
        return true;
    }

    // Check for unclosed tool block
    let triple_backticks_count = content.matches("```").count();
    if triple_backticks_count % 2 != 0 {
        return true;
    }

    // Check for unclosed <tool_call> tag
    let has_tool_call = content.contains("<tool_call>");
    let has_tool_call_end = content.contains("</tool_call>");
    if has_tool_call && !has_tool_call_end {
        return true;
    }

    // Qwen-family open models often close </think> and then emit a stop token
    // with no actual answer or tool call. Treat that as incomplete so the
    // continuation path nudges the model instead of stalling for a manual
    // "continue".
    if is_reasoning_only(content) {
        return true;
    }

    false
}

/// Remove every `<think>...</think>` span so we can inspect the model's actual
/// answer/tool output.
fn strip_think_blocks(content: &str) -> String {
    let mut out = String::new();
    let mut rest = content;
    while let Some(start) = rest.find("<think>") {
        out.push_str(&rest[..start]);
        if let Some(end) = rest[start..].find("</think>") {
            rest = &rest[start + end + "</think>".len()..];
        } else {
            // unclosed — drop the remainder (handled by the unclosed-think check)
            rest = "";
            break;
        }
    }
    out.push_str(rest);
    out
}

/// True when the turn is nothing but reasoning: a non-empty response whose only
/// content is `<think>` blocks, leaving no answer or tool call to act on.
fn is_reasoning_only(content: &str) -> bool {
    if content.trim().is_empty() {
        return false;
    }
    strip_think_blocks(content).trim().is_empty()
}

/// Show the Y/N confirmation modal (when the tool requires it) and run the
/// tool. `display_name` is what the modal shows — subagent calls prefix it
/// with the agent id so the user knows who is asking.
fn get_diff_preview(name: &str, args: &serde_json::Value) -> Option<String> {
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
        Some(prev)
    } else if name == "write_to_file" {
        let path = args.get("path").and_then(|p| p.as_str()).unwrap_or("");
        let old_content = std::fs::read_to_string(&path).unwrap_or_default();
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
        Some(prev)
    } else {
        None
    }
}

fn get_tool_project_root(_name: &str, args: &serde_json::Value) -> std::path::PathBuf {
    let raw_path = if let Some(p) = args.get("path").and_then(|p| p.as_str()) {
        Some(p)
    } else if let Some(s) = args.get("src").and_then(|s| s.as_str()) {
        Some(s)
    } else if let Some(d) = args.get("dest").and_then(|d| d.as_str()) {
        Some(d)
    } else {
        None
    };

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
            return current;
        }
        if let Some(parent) = current.parent() {
            current = parent.to_path_buf();
        } else {
            break;
        }
    }

    std::env::current_dir().unwrap_or_default()
}

fn strip_ansi_escapes(s: &str) -> String {
    if let Ok(re) = regex::Regex::new(r"\x1B\[[0-9;?]*[a-zA-Z]") {
        re.replace_all(s, "").into_owned()
    } else {
        s.to_string()
    }
}

async fn run_compiler_check(cwd: &std::path::Path) -> Option<String> {
    if cwd.join("Cargo.toml").exists() {
        let mut cmd = tokio::process::Command::new("cargo");
        cmd.args(["check", "--message-format=json"])
            .current_dir(cwd)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => return Some(format!("Failed to spawn cargo check: {e}")),
        };

        let timeout_duration = std::time::Duration::from_secs(5);
        let output_res = tokio::time::timeout(timeout_duration, child.wait_with_output()).await;

        let output = match output_res {
            Ok(Ok(out)) => out,
            Ok(Err(e)) => return Some(format!("cargo check failed to run: {e}")),
            Err(_) => return Some("cargo check timed out after 5 seconds".to_string()),
        };

        let stdout_str = String::from_utf8_lossy(&output.stdout);
        let mut errors = Vec::new();

        for line in stdout_str.lines() {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(line) {
                if val.get("reason").and_then(|r| r.as_str()) == Some("compiler-message") {
                    if let Some(msg) = val.get("message") {
                        if let Some(level) = msg.get("level").and_then(|l| l.as_str()) {
                            if level == "error" {
                                if let Some(rendered) = msg.get("rendered").and_then(|r| r.as_str()) {
                                    errors.push(strip_ansi_escapes(rendered));
                                }
                            }
                        }
                    }
                }
            }
        }

        if !errors.is_empty() {
            return Some(errors.join("\n"));
        }
    } else if cwd.join("tsconfig.json").exists() {
        let mut cmd = tokio::process::Command::new("npx");
        cmd.args(["tsc", "--noEmit"])
            .current_dir(cwd)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => return Some(format!("Failed to spawn npx tsc: {e}")),
        };

        let timeout_duration = std::time::Duration::from_secs(5);
        let output_res = tokio::time::timeout(timeout_duration, child.wait_with_output()).await;

        let output = match output_res {
            Ok(Ok(out)) => out,
            Ok(Err(e)) => return Some(format!("npx tsc failed to run: {e}")),
            Err(_) => return Some("npx tsc timed out after 5 seconds".to_string()),
        };

        if !output.status.success() {
            let stdout_str = String::from_utf8_lossy(&output.stdout);
            let stderr_str = String::from_utf8_lossy(&output.stderr);
            let mut combined = String::new();
            if !stdout_str.is_empty() {
                combined.push_str(&stdout_str);
            }
            if !stderr_str.is_empty() {
                if !combined.is_empty() {
                    combined.push('\n');
                }
                combined.push_str(&stderr_str);
            }
            if !combined.is_empty() {
                return Some(strip_ansi_escapes(&combined));
            }
        }
    }

    None
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
) -> (String, Option<String>) {
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

    let needs_confirm = !bypass_confirm
        && crate::tools::needs_confirmation(name)
        && !state.lock().await.auto_confirm;
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
        let run_fut = tokio::task::spawn_blocking(move || {
            crate::tools::set_active_session_id(Some(session_id));
            let result = crate::tools::execute(&name_owned, &args_owned);
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
                    s.status = AppStatus::Streaming;
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
                let run_fut = tokio::task::spawn_blocking(move || {
                    crate::tools::set_active_session_id(Some(session_id));
                    let result = crate::tools::execute(&name_owned, &args_owned);
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
            s.status = AppStatus::Streaming;
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

/// Max tool rounds a subagent gets before being cut off.
const MAX_SUBAGENT_ROUNDS: usize = 15;

fn push_status_line(s: &mut AppState, text: String) {
    s.history.push(ChatMessage::new("system", text));
    crate::config::save_history(&s.history);
}

/// Drop a leading <think>...</think> block so the main agent only gets the
/// subagent's actual reply, not its reasoning.
fn strip_leading_think(text: &str) -> &str {
    match (
        text.trim_start().starts_with("<think>"),
        text.find("</think>"),
    ) {
        (true, Some(i)) => text[i + "</think>".len()..].trim_start(),
        _ => text,
    }
}

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
    let stream_buffer = Arc::new(Mutex::new(StreamBuffer {
        content: String::new(),
    }));
    let mut rounds = 0usize;
    loop {
        if cancel_token.is_cancelled() {
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
        let system_prompt = format!(
            "{}\n\nYou are subagent {agent_id}, working for a main agent in the same \
rustcode session. Complete the task you were given, then reply in plain text \
with NO tool call — that reply is returned to the main agent. Keep the final \
reply compact and information-dense.\n\n{}",
            crate::tools::tool_system_prompt(false, protocol),
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
        let mut accumulated_content = String::new();
        let mut continuation_count = 0;
        const MAX_CONTINUATIONS: usize = 5;

        loop {
            let mut current_msgs = msgs.clone();
            if !accumulated_content.is_empty() {
                current_msgs.push(serde_json::json!({
                    "role": "assistant",
                    "content": accumulated_content
                }));
                current_msgs.push(serde_json::json!({
                    "role": "user",
                    "content": "continue"
                }));
            }
            stream_buffer.lock().await.content.clear();
            let stream_result = stream_request(
                client,
                Arc::clone(state),
                cancel_token.clone(),
                &api_base_url,
                &model_name,
                &current_msgs,
                Arc::clone(&stream_buffer),
                true,
            )
            .await;

            let finish_reason = match stream_result {
                Ok(fr) => fr,
                Err(e) => return format!("error: subagent request failed: {e}"),
            };

            let chunk_content = stream_buffer.lock().await.content.clone();
            accumulated_content.push_str(&chunk_content);

            if continuation_count < MAX_CONTINUATIONS
                && is_cut_off(&accumulated_content, finish_reason.as_deref())
            {
                dbg_log!(
                    "Subagent LLM response cut off. Auto-continuing (round {})...",
                    continuation_count + 1
                );
                continuation_count += 1;
                continue;
            }
            break;
        }

        let content = accumulated_content;
        if content.is_empty() {
            return "error: subagent returned an empty reply".to_string();
        }

        let protocol = { state.lock().await.config.tool_protocol };
        if let Some((name, args)) = crate::tools::parse_tool_call(&content, protocol) {
            if rounds >= MAX_SUBAGENT_ROUNDS {
                return format!(
                    "error: subagent {agent_id} hit the {MAX_SUBAGENT_ROUNDS}-round tool limit without a final reply"
                );
            }
            rounds += 1;
            let (result, diff_opt) = if crate::tools::is_agent_tool(&name) {
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
                    &name,
                    &args,
                    &format!("agent-{agent_id} · {name}"),
                    false,
                )
                .await
            };
            let mut s = state.lock().await;
            if let Some(a) = s.subagents.iter_mut().find(|a| a.id == agent_id) {
                a.history.push(ChatMessage::new("assistant", &content));
                let truncated_result = truncate_tool_output(&name, result);
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
        return strip_leading_think(&content).to_string();
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
            let agent_id = {
                let mut s = state.lock().await;
                let id = s.next_subagent_id;
                s.next_subagent_id += 1;
                s.subagents.push(crate::app::SubAgent {
                    id,
                    task: task.to_string(),
                    model,
                    history: vec![ChatMessage::new("user", task)],
                });
                let brief: String = task.chars().take(60).collect();
                push_status_line(&mut s, format!("agent-{id} spawned: {brief}"));
                id
            };
            let reply = run_subagent(client, state, cancel_token, agent_id).await;
            push_status_line(&mut *state.lock().await, format!("agent-{agent_id} done"));
            format!("(subagent id {agent_id} — follow up with send_agent)\n{reply}")
        }
        "send_agent" => {
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
                    a.history.push(ChatMessage::new("user", message));
                }
            }
            let reply = run_subagent(client, state, cancel_token, id).await;
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

async fn evaluate_and_expand_prompt(
    client: &reqwest::Client,
    config: &crate::config::AppConfig,
    prompt: &str,
) -> Option<(bool, String)> {
    let small_model_name = config.default.small();
    let (url, model) = crate::config::resolve_model_endpoint(config, small_model_name);

    let system_prompt = "You are a prompt optimizer and task classification assistant.\n\
Analyze the user's input and output ONLY a JSON object matching this schema: {\"is_goal\": boolean, \"expanded_prompt\": string}.\n\n\
Rules:\n\
- is_goal = false for greetings, casual chat, simple questions, or single-step requests.\n\
- is_goal = true ONLY for multi-step coding tasks requiring editing files, fixing bugs, refactoring, building, or git workflows.\n\n\
Examples:\n\
Input: \"hello how are you\"\n\
Output: {\"is_goal\": false, \"expanded_prompt\": \"hello how are you\"}\n\n\
Input: \"what is rustcode\"\n\
Output: {\"is_goal\": false, \"expanded_prompt\": \"what is rustcode\"}\n\n\
Input: \"fix the scrollbar bug in ui.rs and run tests\"\n\
Output: {\"is_goal\": true, \"expanded_prompt\": \"Fix the scrollbar rendering bug in src/ui.rs, update the scroll offset logic, and run tests to verify.\"}\n\n\
Return ONLY valid JSON. No markdown code blocks. No introductory text.";

    let payload = serde_json::json!({
        "model": model,
        "messages": [
            {"role": "system", "content": system_prompt},
            {"role": "user", "content": prompt}
        ],
        "temperature": 0.1,
        "max_tokens": 1024,
    });

    let res = client.post(&url)
        .json(&payload)
        .send()
        .await
        .ok()?;

    if !res.status().is_success() {
        return None;
    }

    #[derive(serde::Deserialize)]
    struct ExpansionResult {
        is_goal: bool,
        expanded_prompt: String,
    }

    let json: serde_json::Value = res.json().await.ok()?;
    let text = json.get("choices")?
        .get(0)?
        .get("message")?
        .get("content")?
        .as_str()?;

    let trimmed = text.trim();
    let cleaned = trimmed
        .strip_prefix("```json").unwrap_or(trimmed)
        .strip_prefix("```").unwrap_or(trimmed)
        .strip_suffix("```").unwrap_or(trimmed)
        .trim();

    if let Ok(parsed) = serde_json::from_str::<ExpansionResult>(cleaned) {
        let p_lower = prompt.trim().to_lowercase();
        let is_conversational = p_lower.starts_with("hello")
            || p_lower.starts_with("hi")
            || p_lower.starts_with("hey")
            || p_lower.contains("how are you")
            || p_lower.contains("who are you")
            || p_lower.contains("what can you do")
            || p_lower.starts_with("thanks")
            || p_lower.starts_with("thank you");

        let is_actual_goal = parsed.is_goal && !is_conversational;
        Some((is_actual_goal, parsed.expanded_prompt))
    } else {
        None
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


#[allow(unused_assignments)]
pub async fn process_queue_orchestrator(
    client: reqwest::Client,
    state: Arc<Mutex<AppState>>,
    cancel_token: tokio_util::sync::CancellationToken,
) {
    dbg_log!("Orchestrator started");
    loop {
        let mut next_prompt = {
            let mut s = state.lock().await;
            if s.pending_queue.is_empty() {
                dbg_log!("Pending queue empty, setting status to Idle");
                s.status = AppStatus::Idle;
                break;
            }
            s.status = AppStatus::Streaming;
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

        if is_first_prompt {
            let config = {
                let s = state.lock().await;
                s.config.clone()
            };
            {
                let mut s = state.lock().await;
                push_status_line(&mut s, "Optimizing prompt and detecting goal status...".to_string());
            }
            if let Some((is_goal, expanded_prompt)) = evaluate_and_expand_prompt(&client, &config, &next_prompt).await {
                let mut s = state.lock().await;
                s.history.pop();
                next_prompt = expanded_prompt;
                if is_goal {
                    s.continuous_mode = true;
                    push_status_line(&mut s, "Continuous mode (/goal) activated automatically by prompt optimizer.".to_string());
                } else {
                    push_status_line(&mut s, "Prompt optimized by small model.".to_string());
                }
            } else {
                let mut s = state.lock().await;
                s.history.pop();
            }
        }

        {
            let mut s = state.lock().await;
            if is_wakeup {
                let task_id = next_prompt.strip_prefix("__task_wakeup__:").unwrap_or("");
                s.history.push(ChatMessage::new(
                    "system",
                    format!("Task {task_id} has finished running in the background."),
                ));
            } else {
                s.history.push(ChatMessage::new("user", next_prompt.clone()));
            }
            let active_id = s.active_session_id.clone();
            crate::config::save_session_history(&active_id, &s.history);
            s.current_response.clear();
            s.current_token_usage = None;
            s.response_time = None;
        }

        if is_first_prompt {
            let client_clone = client.clone();
            let config_clone = {
                let s = state.lock().await;
                s.config.clone()
            };
            let session_id = {
                let s = state.lock().await;
                s.active_session_id.clone()
            };
            let first_msg = next_prompt.clone();
            tokio::spawn(async move {
                if let Some(title) = generate_title(&client_clone, &config_clone, &first_msg).await {
                    crate::config::save_session_title(&session_id, &title);
                }
            });
        }

        let prompt_start_time = std::time::Instant::now();

        let mut tool_rounds = 0;
        #[allow(unused_assignments)]
        let mut limit_reached = false;
        let mut last_sent_messages: Vec<serde_json::Value> = Vec::new();
        let mut final_content = String::new();
        let max_tool_rounds = { state.lock().await.config.max_tool_rounds };
        // Detects repetitive tool-call patterns (exact / semantic / stagnation /
        // churn) so continuous mode can't spin forever. One instance per task.
        let mut loop_detector = loop_detect::LoopDetector::new(6);
        loop {
            dbg_log!("Starting agent loop round {}", tool_rounds);

            // Try AI-driven compaction if history is long enough
            {
                let (api_url, model_name) = {
                    let s = state.lock().await;
                    (s.api_base_url.clone(), s.model_name.clone())
                };
                let mut s = state.lock().await;
                let budget = s.get_history_token_budget() as usize;
                if compaction::maybe_compact(&client, &api_url, &model_name, &mut s.history, budget).await {
                    dbg_log!("History compacted via AI summarization. Clearing read/dedup cache.");
                    s.recent_read_calls.clear();
                    s.read_file_mtimes.clear();
                    crate::config::save_history(&s.history);
                }
                drop(s);
            }

            let mut history_snapshot: Vec<ChatMessage> = {
                let s = state.lock().await;
                s.history
                    .iter()
                    .filter(|m| {
                        matches!(m.role.as_str(), "user" | "assistant" | "tool")
                            && !m.content.starts_with('/')
                    })
                    .cloned()
                    .collect()
            };

            let budget_token_limit = { state.lock().await.get_history_token_budget() };
            compact_history_to_budget(&mut history_snapshot, budget_token_limit).await;

            let (mut read_files, todos) = {
                let s = state.lock().await;
                let mut files: Vec<String> =
                    s.read_file_mtimes.keys().cloned().collect();
                files.sort();
                (files, s.todos.clone())
            };

            let current_snapshot = crate::context::ContextSnapshot::capture();
            let context_section = {
                let s = state.lock().await;
                match &s.context_snapshot {
                    Some(prev) => prev.diff(&current_snapshot)
                        .unwrap_or_else(|| "# Environment\n(unchanged since session start)".to_string()),
                    None => crate::context::environment_context(),
                }
            };
            let protocol = { state.lock().await.config.tool_protocol };
            let mut system_prompt = format!(
                "{}\n\n{}",
                crate::tools::tool_system_prompt(true, protocol),
                context_section
            );
            // Store the snapshot if this is the first turn
            {
                let mut s = state.lock().await;
                if s.context_snapshot.is_none() {
                    s.context_snapshot = Some(current_snapshot);
                }
            }

            // Remind the agent which files it already has so it doesn't re-read them.
            if !read_files.is_empty() {
                system_prompt.push_str(&format!(
                    "\n\n# Files already in context (do NOT re-read these unless they changed on disk)\n{}",
                    read_files
                        .drain(..)
                        .map(|f| format!("- {f}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                ));
            }

            // Re-inject the persistent task plan so the agent executes across turns
            // instead of re-planning from scratch.
            if !todos.is_empty() {
                system_prompt.push_str(
                    "\n\n# Your current task plan (execute in order; update via todo_write)\n",
                );
                for (i, t) in todos.iter().enumerate() {
                    let mark = match t.status.as_str() {
                        "completed" => "[x]",
                        "in_progress" => "[~]",
                        _ => "[ ]",
                    };
                    system_prompt.push_str(&format!(
                        "{}. {} {} ({})\n",
                        i + 1,
                        mark,
                        t.content,
                        t.priority
                    ));
                }
            }
            let mut msgs: Vec<serde_json::Value> = vec![serde_json::json!({
                "role": "system",
                "content": system_prompt.clone(),
            })];
            let mut first_user = true;
            msgs.extend(history_snapshot.into_iter().map(|m| {
                if m.role == "tool" {
                    serde_json::json!({
                        "role": "user",
                        "content": format!("<tool_result>\n{}\n</tool_result>", m.content),
                    })
                } else if m.role == "user" && first_user {
                    first_user = false;
                    serde_json::json!({
                        "role": "user",
                        "content": parse_multimodal_content(&m.content),
                    })
                } else if m.role == "user" {
                    serde_json::json!({
                        "role": "user",
                        "content": parse_multimodal_content(&m.content),
                    })
                } else {
                    serde_json::json!({"role": m.role, "content": m.content})
                }
            }));

            let window = { state.lock().await.active_context_window() };
            let budget = window.saturating_sub(RESPONSE_RESERVE_TOKENS).max(512);
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
            let mut accumulated_content = String::new();
            let mut continuation_count = 0;
            const MAX_CONTINUATIONS: usize = 5;
            let mut stream_err = None;

            loop {
                let mut current_msgs = msgs.clone();
                if !accumulated_content.is_empty() {
                    current_msgs.push(serde_json::json!({
                        "role": "assistant",
                        "content": accumulated_content
                    }));
                    current_msgs.push(serde_json::json!({
                        "role": "user",
                        "content": "continue"
                    }));
                }
                last_sent_messages = current_msgs.clone();

                state.lock().await.current_response.clear();
                stream_buffer.lock().await.content.clear();

                let stream_result = stream_request(
                    &client,
                    Arc::clone(&state),
                    cancel_token.clone(),
                    &api_base_url,
                    &model_name,
                    &current_msgs,
                    Arc::clone(&stream_buffer),
                    false,
                )
                .await;

                if cancel_token.is_cancelled() {
                    dbg_log!("Orchestrator: Stream request cancelled by token");
                    let mut s = state.lock().await;
                    let chunk_content = stream_buffer.lock().await.content.clone();
                    accumulated_content.push_str(&chunk_content);
                    if !accumulated_content.is_empty() {
                        let mut msg = ChatMessage::new("assistant", accumulated_content.clone());
                        msg.response_time_ms = Some(prompt_start_time.elapsed().as_millis() as u64);
                        s.history.push(msg);
                        crate::config::save_history(&s.history);
                    }
                    s.current_response.clear();
                    s.status = AppStatus::Idle;
                    break;
                }

                let finish_reason = match stream_result {
                    Ok(fr) => fr,
                    Err(e) => {
                        stream_err = Some(e);
                        break;
                    }
                };

                let chunk_content = stream_buffer.lock().await.content.clone();
                accumulated_content.push_str(&chunk_content);

                {
                    let mut s = state.lock().await;
                    s.current_response = accumulated_content.clone();
                }

                if continuation_count < MAX_CONTINUATIONS
                    && is_cut_off(&accumulated_content, finish_reason.as_deref())
                {
                    dbg_log!(
                        "Orchestrator: LLM response cut off. Auto-continuing (round {})...",
                        continuation_count + 1
                    );
                    continuation_count += 1;
                    continue;
                }
                break;
            }

            if cancel_token.is_cancelled() {
                break;
            }

            if let Some(e) = stream_err {
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
                break;
            }

            final_content = accumulated_content;
            dbg_log!(
                "Stream completed successfully. Content length: {} chars",
                final_content.len()
            );

            if final_content.is_empty() {
                dbg_log!("Stream returned empty content, finishing");
                let mut s = state.lock().await;
                s.status = AppStatus::Idle;
                s.current_token_usage = None;
                break;
            }

            let protocol = { state.lock().await.config.tool_protocol };
            let tool_calls = crate::tools::parse_tool_calls(&final_content, protocol);
            if !tool_calls.is_empty() {
                dbg_log!("Parsed {} tool call requests", tool_calls.len());

                // Loop detection: feed each requested call to the detector and
                // keep the worst status. Abort stops auto-execution; Warning
                // injects a nudge so the model changes approach.
                let mut loop_status = loop_detect::LoopStatus::Ok;
                for (name, args) in &tool_calls {
                    let (exact, category) = loop_detect::signatures(name, args);
                    let s = loop_detector.check(&exact, &category);
                    if s.rank() > loop_status.rank() {
                        loop_status = s;
                    }
                }
                match loop_status {
                    loop_detect::LoopStatus::Abort(n) => {
                        dbg_log!("Loop detector: abort after {} repeats", n);
                        let mut s = state.lock().await;
                        s.history
                            .push(ChatMessage::new("assistant", &final_content));
                        s.history.push(ChatMessage::new(
                            "system",
                            format!(
                                "[Loop detected: the same action repeated {n} times without \
                                 making progress. Automatic execution stopped. Try a different \
                                 approach, or tell the user what you found and ask how to proceed.]"
                            ),
                        ));
                        crate::config::save_history(&s.history);
                        s.current_response.clear();
                        s.continuous_mode = false;
                        s.status = AppStatus::Idle;
                        drop(s);
                        let _ = crate::notifications::notify_finished(
                            crate::notifications::FinishedStatus::Success,
                        );
                        break;
                    }
                    loop_detect::LoopStatus::Warning(n) => {
                        dbg_log!("Loop detector: warning at {} repeats", n);
                        let mut s = state.lock().await;
                        s.history.push(ChatMessage::new(
                            "system",
                            format!(
                                "[Loop warning: this action has repeated {n} times. If you are \
                                 stuck, change your approach — a different query, a different tool, \
                                 or summarize what you have and move on.]"
                            ),
                        ));
                        drop(s);
                    }
                    loop_detect::LoopStatus::Ok => {}
                }

                if !cancel_token.is_cancelled() && tool_rounds < max_tool_rounds {
                    tool_rounds += 1;

                    // Gather all confirmations needed
                    let mut confirmations = Vec::new();
                    let auto_confirm = { state.lock().await.auto_confirm };

                    if !auto_confirm {
                        for (name, args) in &tool_calls {
                            if crate::tools::needs_confirmation(name)
                                && !crate::tools::is_agent_tool(name)
                            {
                                let path =
                                    if let Some(p) = args.get("path").and_then(|p| p.as_str()) {
                                        p.to_string()
                                    } else if let Some(cmd) =
                                        args.get("command").and_then(|c| c.as_str())
                                    {
                                        cmd.to_string()
                                    } else if let (Some(src), Some(dest)) = (
                                        args.get("src").and_then(|s| s.as_str()),
                                        args.get("dest").and_then(|d| d.as_str()),
                                    ) {
                                        format!("{src} -> {dest}")
                                    } else {
                                        "?".to_string()
                                    };

                                let diff_opt = get_diff_preview(name, args);
                                let (preview, content_bytes) = if let Some(ref d) = diff_opt {
                                    (d.clone(), d.len())
                                } else {
                                    let content =
                                        args.get("content").and_then(|c| c.as_str()).unwrap_or("");
                                    let preview =
                                        content.lines().take(6).collect::<Vec<_>>().join("\n");
                                    (preview, content.len())
                                };

                                confirmations.push(crate::app::ToolConfirmation {
                                    tool_name: name.clone(),
                                    path,
                                    content_preview: preview,
                                    content_bytes,
                                });
                            }
                        }
                    }

                    let mut approved = true;
                    if !confirmations.is_empty() {
                        let (tx, rx) = tokio::sync::oneshot::channel::<bool>();
                        {
                            let mut s = state.lock().await;
                            s.pending_tool_confirmation = Some(confirmations);
                            s.tool_confirmation_response = Some(tx);
                            s.status = AppStatus::AwaitingToolConfirmation;
                        }

                        let first_tool_name = &tool_calls[0].0;
                        let _ = crate::notifications::notify_pending_confirmation(first_tool_name);

                        dbg_log!(
                            "Awaiting user batch confirmation for {} tools",
                            tool_calls.len()
                        );
                        approved = match rx.await {
                            Ok(true) => {
                                dbg_log!("User approved batch tool calls");
                                true
                            }
                            Ok(false) => {
                                dbg_log!("User denied batch tool calls");
                                let _ = crate::notifications::notify_finished(
                                    crate::notifications::FinishedStatus::Denied,
                                );
                                false
                            }
                            Err(_) => {
                                dbg_log!("Confirmation channel closed during batch confirmation");
                                false
                            }
                        };
                    }

                    // Update UI state immediately after confirmation is resolved
                    {
                        let mut s = state.lock().await;
                        s.pending_tool_confirmation = None;
                        s.status = AppStatus::Streaming;
                        s.stream_tracker = Some(StreamTracker::new());
                        s.history
                            .push(ChatMessage::new("assistant", &final_content));
                        crate::config::save_history(&s.history);
                    }

                    let results = if !approved {
                        tool_calls
                            .iter()
                            .map(|(name, _)| {
                                (
                                    name.clone(),
                                    "error: user denied this tool call".to_string(),
                                    None,
                                )
                            })
                            .collect::<Vec<_>>()
                    } else {
                        dbg_log!("Executing {} tool calls in parallel", tool_calls.len());
                        let mut futures = Vec::new();
                        for (name, args) in &tool_calls {
                            let client_clone = client.clone();
                            let state_clone = Arc::clone(&state);
                            let cancel_token_clone = cancel_token.clone();
                            let name_clone = name.clone();
                            let args_clone = args.clone();

                            let fut = async move {
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
                                        if let Some(p) =
                                            args_clone.get("path").and_then(|p| p.as_str())
                                        {
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
                                                is_repeat =
                                                    view_file_unchanged_since_last_read(
                                                        stored, current,
                                                    );
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
                                        "You already ran this call recently. For a file you just \
                                         edited, its new contents are already reflected by your \
                                         edit tool's result — re-reading is not needed. Otherwise \
                                         pick a different action, change the query, or summarize/finish."
                                            .to_string(),
                                        None,
                                    )
                                } else if crate::tools::is_agent_tool(&name_clone)
                                {
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
                                    )
                                    .await
                                };

                                // Record this call so future identical read-only calls are caught.
                                {
                                    let mut s = state_clone.lock().await;
                                    if let Some(p) = view_path {
                                        if !is_repeat {
                                            if let Some(mt) = view_mtime {
                                                s.read_file_mtimes.insert(p, mt);
                                            } else {
                                                // File couldn't be stat'd (e.g. already gone);
                                                // drop any stale entry so a later read is allowed.
                                                s.read_file_mtimes.remove(&p);
                                            }
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
                            };
                            futures.push(fut);
                        }
                        join_all(futures).await
                    };

                    if cancel_token.is_cancelled() {
                        dbg_log!("Orchestrator: Cancelled during tool execution");
                        let mut s = state.lock().await;
                        s.status = AppStatus::Idle;
                        break;
                    }

                    let mut s = state.lock().await;
                    s.status = AppStatus::Streaming;
                    let mut completed = false;
                    for (name, result, diff_opt) in results {
                        dbg_log!(
                            "Tool '{}' finished with result length: {} chars",
                            name,
                            result.len()
                        );
                        if name == "complete_task" {
                            completed = true;
                        }
                        // Output-stagnation signal: repeated identical results
                        // (e.g. "No matches found") despite varied commands.
                        if let loop_detect::LoopStatus::Warning(n) | loop_detect::LoopStatus::Abort(n) =
                            loop_detector.record_output(&result)
                        {
                            dbg_log!("Loop detector: output stagnation x{} for '{}'", n, name);
                        }
                        let truncated_result = truncate_tool_output(&name, result);
                        s.history.push(
                            ChatMessage::new("tool", format!("{name}: {truncated_result}"))
                                .with_diff(diff_opt),
                        );
                    }
                    if completed {
                        dbg_log!("complete_task called, turning off continuous mode");
                        s.continuous_mode = false;
                    }
                    crate::config::save_history(&s.history);
                    s.current_response.clear();
                    drop(s);
                    dbg_log!("Tool round finished, looping back");
                    continue;
                } else {
                    dbg_log!("Tool rounds exceeded MAX_TOOL_ROUNDS or cancelled");
                    if !cancel_token.is_cancelled() && tool_rounds >= max_tool_rounds
                    {
                        limit_reached = true;
                    }
                    break;
                }
            } else if has_intended_tool_call(&final_content)
                && tool_rounds < max_tool_rounds
            {
                dbg_log!(
                    "Orchestrator: Detected malformed tool call, auto-correcting and retrying..."
                );
                tool_rounds += 1;
                let mut s = state.lock().await;
                s.history
                    .push(ChatMessage::new("assistant", &final_content));

                let feedback = "tool_error: The tool call block was malformed or could not be parsed. \
Please output a single, complete, valid tool call block inside a ```tool fenced block using JSON format:\n\n\
```tool\n\
{\"name\": \"tool_name\", \"arguments\": {...}}\n\
```\n\n\
Make sure keys are exactly \"name\" and \"arguments\", and do not wrap numbers/booleans in quotes if they are expected as numbers/booleans.";

                s.history
                    .push(ChatMessage::new("tool", feedback.to_string()));
                crate::config::save_history(&s.history);
                s.current_response.clear();
                s.status = AppStatus::Streaming;
                s.stream_tracker = Some(StreamTracker::new());
                drop(s);
                dbg_log!("Retrying agent loop round due to malformed tool call");
                continue;
            }

            let is_continuous = { state.lock().await.continuous_mode };
            if is_continuous && !cancel_token.is_cancelled() && tool_rounds > 0 && tool_rounds < max_tool_rounds {
                dbg_log!("Continuous mode active, assistant didn't call complete_task. Injecting prompt to continue.");
                tool_rounds += 1;
                let mut s = state.lock().await;
                s.history.push(ChatMessage::new("assistant", final_content.clone()));
                s.history.push(ChatMessage::new(
                    "system",
                    "[System Reminder: Continuous mode is active. You have not called the 'complete_task' tool yet. If you are finished, you must call the 'complete_task' tool to end. If you are still working, continue executing the necessary tools to achieve the goal.]".to_string()
                ));
                crate::config::save_history(&s.history);
                s.current_response.clear();
                s.status = AppStatus::Streaming;
                s.stream_tracker = Some(StreamTracker::new());
                drop(s);
                continue;
            } else if is_continuous && tool_rounds == 0 {
                dbg_log!("Continuous mode active, but assistant gave a plain conversational reply (no tools used). Ending turn.");
                let mut s = state.lock().await;
                s.continuous_mode = false;
            }

            break;
        }

        if !final_content.is_empty() {
            dbg_log!("Finishing agent loop, writing final assistant reply");

            let mut s = state.lock().await;
            s.continuous_mode = false;
            s.response_time = Some(prompt_start_time.elapsed());
            let mut msg = ChatMessage::new("assistant", final_content.clone());
            msg.response_time_ms = s.response_time.map(|d| d.as_millis() as u64);
            s.history.push(msg);

            if limit_reached {
                s.history.push(ChatMessage::new(
                    "system",
                    format!(
                        "⚠ Tool execution limit ({} rounds) reached. Type a message to continue or summarize.",
                        max_tool_rounds
                    ),
                ));
            }

            drop(s);

            let usage = {
                let s = state.lock().await;
                if s.current_token_usage.is_some() {
                    s.current_token_usage.clone()
                } else {
                    drop(s);
                    dbg_log!("Estimating token usage...");
                    let est = estimate_token_usage(&last_sent_messages, &final_content).await;
                    dbg_log!("Token usage estimation result: {:?}", est);
                    est
                }
            };

            let mut s = state.lock().await;
            if let Some(msg) = s.history.iter_mut().rev().find(|m| m.role == "assistant") {
                msg.token_usage = usage.clone();
            }

            crate::config::save_history(&s.history);
            let active_id = s.active_session_id.clone();
            crate::config::save_session_history(&active_id, &s.history);

            s.current_response.clear();
            s.status = AppStatus::Idle;

            if let Some(u) = &usage {
                crate::config::track_usage(u.prompt_tokens as u64, u.completion_tokens as u64);
            }
            s.current_token_usage = usage;
            drop(s);

            // Notify the user that the agent loop completed successfully.
            let _ = crate::notifications::notify_finished(
                crate::notifications::FinishedStatus::Success,
            );
        }

        if cancel_token.is_cancelled() {
            dbg_log!("Cancel token is cancelled, exiting orchestrator loop");
            // Best-effort: notify the user that a cancellation happened.
            let _ = crate::notifications::notify_finished(
                crate::notifications::FinishedStatus::Cancelled,
            );
            break;
        }
    }
    dbg_log!("Orchestrator finished");
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
        assert!(msgs2[3]["content"].as_str().unwrap().contains("REMINDER: You are rustcode."));
        assert!(msgs2[3]["content"].as_str().unwrap().contains("tell me a story"));
    }

    #[test]
    fn test_strip_leading_think() {
        assert_eq!(
            strip_leading_think("<think>\nreasoning here\n</think>\n\nfinal answer"),
            "final answer"
        );
        assert_eq!(strip_leading_think("plain reply"), "plain reply");
        // </think> mentioned mid-text without a leading block: untouched
        assert_eq!(
            strip_leading_think("text about </think> tags"),
            "text about </think> tags"
        );
    }

    #[test]
    fn test_is_reasoning_only() {
        // pure reasoning, no answer → stall we want to auto-continue
        assert!(is_reasoning_only("<think>\nlet me plan\n</think>"));
        assert!(is_reasoning_only("<think>plan</think>\n\n  \n"));
        // reasoning followed by a real answer → complete
        assert!(!is_reasoning_only(
            "<think>plan</think>\n\nhere is the answer"
        ));
        // reasoning followed by a tool call → complete
        assert!(!is_reasoning_only(
            "<think>plan</think>\n```tool\n{\"name\":\"get_time\"}\n```"
        ));
        assert!(!is_reasoning_only(
            "<think>plan</think>\n<tool_call>{\"name\":\"get_time\"}</tool_call>"
        ));
        // empty content is handled by the caller, not treated as reasoning-only
        assert!(!is_reasoning_only("   "));
        assert!(!is_reasoning_only("just a normal reply"));
    }

    #[test]
    fn test_is_cut_off_reasoning_only() {
        assert!(is_cut_off("<think>thinking</think>", None));
        assert!(!is_cut_off("<think>thinking</think>\n\nthe answer", None));
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
        )
        .await;
        assert!(
            result.contains("wrote")
                || result.contains("created")
                || result.contains("test_bypass.txt")
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
    fn test_view_file_path_extraction() {
        let content = "view_file: [File: src/main.rs, Lines 1 to 500 of 1128, Bytes offset: 0]\n1: mod app;";
        assert_eq!(
            view_file_path_from_tool_msg(content),
            Some("src/main.rs".to_string())
        );
        // Stubs and non-view_file messages yield None.
        assert_eq!(
            view_file_path_from_tool_msg("view_file: [Tool output truncated: 123 pruned]"),
            None
        );
        assert_eq!(view_file_path_from_tool_msg("grep: some match"), None);
        assert_eq!(view_file_path_from_tool_msg(""), None);
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
            classify_tool_msg(&ChatMessage::new("tool", "check_match: 2-1")),
            Some("other")
        );
        assert_eq!(classify_tool_msg(&ChatMessage::new("assistant", "hi")), None);
    }

    #[test]
    fn test_dedupe_view_file_reads_keeps_newest() {
        let mk = |content: &str| ChatMessage::new("tool", content);
        let vf = |path: &str| {
            format!("view_file: [File: {path}, Lines 1 to 10 of 10]\n1: a\n2: b")
        };
        let mut history = vec![
            mk(&vf("src/main.rs")), // 0: old read -> stubbed
            mk("grep: match"),      // 1: untouched
            mk(&vf("src/main.rs")), // 2: superseded -> stubbed
            mk(&vf("src/other.rs")), // 3: sole read -> kept
            mk(&vf("src/main.rs")), // 4: newest read -> kept
        ];
        dedupe_view_file_reads(&mut history);
        assert!(history[0].content.contains("[superseded"));
        assert_eq!(history[1].content, "grep: match");
        assert!(history[2].content.contains("[superseded"));
        assert!(!history[3].content.contains("[superseded"));
        assert!(!history[4].content.contains("[superseded"));
        assert!(history[4].content.contains("src/main.rs"));
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

    #[test]
    fn test_strip_ansi_escapes() {
        let input = "\x1B[31mError\x1B[0m: compile failed \x1B[1mline 5\x1B[0m";
        let output = strip_ansi_escapes(input);
        assert_eq!(output, "Error: compile failed line 5");
    }

    #[tokio::test]
    async fn test_run_compiler_check_success() {
        let cwd = std::env::current_dir().unwrap();
        let check = run_compiler_check(&cwd).await;
        assert!(check.is_none());
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
}
