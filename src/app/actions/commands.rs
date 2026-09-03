use super::*;

pub(super) fn background_terminal_list(session_id: &str) -> String {
    let tasks = crate::tools::background_task_snapshots(session_id);
    if tasks.is_empty() {
        return "No background terminals are running.".to_string();
    }

    let mut text = format!(
        "{} background terminal{} running:",
        tasks.len(),
        if tasks.len() == 1 { "" } else { "s" }
    );
    const MAX_LISTED_TASKS: usize = 20;
    let omitted = tasks.len().saturating_sub(MAX_LISTED_TASKS);
    for task in tasks.into_iter().take(MAX_LISTED_TASKS) {
        let pid = task
            .child_pid
            .map(|pid| pid.to_string())
            .unwrap_or_else(|| "starting".to_string());
        text.push_str(&format!(
            "\n  • {} · {} · PID {} · {}",
            task.id,
            crate::app::status::format_elapsed_compact(task.start_time.elapsed().as_secs()),
            pid,
            crate::tools::background_command_label(&task.command, 500)
        ));
    }
    if omitted > 0 {
        text.push_str(&format!("\n  … {omitted} more background terminals"));
    }
    text
}

pub(super) fn stop_background_terminals(session_id: &str) -> String {
    let result = crate::tools::stop_background_tasks(session_id);
    match (result.stopped, result.requested, result.failed) {
        (0, 0, 0) => "No background terminals are running.".to_string(),
        (1, 0, 0) => "Stopped 1 background terminal.".to_string(),
        (stopped, 0, 0) => format!("Stopped {stopped} background terminals."),
        (0, requested, 0) => format!(
            "Stop requested for {requested} background terminal(s); they are still starting."
        ),
        (stopped, requested, 0) => format!(
            "Stopped {stopped} background terminal(s); stop requested for {requested} still starting."
        ),
        (0, 0, failed) => format!("Failed to stop {failed} background terminal(s)."),
        (stopped, requested, failed) => format!(
            "Stopped {stopped} background terminal(s); stop requested for {requested}; failed to stop {failed}. Use /ps to inspect the remaining tasks."
        ),
    }
}

/// Max transcript characters sent to the summarizer. Beyond this we keep the
/// most recent content so a long session still summarizes without blowing the
/// model's context.
const MAX_SUMMARY_TRANSCRIPT_CHARS: usize = 16_000;
/// Tool outputs are the bulk of a session's bytes but low signal for a summary;
/// keep only a head of each so the transcript stays small and fast.
const MAX_SUMMARY_TOOL_CHARS: usize = 300;

pub async fn summarize_session(state_arc: &Arc<Mutex<AppState>>, client: &reqwest::Client) {
    summarize_session_inner(state_arc, client, false).await;
}

pub(crate) async fn summarize_session_after_idle(
    state_arc: &Arc<Mutex<AppState>>,
    client: &reqwest::Client,
) {
    summarize_session_inner(state_arc, client, true).await;
}

async fn summarize_session_inner(
    state_arc: &Arc<Mutex<AppState>>,
    client: &reqwest::Client,
    already_claimed: bool,
) {
    let started = std::time::Instant::now();
    let (api_base_url, model_name, transcript, captured_session_id, captured_history_len) = {
        let mut s = state_arc.lock().await;
        if !already_claimed && !s.claim_summary() {
            return;
        }

        // Flatten the chat into a single plain transcript. Sending the raw
        // history (system/assistant/tool roles) through the request builder's
        // alternation/merge logic produced empty responses on some providers;
        // one system instruction + one user message with the transcript is
        // robust everywhere.
        let mut transcript = String::new();
        for m in &s.history {
            if m.content.trim().is_empty() {
                continue;
            }
            let who = match m.role.as_str() {
                "user" => "USER",
                "assistant" => "ASSISTANT",
                "tool" => "TOOL",
                _ => "SYSTEM",
            };
            // Trim verbose tool outputs — they dominate the byte count but add
            // little the summary needs.
            let body: String =
                if m.role == "tool" && m.content.chars().count() > MAX_SUMMARY_TOOL_CHARS {
                    let head: String = m.content.chars().take(MAX_SUMMARY_TOOL_CHARS).collect();
                    format!("{head}… (truncated)")
                } else {
                    m.content.clone()
                };
            transcript.push_str(&format!("{who}: {body}\n\n"));
        }
        // Keep the most recent slice if oversized (char-boundary safe).
        if transcript.len() > MAX_SUMMARY_TRANSCRIPT_CHARS {
            let cut = transcript.len() - MAX_SUMMARY_TRANSCRIPT_CHARS;
            let mut idx = cut;
            while idx < transcript.len() && !transcript.is_char_boundary(idx) {
                idx += 1;
            }
            transcript = format!(
                "...(earlier conversation truncated)...\n\n{}",
                &transcript[idx..]
            );
        }

        // Drive the existing status-bar spinner + elapsed timer.

        s.status = AppStatus::Streaming;
        s.generation_start_time = Some(started);
        s.clear_current_response();

        (
            s.api_base_url.clone(),
            s.model_name.clone(),
            transcript,
            s.active_session_id.clone(),
            s.history.len(),
        )
    };

    dbg_log!(
        "[SUMMARIZE] start model={} url={} transcript_chars={}",
        model_name,
        api_base_url,
        transcript.len()
    );

    if transcript.trim().is_empty() {
        let mut s = state_arc.lock().await;
        s.status = AppStatus::Idle;
        s.generation_start_time = None;
        s.history
            .push(ChatMessage::new("system", "Nothing to summarize yet."));
        s.finish_summary();
        s.request_redraw();
        return;
    }

    let messages = vec![
        serde_json::json!({
            "role": "system",
            "content": "You are summarizing a coding assistant session. Produce a concise, structured summary with these sections: Problem, What was done, Current state, Open problems, Next steps. Omit a section if it has nothing. Be specific about files and decisions."
        }),
        serde_json::json!({ "role": "user", "content": format!("Summarize this session transcript:\n\n{transcript}") }),
    ];

    let stream_buffer = Arc::new(Mutex::new(crate::network::StreamBuffer::new()));
    let cancel_token = tokio_util::sync::CancellationToken::new();

    let stream_result = crate::network::stream_request(
        client,
        state_arc.clone(),
        cancel_token,
        &api_base_url,
        &model_name,
        messages,
        Arc::clone(&stream_buffer),
        true, // quiet: don't stream into the main chat view; we post the result
        false,
        crate::network::stream_request::ThinkingMode::Normal,
        crate::tools::ToolSchemaPolicy::root(false),
        None,
    )
    .await;

    let summary_content = stream_buffer.lock().await.content.clone();
    let elapsed = started.elapsed().as_secs_f32();

    let mut s = state_arc.lock().await;
    if s.active_session_id != captured_session_id || s.history.len() != captured_history_len {
        s.summary_in_flight = false;
        s.clear_current_response();
        s.request_redraw();
        return;
    }
    s.status = AppStatus::Idle;
    s.generation_start_time = None;
    s.clear_current_response();

    match stream_result {
        Ok(_) if !summary_content.trim().is_empty() => {
            dbg_log!(
                "[SUMMARIZE] ok in {:.1}s, {} chars",
                elapsed,
                summary_content.len()
            );
            // Post as an assistant message so it renders as a normal model reply
            // (chat bubble), not a system Notice/Warning — the summary text often
            // contains words like "error"/"loop" that would trip the warning style.
            let mut msg = ChatMessage::new("assistant", summary_content);
            msg.response_time_ms = Some((elapsed * 1000.0) as u64);
            s.history.push(msg);
        }
        Ok(_) => {
            dbg_log!("[SUMMARIZE] empty response after {:.1}s", elapsed);
            s.history.push(ChatMessage::new(
                "system",
                format!("Summarization failed: the model returned an empty response ({model_name}, {elapsed:.1}s). It may be rate-limited or rejecting the request — check debug.log."),
            ));
        }
        Err(e) => {
            dbg_log!("[SUMMARIZE] error after {:.1}s: {}", elapsed, e);
            s.history.push(ChatMessage::new(
                "system",
                format!("Summarization failed after {elapsed:.1}s: {e}"),
            ));
        }
    }
    s.finish_summary();
    crate::config::save_session_history(&captured_session_id, &s.history);
    s.request_redraw();
}

pub fn build_info_text() -> String {
    format!(
        "RustCode Info\n\
        ⚡ AI-powered agentic coding assistant for terminal workflows.\n\n\
        • Version:      v{}\n\
        • Repository:   https://github.com/LHagfoss/rustcode\n\n\
        Quick Commands:\n\
        • /help      - View full command list & keybindings\n\
        • /status    - View active session status & model info\n\
        • /update    - Upgrade rustcode via Homebrew tap\n\
        • /changelog - View recent version releases",
        env!("CARGO_PKG_VERSION")
    )
}

pub fn build_help_text() -> String {
    let mut help = String::from("Available Commands:\n\n");
    let categories: &[(&str, &[(&str, &str)])] = &[
        (
            "Core & Session:",
            &[
                ("/help", "Show help and commands"),
                ("/status", "Show session status and model quota"),
                ("/info", "Show version and system info"),
                ("/clear", "Clear conversation history"),
                ("/new", "Start a new conversation"),
                ("/fork", "Fork the current conversation into a new session"),
                ("/archive", "Persist the current session"),
                ("/agents", "Browse subagent conversation contexts"),
                ("/delete_chat", "Delete current session and start fresh"),
                ("/history", "Pick a previous session to resume"),
                ("/change_title", "Rename current session title"),
                ("/cancel", "Cancel active stream or queued prompt"),
                ("/exit", "Exit the app"),
            ],
        ),
        (
            "Model & Configuration:",
            &[
                ("/model", "Open model picker or switch profile"),
                ("/quota", "Show provider quota and remaining limits"),
                ("/context", "Show context usage or set context window"),
                ("/mcp", "Configure Model Context Protocol (MCP) servers"),
                ("/ollama", "Configure or list Ollama models"),
                ("/provider", "Add or update model provider profile"),
                ("/protocol", "Show or set current tool protocol"),
            ],
        ),
        (
            "Automation & Utilities:",
            &[
                ("/goal", "Run a task in continuous autoloop mode"),
                ("/delegate", "Allow subagents for next task only"),
                ("/yolo", "Show or set tool confirmation mode"),
                ("/skills", "Discover and list custom skills"),
                ("/sync", "Sync config, skills, and themes with Git"),
                ("/copy", "Copy last assistant reply to clipboard"),
                ("/memory", "Inspect or update bounded project memory"),
                ("/ps", "Show running background terminals"),
                ("/stop", "Stop all running background terminals"),
                ("/changelog", "Show recent changelog updates"),
            ],
        ),
    ];

    for (cat, cmds) in categories {
        help.push_str(&format!("{}\n", cat));
        for (name, desc) in *cmds {
            help.push_str(&format!("  {:<16} {}\n", name, desc));
        }
        help.push_str("\n");
    }

    help.push_str("Keyboard Shortcuts:\n");
    help.push_str("  Enter            Send prompt\n");
    help.push_str("  Shift+Enter      Insert newline\n");
    help.push_str("  Esc              Clear input or cancel generation\n");
    help.push_str("  Up/Down          Cycle prompt history\n");
    help.push_str("  Ctrl+P           Open command picker\n");
    help.push_str("  Ctrl+V           Paste image or text from clipboard\n");
    help.push_str("  Ctrl+L           Clear screen\n");
    help.push_str("  ?                Show help and keyboard shortcuts\n");
    help.push_str("  Ctrl+A / Ctrl+E  Move cursor to start / end of line\n");
    help.push_str("  Alt+F / Alt+B    Move cursor word right / left\n");
    help.push_str("  Ctrl+U / Ctrl+W  Delete line / word\n");
    help
}

pub fn get_picker_items_count(s: &AppState) -> usize {
    let search = s.model_picker_search.to_lowercase();
    s.config
        .models
        .iter()
        .filter(|m| m.name.to_lowercase().contains(&search))
        .count()
}

pub fn select_picker_model(s: &mut AppState) {
    let search = s.model_picker_search.to_lowercase();
    let filtered: Vec<&crate::config::ModelProfile> = s
        .config
        .models
        .iter()
        .filter(|m| m.name.to_lowercase().contains(&search))
        .collect();

    let idx = s.model_picker_index.min(filtered.len().saturating_sub(1));
    if !filtered.is_empty() {
        let profile = filtered[idx];
        s.api_base_url = profile.url.clone();
        s.model_name = profile.model.clone();
        s.config.default.set_big(profile.name.clone());
        crate::config::save_entire_config(&s.config);
        s.set_notice(format!("Switched to model profile '{}'", profile.name));
    }
}

pub fn trigger_sync(state: &Arc<Mutex<AppState>>, subcommand: Option<String>, arg: Option<String>) {
    let state_clone = Arc::clone(state);
    tokio::spawn(async move {
        {
            let mut s = state_clone.lock().await;
            s.set_notice("🔄 Syncing config repository...");
        }

        let result =
            tokio::task::spawn_blocking(move || match subcommand.as_deref() {
                Some("pull") => crate::config::sync_config_pull()
                    .map(|_| "Config pull complete! 📥".to_string()),
                Some("push") => crate::config::sync_config_push()
                    .map(|_| "Config push complete! 💾".to_string()),
                Some("init") => {
                    if let Some(url) = arg {
                        crate::config::init_sync_repo(&url)
                            .map(|_| "Sync repository initialized! 🚀".to_string())
                    } else {
                        Err("Usage: /sync init <remote-git-url>".to_string())
                    }
                }
                _ => {
                    crate::config::sync_config_pull()?;
                    crate::config::sync_config_push()?;
                    Ok("Config sync complete! 🚀".to_string())
                }
            })
            .await;

        let mut s = state_clone.lock().await;
        match result {
            Ok(Ok(msg)) => {
                s.set_notice(format!("✅ {msg}"));
                s.history
                    .push(ChatMessage::new("system", format!("✅ {msg}")));
            }
            Ok(Err(err)) => {
                s.set_warning_notice(format!("❌ {err}"));
                s.history
                    .push(ChatMessage::new("system", format!("❌ Sync error: {err}")));
            }
            Err(join_err) => {
                let err_msg = format!("Sync task error: {join_err}");
                s.set_warning_notice(format!("❌ {err_msg}"));
                s.history
                    .push(ChatMessage::new("system", format!("❌ {err_msg}")));
            }
        }
    });
}

pub fn trigger_update(state: &Arc<Mutex<AppState>>, client: &reqwest::Client) {
    let state_clone = Arc::clone(state);
    let client_clone = client.clone();
    tokio::spawn(async move {
        let check = crate::update::check_for_update(&client_clone).await;
        let mut s = state_clone.lock().await;
        match check {
            Ok(crate::update::UpdateCheck::UpToDate { current, latest }) => {
                s.update_check = crate::update::UpdateState::UpToDate(latest);
                s.set_notice(format!(
                    "✨ RustCode v{} is up to date (latest: v{}).",
                    crate::update::format_version(current),
                    crate::update::format_version(latest)
                ));
            }
            Ok(crate::update::UpdateCheck::Available { current, latest }) => {
                s.update_check = crate::update::UpdateState::Available(latest);
                s.set_notice(format!(
                    "Found new release: v{} → v{}, updating...",
                    crate::update::format_version(current),
                    crate::update::format_version(latest)
                ));
                s.update_requested = true;
            }
            Err(error) => {
                s.update_check = crate::update::UpdateState::Failed;
                s.set_warning_notice(format!("Update check failed: {error}"));
            }
        }
        s.request_redraw();
    });
}

pub fn trigger_quota_fetch(s: &AppState, state: &Arc<Mutex<AppState>>, client: &reqwest::Client) {
    let (url, key_opt) = {
        let active_url = s.api_base_url.clone();
        let key = s
            .config
            .models
            .iter()
            .find(|m| m.url == active_url || m.model == s.model_name)
            .and_then(|m| m.api_key.clone());
        (active_url, key)
    };
    let state_clone = Arc::clone(state);
    let client_clone = client.clone();
    tokio::spawn(async move {
        let base_url = if let Some(idx) = url.find("/v1") {
            &url[..idx]
        } else {
            url.trim_end_matches('/')
        };
        let status_url = format!("{}/auth/status", base_url);
        let mut req = client_clone.get(&status_url);
        if let Some(key) = key_opt {
            req = req.header("Authorization", format!("Bearer {key}"));
        }
        match req.send().await {
            Ok(res) => {
                if let Ok(json) = res.json::<serde_json::Value>().await {
                    let mut text = String::from("📊 Model Quota Status:\n");
                    let quota_obj = json.get("quota");
                    let buckets_arr = quota_obj
                        .and_then(|q| q.get("buckets").or_else(|| q.get("quotaBuckets")))
                        .and_then(|b| b.as_array());

                    if let Some(buckets) = buckets_arr {
                        for b in buckets {
                            if let (Some(m), Some(f)) = (
                                b.get("modelId").and_then(|x| x.as_str()),
                                b.get("remainingFraction").and_then(|x| x.as_f64()),
                            ) {
                                let display_name = match m {
                                    "gemini-2.5-flash" => {
                                        "gemini-2.5-flash / gemini-3.6-flash / 3.5-flash"
                                    }
                                    "gemini-2.5-pro" => "gemini-2.5-pro",
                                    _ => m,
                                };
                                text.push_str(&format!(
                                    "\n  • {}: {:.1}% remaining",
                                    display_name,
                                    f * 100.0
                                ));
                            }
                        }
                    } else if let Some(rate_limits) =
                        json.get("rate_limits").or_else(|| json.get("rate_limit"))
                    {
                        append_codex_rate_limits(&mut text, rate_limits);
                    } else {
                        text.push_str("\n  No quota information returned by this provider.");
                    }
                    let mut s = state_clone.lock().await;
                    s.history.push(ChatMessage::new("system", text));
                } else {
                    let mut s = state_clone.lock().await;
                    s.history.push(ChatMessage::new(
                        "system",
                        "Failed to parse quota JSON response.",
                    ));
                }
            }
            Err(e) => {
                let mut s = state_clone.lock().await;
                s.history.push(ChatMessage::new(
                    "system",
                    format!("Failed to reach proxy: {}", e),
                ));
            }
        }
        state_clone.lock().await.request_redraw();
    });
}

pub(super) fn append_codex_rate_limits(text: &mut String, rate_limits: &serde_json::Value) {
    for (label, key) in [("primary", "primary"), ("secondary", "secondary")] {
        let window = rate_limits.get(key).or_else(|| {
            if key == "primary" {
                rate_limits.get("primary_window")
            } else {
                rate_limits.get("secondary_window")
            }
        });
        let Some(window) = window else { continue };
        let Some(used) = window.get("used_percent").and_then(|v| v.as_f64()) else {
            continue;
        };
        let remaining = (100.0 - used).clamp(0.0, 100.0);
        let window_minutes = window
            .get("window_minutes")
            .and_then(|v| v.as_u64())
            .or_else(|| {
                window
                    .get("limit_window_seconds")
                    .and_then(|v| v.as_u64())
                    .map(|seconds| seconds / 60)
            });
        let window_label = match window_minutes {
            Some(minutes) if minutes % 1440 == 0 => format!("{}d", minutes / 1440),
            Some(minutes) if minutes % 60 == 0 => format!("{}h", minutes / 60),
            Some(minutes) => format!("{}m", minutes),
            None => String::new(),
        };
        let suffix = if window_label.is_empty() {
            String::new()
        } else {
            format!(" ({window_label})")
        };
        text.push_str(&format!(
            "\n  • ChatGPT {label}{suffix}: {remaining:.1}% remaining"
        ));
        if let Some(reset) = window.get("resets_at").and_then(|v| v.as_i64())
            && let Some(dt) = chrono::DateTime::from_timestamp(reset, 0)
        {
            text.push_str(&format!(
                "; resets {}",
                dt.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M")
            ));
        }
    }
}
