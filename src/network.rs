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

struct StreamBuffer {
    content: String,
}

async fn stream_request(
    client: &reqwest::Client,
    state: Arc<Mutex<AppState>>,
    cancel_token: tokio_util::sync::CancellationToken,
    url: &str,
    model: &str,
    messages: &[serde_json::Value],
    buffer: Arc<Mutex<StreamBuffer>>,
) -> Result<(), String> {
    dbg_log!("stream_request: Sending POST request to {}", url);
    let response = client
        .post(url)
        .json(&serde_json::json!({
            "model": model,
            "messages": messages,
            "stream": true,
            "temperature": 0.7,
            "max_tokens": 4096,
        }))
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

                                        let mut buf = buffer.lock().await;
                                        let mut s = state.lock().await;

                                        if let Some(r_token) = reasoning {
                                            if !in_reasoning {
                                                in_reasoning = true;
                                                buf.content.push_str("<think>\n");
                                                s.current_response.push_str("<think>\n");
                                            }
                                            buf.content.push_str(r_token);
                                            s.current_response.push_str(r_token);
                                        } else if let Some(c_token) = content {
                                            if in_reasoning {
                                                in_reasoning = false;
                                                buf.content.push_str("\n</think>\n\n");
                                                s.current_response.push_str("\n</think>\n\n");
                                            }
                                            buf.content.push_str(c_token);
                                            s.current_response.push_str(c_token);
                                        }
                                    }
                                }
                                if let Some(usage) = val.get("usage") {
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
        let mut buf = buffer.lock().await;
        let mut s = state.lock().await;
        buf.content.push_str("\n</think>\n\n");
        s.current_response.push_str("\n</think>\n\n");
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

            let system_prompt = crate::tools::tool_system_prompt();
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
                        "content": format!("System Instructions:\n{}\n\nUser request: {}", system_prompt, m.content),
                    })
                } else {
                    serde_json::json!({"role": m.role, "content": m.content})
                }
            }));

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

                    let needs_confirm =
                        crate::tools::needs_confirmation(&name) && !state.lock().await.auto_confirm;

                    let result = if needs_confirm {
                        dbg_log!("Tool '{}' requires confirmation", name);
                        let path = if let Some(p) = args.get("path").and_then(|p| p.as_str()) {
                            p.to_string()
                        } else if let (Some(src), Some(dest)) = (
                            args.get("src").and_then(|s| s.as_str()),
                            args.get("dest").and_then(|d| d.as_str()),
                        ) {
                            format!("{src} -> {dest}")
                        } else {
                            "?".to_string()
                        };
                        let content = args.get("content").and_then(|c| c.as_str()).unwrap_or("");
                        let preview: String =
                            content.lines().take(6).collect::<Vec<_>>().join("\n");
                        let (tx, rx) = tokio::sync::oneshot::channel::<bool>();
                        {
                            let mut s = state.lock().await;
                            s.pending_tool_confirmation = Some(ToolConfirmation {
                                tool_name: name.clone(),
                                path: path.clone(),
                                content_preview: preview,
                                content_bytes: content.len(),
                            });
                            s.tool_confirmation_response = Some(tx);
                            s.status = AppStatus::AwaitingToolConfirmation;
                        }
                        dbg_log!("Awaiting user confirmation for '{}'", name);

                        match rx.await {
                            Ok(true) => {
                                dbg_log!("User approved tool call '{}', executing...", name);
                                crate::tools::execute(&name, &args)
                            }
                            Ok(false) => {
                                dbg_log!("User denied tool call '{}'", name);
                                "error: user denied this tool call".to_string()
                            }
                            Err(_) => {
                                dbg_log!("Confirmation channel closed for '{}'", name);
                                "error: confirmation channel closed".to_string()
                            }
                        }
                    } else {
                        dbg_log!("Executing tool '{}' immediately...", name);
                        let execute_result = crate::tools::execute(&name, &args);
                        dbg_log!(
                            "Tool '{}' execution finished with length: {} chars",
                            name,
                            execute_result.len()
                        );
                        execute_result
                    };

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
