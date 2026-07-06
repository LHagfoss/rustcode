//! Network layer: SSE streaming, tool-calling agent loop, CLI tokenizer.

use crate::app::{AppStatus, AppState, ChatMessage, TokenUsage};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Mutex;
use futures_util::StreamExt;
use tokio_util::io::StreamReader;

// ── Tool execution (delegated to the config registry) ────────────────

fn execute_tool(name: &str, tools: &[crate::config::ToolDef]) -> String {
    let Some(tool) = tools.iter().find(|t| t.name == name) else {
        return format!("Unknown tool: {name}");
    };
    match std::process::Command::new(tool.command[0]).args(&tool.command[1..]).output() {
        Ok(out) => String::from_utf8_lossy(&out.stdout).trim().to_string(),
        Err(e) => format!("Error running `{name}`: {e}"),
    }
}

/// Look for a [TOOL: name] marker in text. Returns the tool name if found.
fn parse_tool_call(text: &str) -> Option<String> {
    const PREFIX: &str = "[TOOL: ";
    text.find(PREFIX).and_then(|start| {
        let rest = &text[start + PREFIX.len()..];
        rest.find(']')
            .map(|end| rest[..end].trim().to_string())
    })
}

// ── CLI tokenizer (runs fm token-count --quiet in a blocking task) ──

async fn count_tokens(text: &str) -> Option<u32> {
    if text.trim().is_empty() { return Some(0); }
    let t = text.to_string();
    tokio::task::spawn_blocking(move || {
        let out = std::process::Command::new("fm")
            .args(["token-count", "--quiet", &t]).output().ok()?;
        if !out.status.success() { return None; }
        std::str::from_utf8(&out.stdout).ok()?.trim().parse::<u32>().ok()
    })
    .await.ok()?
}

async fn estimate_token_usage(history_before: &[ChatMessage], reply: &str) -> Option<TokenUsage> {
    let mut prompt_text = String::new();
    for msg in history_before {
        if matches!(msg.role.as_str(), "user" | "assistant" | "tool_call" | "tool_output") {
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

// ── SSE / JSON helpers ────────────────────────────────

fn parse_sse_line(line: &str) -> Option<&str> {
    if let Some(s) = line.strip_prefix("data: ") {
        if s == "[DONE]" || s.is_empty() { return None; }
        Some(s)
    } else { None }
}

struct StreamBuffer { content: String }

// ── Streaming request ────────────────────────────────

/// Perform a single streaming completion. Appends delta content to buffer.
async fn stream_request(
    client: &reqwest::Client,
    state: Arc<Mutex<AppState>>,
    cancel_token: tokio_util::sync::CancellationToken,
    url: &str, model: &str, messages: &[serde_json::Value],
    buffer: Arc<Mutex<StreamBuffer>>,
) -> Result<(), String> {
    let response = client.post(url)
        .json(&serde_json::json!({
            "model": model,
            "messages": messages,
            "stream": true,
            "temperature": 0.7,
            "max_tokens": 4096,
        }))
        .send().await.map_err(|e| format!("Request failed: {e}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let err_body = response.text().await.unwrap_or_default();
        return Err(format!("{status} - {err_body}"));
    }

    let stream = response.bytes_stream().map(|r| {
        r.map_err(std::io::Error::other)
    });
    let wrapped = StreamReader::new(stream);
    let mut reader = BufReader::with_capacity(4096, wrapped);
    let mut line_buf = String::with_capacity(4096);

    loop {
        if cancel_token.is_cancelled() { return Ok(()); }

        tokio::select! {
            r = reader.read_line(&mut line_buf) => {
                match r {
                    Ok(0) => break, // EOF.
                    Ok(_) => {
                        let trimmed = line_buf.trim();
                        if let Some(json_str) = parse_sse_line(trimmed) {
                            if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_str) {
                                if let Some(choices) = val.get("choices").and_then(|c| c.as_array()) {
                                    if !choices.is_empty() {
                                        if let Some(content) = choices[0].get("delta").and_then(|d| d.get("content").or_else(|| d.get("text"))).and_then(|c| c.as_str()) {
                                            buffer.lock().await.content.push_str(content);
                                            state.lock().await.current_response.push_str(content);
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
                            }
                        }
                        line_buf.clear();
                    }
                    Err(_) => break, // stream error -> EOF.
                }
            }
            _ = cancel_token.cancelled() => return Ok(()),
        }
    }

    // Trim trailing whitespace from the accumulated response.
    let mut buf = buffer.lock().await;
    buf.content = buf.content.trim_end_matches(char::is_whitespace).to_string();
    Ok(())
}

// ── Queue orchestrator (the main work loop) ────────────────────────

pub async fn process_queue_orchestrator(
    client: reqwest::Client,
    state: Arc<Mutex<AppState>>,
    cancel_token: tokio_util::sync::CancellationToken,
) {
    let tools = crate::config::TOOLS;
    let system_prompt = crate::config::TOOLS_SYSTEM_PROMPT.to_string();

    loop {
        // Pop next queued prompt.
        let next_prompt = {
            let mut s = state.lock().await;
            if s.pending_queue.is_empty() {
                s.status = AppStatus::Idle;
                break;
            }
            s.status = AppStatus::Streaming;
            s.pending_queue.remove(0)
        };

        // Record user message and reset streaming buffer.
        let stream_buffer = Arc::new(Mutex::new(StreamBuffer { content: String::new() }));
        {
            let mut s = state.lock().await;
            s.history.push(ChatMessage::new("user", next_prompt));
            s.current_response.clear();
            s.current_token_usage = None;
            s.response_time = None;
        }

        let prompt_start_time = std::time::Instant::now();
        let mut tool_iterations: usize = 0;

        // Inner agent loop.
        loop {
            if cancel_token.is_cancelled() { break; }

            if tool_iterations >= crate::config::MAX_AGENT_ITERATIONS {
                state.lock().await.history.push(ChatMessage::new(
                    "assistant",
                    format!("Stopped after {} agent iterations (tool-calling loop guard).", crate::config::MAX_AGENT_ITERATIONS),
                ));
                state.lock().await.status = AppStatus::Idle;
                break;
            }

            // Build request payload: filter out local system messages.
            let history_snapshot: Vec<ChatMessage> = {
                let s = state.lock().await;
                s.history.iter()
                    .filter(|m| matches!(m.role.as_str(), "user" | "assistant" | "tool_call" | "tool_output"))
                    .cloned()
                    .collect()
            };

            let mut msgs: Vec<serde_json::Value> = history_snapshot
                .into_iter()
                .map(|m| {
                    let mapped_role = match m.role.as_str() {
                        "tool_call" => "assistant",
                        "tool_output" => "user",
                        r => r,
                    };
                    serde_json::json!({"role": mapped_role, "content": m.content})
                })
                .collect();
            // Prepend the tool-aware system prompt.
            msgs.insert(0, serde_json::json!({"role": "system", "content": &system_prompt}));

            // Clear current response buffer for this stream iteration
            state.lock().await.current_response.clear();
            stream_buffer.lock().await.content.clear();

            let stream_result = stream_request(&client, Arc::clone(&state), cancel_token.clone(),
                crate::config::API_BASE_URL, crate::config::MODEL_NAME, &msgs, Arc::clone(&stream_buffer)).await;

            if let Err(e) = stream_result {
                state.lock().await.history.push(ChatMessage::new(
                    "assistant", format!("Connection error to Apple FM Serve: {e}")));
                state.lock().await.status = AppStatus::Idle;
                break; // fatal - stop processing queue.
            }

            let final_content = stream_buffer.lock().await.content.clone();

            if final_content.is_empty() {
                state.lock().await.status = AppStatus::Idle;
                break;
            }

            // Detect tool call
            if let Some(tool_name) = parse_tool_call(&final_content) {
                // Push tool call to history
                state.lock().await.history.push(ChatMessage::new("tool_call", final_content.clone()));
                // Execute the tool and loop back
                let output = execute_tool(&tool_name, tools);
                state.lock().await.history.push(ChatMessage::new(
                    "tool_output", format!("Tool Output ({tool_name}): {output}")));
                tool_iterations += 1;
            } else {
                // Final assistant reply.
                let history_before = {
                    let s = state.lock().await;
                    s.history.clone()
                };

                state.lock().await.history.push(ChatMessage::new("assistant", final_content.clone()));
                state.lock().await.current_response.clear();

                let usage = estimate_token_usage(&history_before, &final_content).await;
                state.lock().await.current_token_usage = usage;
                break; // done with this user prompt.
            }
        }

        // Record total elapsed response duration
        state.lock().await.response_time = Some(prompt_start_time.elapsed());
        state.lock().await.status = AppStatus::Idle;

        if cancel_token.is_cancelled() { break; }
    }
}
