use crate::app::{AppState, AppStatus, ChatMessage, TokenUsage, ToolConfirmation};
use futures_util::StreamExt;
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
    let t = text.to_string();
    tokio::task::spawn_blocking(move || {
        let out = std::process::Command::new("fm")
            .args(["token-count", "--quiet", &t])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        std::str::from_utf8(&out.stdout)
            .ok()?
            .trim()
            .parse::<u32>()
            .ok()
    })
    .await
    .ok()?
}

async fn estimate_token_usage(history_before: &[ChatMessage], reply: &str) -> Option<TokenUsage> {
    let mut prompt_text = String::new();
    for msg in history_before {
        if matches!(msg.role.as_str(), "user" | "assistant" | "tool")
            && !msg.content.starts_with('/')
        {
            prompt_text.push_str(&msg.content);
            prompt_text.push('\n');
        }
    }
    let prompt = count_tokens(&prompt_text).await?;
    let full = prompt_text + reply + "\n";
    let total = count_tokens(&full).await?;
    Some(TokenUsage {
        prompt_tokens: prompt,
        completion_tokens: total.saturating_sub(prompt),
        total_tokens: total,
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
) -> Option<u32> {
    let base = chat_url.strip_suffix("/v1/chat/completions")?;
    let show_url = format!("{base}/api/show");
    let resp = client
        .post(&show_url)
        .json(&serde_json::json!({"model": model}))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body: serde_json::Value = resp.json().await.ok()?;
    context_length_from_model_info(body.get("model_info")?)
}

/// Tokens reserved for the model's reply when budgeting the request.
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
) -> Result<(), String> {
    let payload = serde_json::json!({
        "model": model,
        "messages": messages,
        "stream": true,
        "temperature": 0.7,
        "max_tokens": 4096,
    });
    dbg_log!("stream_request: Request payload: {}", serde_json::to_string_pretty(&payload).unwrap_or_default());

    let response = client
        .post(url)
        .json(&payload)
        .send()
        .await
        .map_err(|e| {
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

    dbg_log!("stream_request: Starting SSE stream read loop");
    loop {
        if cancel_token.is_cancelled() {
            dbg_log!("stream_request: Stream reading cancelled via token");
            return Ok(());
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
                                        let delta = choices[0].get("delta");
                                        let reasoning = delta
                                            .and_then(|d| d.get("reasoning").or_else(|| d.get("reasoning_content")))
                                            .and_then(|r| r.as_str());
                                        let content = delta
                                            .and_then(|d| d.get("content").or_else(|| d.get("text")))
                                            .and_then(|c| c.as_str());

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
                                        state.lock().await.current_token_usage = Some(TokenUsage {
                                            prompt_tokens: p as u32,
                                            completion_tokens: c as u32,
                                            total_tokens: t as u32,
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
                return Ok(());
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

    let mut buf = buffer.lock().await;
    buf.content = buf
        .content
        .trim_end_matches(char::is_whitespace)
        .to_string();
    dbg_log!(
        "stream_request: Stream request loop ended. Total content: {} chars",
        buf.content.len()
    );
    Ok(())
}

/// Show the Y/N confirmation modal (when the tool requires it) and run the
/// tool. `display_name` is what the modal shows — subagent calls prefix it
/// with the agent id so the user knows who is asking.
async fn confirm_and_execute(
    state: &Arc<Mutex<AppState>>,
    name: &str,
    args: &serde_json::Value,
    display_name: &str,
) -> String {
    let needs_confirm =
        crate::tools::needs_confirmation(name) && !state.lock().await.auto_confirm;
    if !needs_confirm {
        dbg_log!("Executing tool '{}' immediately...", name);
        return crate::tools::execute(name, args);
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
    let content = args.get("content").and_then(|c| c.as_str()).unwrap_or("");
    let preview: String = content.lines().take(6).collect::<Vec<_>>().join("\n");
    let (tx, rx) = tokio::sync::oneshot::channel::<bool>();
    {
        let mut s = state.lock().await;
        s.pending_tool_confirmation = Some(ToolConfirmation {
            tool_name: display_name.to_string(),
            path,
            content_preview: preview,
            content_bytes: content.len(),
        });
        s.tool_confirmation_response = Some(tx);
        s.status = AppStatus::AwaitingToolConfirmation;
    }
    dbg_log!("Awaiting user confirmation for '{}'", name);
    let result = match rx.await {
        Ok(true) => {
            dbg_log!("User approved tool call '{}', executing...", name);
            crate::tools::execute(name, args)
        }
        Ok(false) => {
            dbg_log!("User denied tool call '{}'", name);
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
    }
    result
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
    match (text.trim_start().starts_with("<think>"), text.find("</think>")) {
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
        let history_snapshot: Vec<ChatMessage> = {
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

        let system_prompt = format!(
            "{}\n\nYou are subagent {agent_id}, working for a main agent in the same \
rustcode session. Complete the task you were given, then reply in plain text \
with NO tool call — that reply is returned to the main agent. Keep the final \
reply compact and information-dense.\n\n{}",
            crate::tools::tool_system_prompt(false),
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

        stream_buffer.lock().await.content.clear();
        let (api_base_url, model_name) = {
            let s = state.lock().await;
            (s.api_base_url.clone(), s.model_name.clone())
        };
        dbg_log!("subagent {} round {}: requesting {}", agent_id, rounds, model_name);
        if let Err(e) = stream_request(
            client,
            Arc::clone(state),
            cancel_token.clone(),
            &api_base_url,
            &model_name,
            &msgs,
            Arc::clone(&stream_buffer),
            true,
        )
        .await
        {
            return format!("error: subagent request failed: {e}");
        }
        let content = stream_buffer.lock().await.content.clone();
        if content.is_empty() {
            return "error: subagent returned an empty reply".to_string();
        }

        if let Some((name, args)) = crate::tools::parse_tool_call(&content) {
            if rounds >= MAX_SUBAGENT_ROUNDS {
                return format!(
                    "error: subagent {agent_id} hit the {MAX_SUBAGENT_ROUNDS}-round tool limit without a final reply"
                );
            }
            rounds += 1;
            let result = if crate::tools::is_agent_tool(&name) {
                "error: subagents cannot spawn or message other agents".to_string()
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
                confirm_and_execute(state, &name, &args, &format!("agent-{agent_id} · {name}"))
                    .await
            };
            let mut s = state.lock().await;
            if let Some(a) = s.subagents.iter_mut().find(|a| a.id == agent_id) {
                a.history.push(ChatMessage::new("assistant", &content));
                a.history.push(ChatMessage::new("tool", format!("{name}: {result}")));
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
            let agent_id = {
                let mut s = state.lock().await;
                let id = s.next_subagent_id;
                s.next_subagent_id += 1;
                s.subagents.push(crate::app::SubAgent {
                    id,
                    task: task.to_string(),
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
                    let known: Vec<String> =
                        s.subagents.iter().map(|a| a.id.to_string()).collect();
                    return if known.is_empty() {
                        "error: no subagents exist — use spawn_agent first".to_string()
                    } else {
                        format!("error: no subagent with id {id}. Known ids: {}", known.join(", "))
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
        _ => format!("error: unknown agent tool '{name}'"),
    }
}

pub async fn process_queue_orchestrator(
    client: reqwest::Client,
    state: Arc<Mutex<AppState>>,
    cancel_token: tokio_util::sync::CancellationToken,
) {
    dbg_log!("Orchestrator started");
    loop {
        let next_prompt = {
            let mut s = state.lock().await;
            if s.pending_queue.is_empty() {
                dbg_log!("Pending queue empty, setting status to Idle");
                s.status = AppStatus::Idle;
                break;
            }
            s.status = AppStatus::Streaming;
            let prompt = s.pending_queue.remove(0);
            dbg_log!("Popped prompt from queue: '{}'", prompt);
            prompt
        };

        let stream_buffer = Arc::new(Mutex::new(StreamBuffer {
            content: String::new(),
        }));
        {
            let mut s = state.lock().await;
            s.history.push(ChatMessage::new("user", next_prompt));
            crate::config::save_history(&s.history);
            s.current_response.clear();
            s.current_token_usage = None;
            s.response_time = None;
        }

        let prompt_start_time = std::time::Instant::now();

        let mut tool_rounds = 0;
        loop {
            dbg_log!("Starting agent loop round {}", tool_rounds);

            let history_snapshot: Vec<ChatMessage> = {
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

            let system_prompt = format!(
                "{}\n\n{}",
                crate::tools::tool_system_prompt(true),
                crate::context::environment_context()
            );
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
            let stream_result = stream_request(
                &client,
                Arc::clone(&state),
                cancel_token.clone(),
                &api_base_url,
                &model_name,
                &msgs,
                Arc::clone(&stream_buffer),
                false,
            )
            .await;

            if let Err(e) = stream_result {
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

            let final_content = stream_buffer.lock().await.content.clone();
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

            if let Some((name, args)) = crate::tools::parse_tool_call(&final_content) {
                dbg_log!("Parsed tool call request: '{}' with args: {:?}", name, args);
                if !cancel_token.is_cancelled() && tool_rounds < crate::tools::MAX_TOOL_ROUNDS {
                    tool_rounds += 1;

                    let result = if crate::tools::is_agent_tool(&name) {
                        handle_agent_tool(&client, &state, &cancel_token, &name, &args).await
                    } else {
                        confirm_and_execute(&state, &name, &args, &name).await
                    };
                    dbg_log!(
                        "Tool '{}' finished with result length: {} chars",
                        name,
                        result.len()
                    );

                    let mut s = state.lock().await;
                    s.pending_tool_confirmation = None;
                    s.status = AppStatus::Streaming;
                    s.history
                        .push(ChatMessage::new("assistant", &final_content));
                    s.history
                        .push(ChatMessage::new("tool", format!("{name}: {result}")));
                    crate::config::save_history(&s.history);
                    s.current_response.clear();
                    drop(s);
                    dbg_log!("Tool round finished, looping back");
                    continue;
                } else {
                    dbg_log!("Tool rounds exceeded MAX_TOOL_ROUNDS or cancelled");
                }
            }

            dbg_log!("Finishing agent loop, writing final assistant reply");
            let history_before = {
                let s = state.lock().await;
                s.history.clone()
            };

            let mut s = state.lock().await;
            s.response_time = Some(prompt_start_time.elapsed());
            let mut msg = ChatMessage::new("assistant", final_content.clone());
            msg.response_time_ms = s.response_time.map(|d| d.as_millis() as u64);
            s.history.push(msg);
            crate::config::save_history(&s.history);
            s.current_response.clear();
            s.status = AppStatus::Idle;
            drop(s);

            dbg_log!("Estimating token usage...");
            let usage = estimate_token_usage(&history_before, &final_content).await;
            dbg_log!("Token usage estimation result: {:?}", usage);
            state.lock().await.current_token_usage = usage;
            break;
        }

        if cancel_token.is_cancelled() {
            dbg_log!("Cancel token is cancelled, exiting orchestrator loop");
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
    fn test_parse_multimodal_content_plain() {
        let val = parse_multimodal_content("Hello world");
        assert_eq!(val, serde_json::Value::String("Hello world".to_string()));
    }

    #[test]
    fn test_parse_multimodal_content_with_image_nonexistent() {
        let val = parse_multimodal_content("Look at this: ![image](file:///nonexistent/path.png) interesting!");
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
}
