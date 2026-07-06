use crate::app::{AppState, ChatMessage, AppStatus};
use futures_util::TryStreamExt;
use serde_json::json;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Mutex;
use tokio_util::io::StreamReader;
use tokio_util::sync::CancellationToken;

fn execute_tool(name: &str) -> String {
    match name {
        "get_time" => {
            if let Ok(output) = std::process::Command::new("date").output() {
                String::from_utf8_lossy(&output.stdout).trim().to_string()
            } else {
                "Error: Could not retrieve system time.".to_string()
            }
        }
        "get_env" => {
            if let Ok(output) = std::process::Command::new("uname").arg("-sr").output() {
                String::from_utf8_lossy(&output.stdout).trim().to_string()
            } else {
                "Error: Could not retrieve system environment info.".to_string()
            }
        }
        _ => "Error: Unknown tool.".to_string(),
    }
}

async fn estimate_token_usage_via_cli(
    history_before: &[ChatMessage],
    assistant_reply: &str,
) -> Option<crate::app::TokenUsage> {
    // Format history for token counting (excluding local system messages)
    let mut prompt_text = String::new();
    for msg in history_before {
        if msg.role == "user" || msg.role == "assistant" || msg.role == "tool_call" || msg.role == "tool_output" {
            prompt_text.push_str(&msg.content);
            prompt_text.push('\n');
        }
    }

    let prompt_tokens = if prompt_text.trim().is_empty() {
        0
    } else {
        tokio::task::spawn_blocking({
            let text = prompt_text.clone();
            move || {
                let output = std::process::Command::new("fm")
                    .args(&["token-count", "--quiet", &text])
                    .output()
                    .ok()?;
                if output.status.success() {
                    std::str::from_utf8(&output.stdout).ok()?.trim().parse::<u32>().ok()
                } else {
                    None
                }
            }
        })
        .await
        .ok()??
    };

    let mut full_text = prompt_text;
    full_text.push_str(assistant_reply);
    full_text.push('\n');

    let total_tokens = if full_text.trim().is_empty() {
        0
    } else {
        tokio::task::spawn_blocking({
            let text = full_text;
            move || {
                let output = std::process::Command::new("fm")
                    .args(&["token-count", "--quiet", &text])
                    .output()
                    .ok()?;
                if output.status.success() {
                    std::str::from_utf8(&output.stdout).ok()?.trim().parse::<u32>().ok()
                } else {
                    None
                }
            }
        })
        .await
        .ok()??
    };

    let completion_tokens = total_tokens.saturating_sub(prompt_tokens);

    Some(crate::app::TokenUsage {
        prompt_tokens,
        completion_tokens,
        total_tokens,
    })
}

pub async fn process_queue_orchestrator(
    client: reqwest::Client,
    state: Arc<Mutex<AppState>>,
    cancel_token: CancellationToken,
) {
    loop {
        let next_prompt = {
            let mut s = state.lock().await;
            if s.pending_queue.is_empty() {
                s.status = AppStatus::Idle;
                break;
            }
            s.status = AppStatus::Streaming;
            s.pending_queue.remove(0)
        };

        {
            let mut s = state.lock().await;
            s.history.push(ChatMessage {
                role: "user".to_string(),
                content: next_prompt,
                token_usage: None,
            });
            s.current_response.clear();
            s.current_token_usage = None;
        }

        let mut should_stop = false;
        let mut tool_iterations = 0;

        // Inner agent loop to handle tool calls recursively
        loop {
            // Guardrail to prevent infinite agent tool calling loops
            if tool_iterations >= 5 {
                let mut s = state.lock().await;
                s.history.push(ChatMessage {
                    role: "system".to_string(),
                    content: "Warning: Agent reached maximum tool execution limit (5 iterations). Stopping tool loop.".to_string(),
                    token_usage: None,
                });
                s.pending_queue.clear();
                s.status = AppStatus::Idle;
                break;
            }

            // Clean payload: Filter out TUI-local system messages
            let mut current_history_payload: Vec<serde_json::Value> = {
                let s = state.lock().await;
                s.history
                    .iter()
                    .filter(|msg| msg.role == "user" || msg.role == "assistant" || msg.role == "tool_call" || msg.role == "tool_output")
                    .map(|msg| {
                        let mapped_role = match msg.role.as_str() {
                            "tool_call" => "assistant",
                            "tool_output" => "user",
                            r => r,
                        };
                        json!({
                            "role": mapped_role,
                            "content": msg.content
                        })
                    })
                    .collect()
            };

            // Prepend direct, concise system instructions so the local LLM knows it has tools
            let system_prompt = json!({
                "role": "system",
                "content": "You are a helpful local assistant. You have access to local tools:\n- To get the current time, output exactly: `[TOOL: get_time]`\n- To get the system info, output exactly: `[TOOL: get_env]`\nWhen you need to call a tool, output only the command and wait for the output."
            });
            current_history_payload.insert(0, system_prompt);

            let payload = json!({
                "model": "system",
                "messages": current_history_payload,
                "stream": true,
                "stream_options": {
                    "include_usage": true
                }
            });

            let request_future = client
                .post("http://127.0.0.1:1976/v1/chat/completions")
                .json(&payload)
                .send();

            let mut cancelled = false;
            let mut stream_completed = false;

            tokio::select! {
                _ = cancel_token.cancelled() => {
                    let mut s = state.lock().await;
                    s.history.push(ChatMessage {
                        role: "assistant".to_string(),
                        content: "[Stream Cancelled by User]".to_string(),
                        token_usage: None,
                    });
                    s.current_response.clear();
                    s.current_token_usage = None;
                    s.pending_queue.clear();
                    s.status = AppStatus::Idle;
                    should_stop = true;
                }
                res_wrapper = request_future => {
                    match res_wrapper {
                        Ok(res) if res.status().is_success() => {
                            let stream = res
                                .bytes_stream()
                                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e));
                            let stream_reader = StreamReader::new(stream);
                            let mut reader = BufReader::new(stream_reader);
                            let mut line_buf = String::new();

                            loop {
                                line_buf.clear();
                                tokio::select! {
                                    _ = cancel_token.cancelled() => {
                                        cancelled = true;
                                        break;
                                    }
                                    read_res = reader.read_line(&mut line_buf) => {
                                        match read_res {
                                            Ok(0) => break, // EOF
                                            Ok(_) => {
                                                let line = line_buf.trim();
                                                if line.starts_with("data: ") {
                                                    let json_str = line.trim_start_matches("data: ").trim();
                                                    if json_str == "[DONE]" {
                                                        break;
                                                    }
                                                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_str) {
                                                        // Parse message content chunk safely
                                                        if let Some(choices) = val.get("choices").and_then(|c| c.as_array()) {
                                                            if !choices.is_empty() {
                                                                if let Some(content) = choices[0]
                                                                    .get("delta")
                                                                    .and_then(|d| d.get("content"))
                                                                    .and_then(|c| c.as_str())
                                                                {
                                                                    let mut s = state.lock().await;
                                                                    s.current_response.push_str(content);
                                                                }
                                                            }
                                                        }
                                                        // Parse token usage statistics safely (if supported)
                                                        if let Some(usage) = val.get("usage") {
                                                            if let (Some(p), Some(c), Some(t)) = (
                                                                usage.get("prompt_tokens").and_then(|v| v.as_u64()),
                                                                usage.get("completion_tokens").and_then(|v| v.as_u64()),
                                                                usage.get("total_tokens").and_then(|v| v.as_u64()),
                                                            ) {
                                                                let mut s = state.lock().await;
                                                                s.current_token_usage = Some(crate::app::TokenUsage {
                                                                    prompt_tokens: p as u32,
                                                                    completion_tokens: c as u32,
                                                                    total_tokens: t as u32,
                                                                });
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                            Err(_) => break,
                                        }
                                    }
                                }
                            }

                            let mut s = state.lock().await;
                            if cancelled {
                                let response_text = format!("{} [Cancelled]", s.current_response);
                                let usage = s.current_token_usage.take();
                                s.history.push(ChatMessage {
                                    role: "assistant".to_string(),
                                    content: response_text,
                                    token_usage: usage,
                                });
                                s.pending_queue.clear();
                                s.current_response.clear();
                                s.status = AppStatus::Idle;
                                should_stop = true;
                            } else {
                                let final_reply = s.current_response.clone();
                                let history_before = s.history.clone();
                                let mut usage = s.current_token_usage.take();
                                drop(s);

                                // If server didn't supply token stats, count them via background CLI call
                                if usage.is_none() {
                                    usage = estimate_token_usage_via_cli(&history_before, &final_reply).await;
                                }

                                let mut s = state.lock().await;
                                let is_tool_call = final_reply.contains("[TOOL: get_time]") || final_reply.contains("[TOOL: get_env]");
                                let role = if is_tool_call { "tool_call".to_string() } else { "assistant".to_string() };

                                s.history.push(ChatMessage {
                                    role,
                                    content: final_reply,
                                    token_usage: usage,
                                });
                                s.current_response.clear();
                                stream_completed = true;
                            }
                        }
                        Ok(res) => {
                            let status = res.status();
                            let err_body = res.text().await.unwrap_or_default();
                            let mut s = state.lock().await;
                            s.history.push(ChatMessage {
                                role: "assistant".to_string(),
                                content: format!("Error from Apple FM Serve: {} - {}", status, err_body),
                                token_usage: None,
                            });
                            s.pending_queue.clear();
                            s.current_token_usage = None;
                            s.status = AppStatus::Idle;
                            should_stop = true;
                        }
                        Err(e) => {
                            let mut s = state.lock().await;
                            s.history.push(ChatMessage {
                                role: "assistant".to_string(),
                                content: format!("Connection error to Apple FM Serve: {}", e),
                                token_usage: None,
                            });
                            s.pending_queue.clear();
                            s.current_token_usage = None;
                            s.status = AppStatus::Idle;
                            should_stop = true;
                        }
                    }
                }
            }

            if should_stop {
                break;
            }

            // Check if the latest response has a tool call
            if stream_completed {
                let latest_msg = {
                    let s = state.lock().await;
                    s.history.last().cloned()
                };

                if let Some(msg) = latest_msg {
                    if msg.role == "tool_call" {
                        let tool_name = if msg.content.contains("[TOOL: get_time]") {
                            "get_time"
                        } else {
                            "get_env"
                        };
                        let output = execute_tool(tool_name);
                        let mut s = state.lock().await;
                        s.history.push(ChatMessage {
                            role: "tool_output".to_string(),
                            content: format!("Tool Output ({}): {}", tool_name, output),
                            token_usage: None,
                        });
                        tool_iterations += 1;
                        continue; // Loop back to request the next reply using the tool output
                    }
                }
            }

            // No tool calls, we are finished with this user prompt
            break;
        }

        if should_stop {
            break;
        }
    }
}
