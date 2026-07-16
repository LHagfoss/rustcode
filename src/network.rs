use crate::app::{AppState, AppStatus, ChatMessage, StreamTracker, TokenUsage, ToolConfirmation};
use futures_util::{StreamExt, future::join_all};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Mutex;
use tokio_util::io::StreamReader;

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

async fn count_tokens(text: &str) -> Option<u32> {
    if text.trim().is_empty() {
        return Some(0);
    }
    let mut cmd = tokio::process::Command::new("fm");
    cmd.args(["token-count", "--quiet", text])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());

    let child = cmd.spawn().ok()?;
    let output_res =
        tokio::time::timeout(std::time::Duration::from_secs(2), child.wait_with_output()).await;

    let output = output_res.ok()?.ok()?;
    if !output.status.success() {
        return None;
    }
    std::str::from_utf8(&output.stdout)
        .ok()?
        .trim()
        .parse::<u32>()
        .ok()
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
        let t = count_tokens(&truncated)
            .await
            .unwrap_or((truncated.len() / 4) as u32);
        m.content = truncated;
        t
    } else {
        let stubbed = format!(
            "{}: [Tool output truncated: {} tokens pruned to maintain context window]",
            tool_name, current_tokens
        );
        count_tokens(&stubbed)
            .await
            .unwrap_or((stubbed.len() / 4) as u32)
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
        let t = count_tokens(&m.content)
            .await
            .unwrap_or_else(|| (m.content.len() / 4) as u32);
        tokens.push(t);
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
    let prompt = count_tokens(&prompt_text).await?;
    let full = prompt_text + reply + "\n";
    let total = count_tokens(&full).await?;
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
        let reminder = serde_json::json!({
            "role": "system",
            "content": "REMINDER: You are rustcode. Always follow your core instructions:\n\
            - Be extremely concise and direct. No filler or preamble.\n\
            - To call a tool, output exactly one fenced `tool` block containing a single JSON object. Do not output any conversational text or narration before or after the block.\n\
            - Available tools: view_file, replace_file_content, multi_replace_file_content, write_to_file, delete_file, move_file, copy_file, list_directory, grep, glob, run_command, search_web, find_symbol, get_project_map."
        });
        let last_idx = msgs.len() - 1;
        msgs.insert(last_idx, reminder);
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
            match args.get("end_line").and_then(|v| v.as_u64()) {
                Some(e) => {
                    let start = args.get("start_line").and_then(|v| v.as_u64()).unwrap_or(1);
                    format!("{path}|{start}-{e}")
                }
                None => format!("{path}|full"),
            }
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
    let payload = serde_json::json!({
        "model": model,
        "messages": messages,
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

    let response = client.post(url).json(&payload).send().await.map_err(|e| {
        let mut msg = format!("Request failed: {e}");
        let mut src = std::error::Error::source(&e);
        while let Some(cause) = src {
            msg.push_str(&format!(": {cause}"));
            src = cause.source();
        }
        msg
    })?;

    dbg_log!(
        "stream_request: Received response status: {}",
        response.status()
    );

    if !response.status().is_success() {
        let status = response.status();
        let err_body = response.text().await.unwrap_or_default();
        dbg_log!(
            "stream_request: Request failed with status {}. Body: {}",
            status,
            err_body
        );
        return Err(format!("{status} - {err_body}"));
    }

    let stream = response
        .bytes_stream()
        .map(|r| r.map_err(std::io::Error::other));
    let wrapped = StreamReader::new(stream);
    let mut reader = BufReader::with_capacity(4096, wrapped);
    let mut line_buf = String::with_capacity(4096);
    let mut line_count = 0;
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
                    Ok(n) => {
                        line_count += 1;
                        let trimmed = line_buf.trim();
                        if line_count % 50 == 0 || trimmed.starts_with("data:") || trimmed.is_empty() {

                            dbg_log!("stream_request: Read SSE line {} ({} bytes): '{}'", line_count, n, trimmed);
                        }
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
    lower.contains("```tool") || lower.contains("```json")
}

fn is_cut_off(content: &str, finish_reason: Option<&str>) -> bool {
    // If the model already produced a valid tool call, we don't need to continue text generation.
    // We should execute the tool and get its output first.
    if !crate::tools::parse_tool_calls(content, crate::config::ToolProtocol::Json).is_empty() {
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
    if name == "edit" {
        let search_block = args
            .get("search_block")
            .and_then(|s| s.as_str())
            .unwrap_or("");
        let replace_block = args
            .get("replace_block")
            .and_then(|s| s.as_str())
            .unwrap_or("");

        let diff = similar::TextDiff::from_lines(search_block, replace_block);
        let mut prev = String::new();
        for change in diff.iter_all_changes() {
            match change.tag() {
                similar::ChangeTag::Delete => {
                    prev.push('-');
                    prev.push_str(change.value());
                }
                similar::ChangeTag::Insert => {
                    prev.push('+');
                    prev.push_str(change.value());
                }
                similar::ChangeTag::Equal => {
                    prev.push(' ');
                    prev.push_str(change.value());
                }
            }
        }
        Some(prev)
    } else if name == "write_file" || name == "create_file" {
        let path = args.get("path").and_then(|p| p.as_str()).unwrap_or("");
        let old_content = std::fs::read_to_string(&path).unwrap_or_default();
        let new_content = args.get("content").and_then(|c| c.as_str()).unwrap_or("");

        let diff = similar::TextDiff::from_lines(&old_content, new_content);
        let mut prev = String::new();
        for group in diff.grouped_ops(3) {
            for op in group {
                for change in diff.iter_changes(&op) {
                    match change.tag() {
                        similar::ChangeTag::Delete => {
                            prev.push('-');
                            prev.push_str(change.value());
                        }
                        similar::ChangeTag::Insert => {
                            prev.push('+');
                            prev.push_str(change.value());
                        }
                        similar::ChangeTag::Equal => {
                            prev.push(' ');
                            prev.push_str(change.value());
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
    if !needs_confirm {
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

        let res = tokio::select! {
            res = run_fut => {
                res.unwrap_or_else(|e| format!("tool panicked: {e}"))
            }
            _ = cancel_token.cancelled() => {
                dbg_log!("Tool execution cancelled during spawn_blocking await (immediate execution)");
                "error: tool execution cancelled by user".to_string()
            }
        };
        return (res, diff_opt);
    }

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
    let result = match rx.await {
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
                a.history.push(
                    ChatMessage::new("tool", format!("{name}: {result}")).with_diff(diff_opt),
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
Given a user's coding prompt, determine if it is a complex or multi-step task that requires an autonomous agent loop (executing multiple tools to search, edit, compile, or test code) to complete.\n\
If it is a multi-step task, expand the prompt to be detailed and structured, but do not override the user's original intent.\n\
Return ONLY a valid JSON object matching this schema: {\"is_goal\": bool, \"expanded_prompt\": \"string\"}. No markdown formatting, no code fences.";

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
        Some((parsed.is_goal, parsed.expanded_prompt))
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
        loop {
            dbg_log!("Starting agent loop round {}", tool_rounds);

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

            let mut system_prompt = format!(
                "{}\n\n{}",
                crate::tools::tool_system_prompt(true, crate::config::ToolProtocol::Json),
                crate::context::environment_context()
            );

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
                                            let current = path_mtime(p);
                                            let stored = {
                                                let s = state_clone.lock().await;
                                                s.read_file_mtimes.get(p).copied()
                                            };
                                            is_repeat =
                                                view_file_unchanged_since_last_read(
                                                    stored, current,
                                                );
                                            view_path = Some(p.to_string());
                                            view_mtime = current;
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
                                    } else if is_read_only && !is_repeat {
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
                        s.history.push(
                            ChatMessage::new("tool", format!("{name}: {result}"))
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
            if is_continuous && !cancel_token.is_cancelled() && tool_rounds < max_tool_rounds {
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

        // 4 or more messages: reminder is injected right before the last message
        let mut msgs2: Vec<serde_json::Value> = vec![
            serde_json::json!({"role": "system", "content": "sys"}),
            serde_json::json!({"role": "user", "content": "hello"}),
            serde_json::json!({"role": "assistant", "content": "hi"}),
            serde_json::json!({"role": "user", "content": "tell me a story"}),
        ];
        inject_system_reminder(&mut msgs2);
        assert_eq!(msgs2.len(), 5);
        assert_eq!(msgs2[3]["role"], "system");
        assert!(msgs2[3]["content"].as_str().unwrap().contains("REMINDER: You are rustcode."));
        assert_eq!(msgs2[4]["role"], "user");
        assert_eq!(msgs2[4]["content"], "tell me a story");
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
}
