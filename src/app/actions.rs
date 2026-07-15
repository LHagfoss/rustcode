use crate::app::{AppState, AppStatus, ChatMessage};
use std::sync::Arc;
use sysinfo::{Pid, System};
use tokio::sync::Mutex;

pub async fn handle_escape(
    state: &Arc<Mutex<AppState>>,
    cancel_token: &mut tokio_util::sync::CancellationToken,
) {
    let mut s = state.lock().await;
    s.reset_suggestion_cycle();
    s.input_buffer.clear();
    s.cursor_position = 0;

    cancel_token.cancel();
    *cancel_token = tokio_util::sync::CancellationToken::new();

    if s.status == AppStatus::Streaming {
        s.status = AppStatus::Idle;
        s.pending_queue.clear();
    } else if !s.pending_queue.is_empty() {
        s.pending_queue.remove(0);
        if s.pending_queue.is_empty() {
            s.status = AppStatus::Idle;
        }
    }
}

pub async fn handle_enter(
    state: &Arc<Mutex<AppState>>,
    client: &reqwest::Client,
    cancel_token: &mut tokio_util::sync::CancellationToken,
) -> bool {
    let mut s = state.lock().await;
    s.reset_suggestion_cycle();
    s.history_index = None;

    if s.active_suggestion_index.is_some() {
        apply_autocomplete(&mut s);
    }

    let raw_input = s.input_buffer.trim().to_string();

    if raw_input.is_empty() {
        return false;
    }

    if raw_input.starts_with('/') {
        let tokens: Vec<&str> = raw_input.split_whitespace().collect();
        if tokens.is_empty() {
            s.input_buffer.clear();
            s.cursor_position = 0;
            return false;
        }

        s.history.push(ChatMessage::new("user", raw_input.clone()));
        crate::config::save_history(&s.history);

        let cmd = tokens[0];
        let mut should_exit = false;

        match cmd {
            "/memory" => {
                check_memory_usage(&mut s);
            }
            "/clear" => {
                // Visual wipe only: clear streamed response and token usage.
                // History, cancel-token, session state all stay intact — the
                // LLM still sees the same chat on next message.
                s.current_response.clear();
                s.current_token_usage = None;
                s.status = AppStatus::Idle;
            }
            "/new" => {
                cancel_token.cancel();
                *cancel_token = tokio_util::sync::CancellationToken::new();
                start_new_session(&mut s);
            }
            "/cancel" => {
                cancel_token.cancel();
                *cancel_token = tokio_util::sync::CancellationToken::new();
            }
            "/goal" => {
                let goal_text = tokens[1..].join(" ");
                if goal_text.trim().is_empty() {
                    s.history.push(ChatMessage::new("system", "Usage: /goal <task description>"));
                } else {
                    s.continuous_mode = true;
                    let goal_msg = format!("Goal: {}\n\nContinuous autoloop mode is active. You must execute tools in a loop to complete the goal, and call the 'complete_task' tool when you are fully finished.", goal_text);
                    if let Some(m) = s.history.last_mut() {
                        if m.role == "user" {
                            m.content = goal_msg;
                        }
                    }
                    s.input_buffer.clear();
                    s.cursor_position = 0;
                    return true;
                }
            }
            "/help" => {
                let help = build_help_text();
                s.history.push(ChatMessage::new("system", help));
            }
            "/exit" | "/quit" => {
                should_exit = true;
            }
            "/copy" => {
                copy_last_reply(&mut s);
            }
            "/resume" => {
                resume_latest_session(&mut s);
            }
            "/history" => {
                let sessions = build_session_list(&s);
                if sessions.is_empty() {
                    s.history
                        .push(ChatMessage::new("system", "No saved sessions found."));
                } else {
                    s.history_picker_sessions = sessions;
                    s.history_picker_index = 0;
                    s.show_history_picker = true;
                }
            }
            "/mcp" => {
                s.show_mcp_config = true;
                s.mcp_picker_index = 0;
                s.mcp_edit_state = None;
            }
            "/context" => {
                let default_name = s.config.default.clone();
                if tokens.len() >= 2 {
                    match parse_token_count(tokens[1]) {
                        Some(n) => {
                            if let Some(profile) =
                                s.config.models.iter_mut().find(|m| m.name == default_name)
                            {
                                profile.context_window = Some(n);
                                crate::config::save_entire_config(&s.config);
                                s.history.push(ChatMessage::new(
                                    "system",
                                    format!(
                                        "Set context window for profile '{}' to {} tokens",
                                        default_name, n
                                    ),
                                ));
                            } else {
                                s.history.push(ChatMessage::new(
                                    "system",
                                    "No active profile to set context window on.",
                                ));
                            }
                        }
                        None => {
                            s.history.push(ChatMessage::new(
                                "system",
                                "Usage: /context <tokens> - e.g. /context 262144 or /context 256k",
                            ));
                        }
                    }
                } else {
                    let window = s
                        .config
                        .models
                        .iter()
                        .find(|m| m.name == default_name)
                        .and_then(|p| p.context_window);
                    let text = match window {
                        Some(w) => format!("Context window for '{}': {} tokens", default_name, w),
                        None => format!(
                            "Context window for '{}': not set (using default {})",
                            default_name,
                            crate::config::DEFAULT_CONTEXT_WINDOW
                        ),
                    };
                    s.history.push(ChatMessage::new(
                        "system",
                        format!("{text}\nSet with: /context <tokens>"),
                    ));
                }
            }
            "/usage" | "/stats" | "/status" => {
                let mut text = String::from("Session usage:");
                let user_msgs = s.history.iter().filter(|m| m.role == "user").count();
                let assistant_msgs = s.history.iter().filter(|m| m.role == "assistant").count();
                let tool_calls = s.history.iter().filter(|m| m.role == "tool").count();
                text.push_str(&format!(
                    "\n  messages: {} user, {} assistant, {} tool calls",
                    user_msgs, assistant_msgs, tool_calls
                ));
                match &s.current_token_usage {
                    Some(u) => {
                        text.push_str(&format!(
                            "\n  last exchange: {} prompt + {} completion = {} tokens",
                            u.prompt_tokens, u.completion_tokens, u.total_tokens
                        ));
                        if s.model_name == "system" {
                            let pct = (u.total_tokens as f32
                                / crate::config::MAX_CONTEXT_TOKENS as f32)
                                * 100.0;
                            text.push_str(&format!(
                                "\n  context: {} / {} tokens ({:.0}%, apple-fm limit)",
                                u.total_tokens,
                                crate::config::MAX_CONTEXT_TOKENS,
                                pct
                            ));
                        }
                    }
                    None => {
                        text.push_str("\n  no token data yet - send a message first");
                    }
                }
                if let Some(rt) = s.response_time {
                    text.push_str(&format!("\n  last response time: {:.1}s", rt.as_secs_f32()));
                }

                let format_commas = |n: u64| -> String {
                    let s = n.to_string();
                    let mut result = String::new();
                    let len = s.len();
                    for (i, c) in s.chars().enumerate() {
                        if i > 0 && (len - i) % 3 == 0 {
                            result.push(',');
                        }
                        result.push(c);
                    }
                    result
                };

                let usage_history = crate::config::get_usage_history();
                if !usage_history.is_empty() {
                    text.push_str("\n\nMonthly usage statistics:");
                    for (month, stats) in usage_history {
                        text.push_str(&format!(
                            "\n  {}: {} prompt + {} completion = {} tokens ({} calls)",
                            month,
                            format_commas(stats.prompt_tokens),
                            format_commas(stats.completion_tokens),
                            format_commas(stats.total_tokens),
                            format_commas(stats.calls)
                        ));
                    }
                }

                s.history.push(ChatMessage::new("system", text));
            }
            "/protocol" | "/parser" => {
                if tokens.len() < 2 {
                    let msg = "Current tool protocol: json\nThe only supported format is JSON. Use /tool to toggle between tool-calling and plain text mode.";
                    s.history.push(ChatMessage::new("system", msg.to_string()));
                } else {
                    let new_proto = tokens[1].to_lowercase();
                    match new_proto.as_str() {
                        "json" => {
                            s.config.tool_protocol = crate::config::ToolProtocol::Json;
                            crate::config::save_entire_config(&s.config);
                            s.history.push(ChatMessage::new(
                                "system",
                                "Switched tool protocol to JSON.".to_string(),
                            ));
                        }
                        _ => {
                            s.history.push(ChatMessage::new(
                                "system",
                                format!(
                                    "Unknown protocol '{}'. Only 'json' is supported.",
                                    tokens[1]
                                ),
                            ));
                        }
                    }
                }
            }
            "/tools" => {
                let mut text = String::from("Available tools (model can call these):");
                for t in crate::tools::TOOLS {
                    text.push_str(&format!("\n  {} - {}", t.name, t.description));
                }
                text.push_str(&format!(
                    "\n\nMax {} tool rounds per prompt. Add tools in src/tools.rs.",
                    crate::tools::MAX_TOOL_ROUNDS
                ));
                s.history.push(ChatMessage::new("system", text));
            }
            "/model" => {
                if tokens.len() < 2 {
                    s.show_model_picker = true;
                    s.model_picker_index = 0;
                    s.model_picker_search.clear();
                } else {
                    let name = tokens[1].to_string();
                    if let Some(profile) = s.config.models.iter().find(|m| m.name == name) {
                        let url = profile.url.clone();
                        let model = profile.model.clone();
                        s.api_base_url = url;
                        s.model_name = model;
                        s.config.default = name.clone();
                        crate::config::save_entire_config(&s.config);
                        s.history.push(ChatMessage::new(
                            "system",
                            format!("Switched to model profile '{}'", name),
                        ));
                    } else {
                        s.model_name = name.clone();
                        let default_name = s.config.default.clone();
                        if let Some(profile) =
                            s.config.models.iter_mut().find(|m| m.name == default_name)
                        {
                            profile.model = name.clone();
                        }
                        crate::config::save_entire_config(&s.config);
                        s.history.push(ChatMessage::new(
                            "system",
                            format!("Switched active model to '{}'", name),
                        ));
                    }
                }
            }
            "/provider" => {
                if tokens.len() >= 4 {
                    let name = tokens[1].to_string();
                    let url = tokens[2].to_string();
                    let model = tokens[3].to_string();
                    let context_window = tokens.get(4).and_then(|t| parse_token_count(t));
                    let engine = tokens.get(5).map(|s| s.to_string());

                    s.api_base_url = url.clone();
                    s.model_name = model.clone();

                    if let Some(profile) = s.config.models.iter_mut().find(|m| m.name == name) {
                        profile.url = url;
                        profile.model = model;
                        if context_window.is_some() {
                            profile.context_window = context_window;
                        }
                        if engine.is_some() {
                            profile.engine = engine;
                        }
                    } else {
                        s.config.models.push(crate::config::ModelProfile {
                            name: name.clone(),
                            url,
                            model,
                            context_window,
                            engine,
                        });
                    }
                    s.config.default = name.clone();
                    crate::config::save_entire_config(&s.config);
                    s.history.push(ChatMessage::new(
                        "system",
                        format!("Created/updated profile '{}' and set as default", name),
                    ));
                } else if tokens.len() == 3 {
                    let url = tokens[1].to_string();
                    let model = tokens[2].to_string();
                    s.api_base_url = url.clone();
                    s.model_name = model.clone();

                    let default_name = s.config.default.clone();
                    if let Some(profile) =
                        s.config.models.iter_mut().find(|m| m.name == default_name)
                    {
                        profile.url = url;
                        profile.model = model;
                    }
                    crate::config::save_entire_config(&s.config);

                    let active_default = s.config.default.clone();
                    let active_url = s.api_base_url.clone();
                    let active_model = s.model_name.clone();
                    s.history.push(ChatMessage::new(
                        "system",
                        format!(
                            "Updated active profile '{}' with URL '{}' and model '{}'",
                            active_default, active_url, active_model
                        ),
                    ));
                } else {
                    s.history.push(ChatMessage::new("system", "Usage:\n  /provider <name> <url> <model> [context_window] - Create/update profile\n  /provider <url> <model> - Update active profile"));
                }
            }
            "/ollama" => {
                if tokens.len() >= 2 && tokens[1] == "list" {
                    let ollama_url = if tokens.len() >= 3 {
                        tokens[2]
                    } else {
                        &s.api_base_url
                    };

                    let tags_url = if ollama_url.ends_with("/v1/chat/completions") {
                        ollama_url.replace("/v1/chat/completions", "/api/tags")
                    } else if ollama_url.ends_with("/v1/") {
                        ollama_url.replace("/v1/", "/api/tags")
                    } else if ollama_url.ends_with('/') {
                        format!("{}api/tags", ollama_url)
                    } else {
                        format!("{}/api/tags", ollama_url)
                    };

                    s.history.push(ChatMessage::new(
                        "system",
                        format!("Fetching Ollama models from '{}'...", tags_url),
                    ));

                    let client_clone = client.clone();
                    let state_clone = Arc::clone(state);
                    tokio::spawn(async move {
                        match client_clone.get(&tags_url).send().await {
                            Ok(res) => {
                                if res.status().is_success() {
                                    #[derive(serde::Deserialize)]
                                    struct OllamaModel {
                                        name: String,
                                    }
                                    #[derive(serde::Deserialize)]
                                    struct OllamaTags {
                                        models: Vec<OllamaModel>,
                                    }

                                    match res.json::<OllamaTags>().await {
                                        Ok(tags) => {
                                            let names: Vec<String> =
                                                tags.models.into_iter().map(|m| m.name).collect();
                                            let mut s = state_clone.lock().await;
                                            if names.is_empty() {
                                                s.history.push(ChatMessage::new(
                                                    "system",
                                                    "Ollama returned no models.",
                                                ));
                                            } else {
                                                s.history.push(ChatMessage::new(
                                                    "system",
                                                    format!(
                                                        "Available Ollama models:\n  {}",
                                                        names.join("\n  ")
                                                    ),
                                                ));
                                            }
                                        }
                                        Err(e) => {
                                            let mut s = state_clone.lock().await;
                                            s.history.push(ChatMessage::new(
                                                "system",
                                                format!(
                                                    "Failed to parse Ollama tags response: {}",
                                                    e
                                                ),
                                            ));
                                        }
                                    }
                                } else {
                                    let mut s = state_clone.lock().await;
                                    s.history.push(ChatMessage::new(
                                        "system",
                                        format!("Ollama returned status code: {}", res.status()),
                                    ));
                                }
                            }
                            Err(e) => {
                                let mut s = state_clone.lock().await;
                                s.history.push(ChatMessage::new(
                                    "system",
                                    format!("Failed to fetch Ollama models: {}", e),
                                ));
                            }
                        }
                    });
                } else if tokens.len() == 3 {
                    let url = tokens[1].to_string();
                    let model = tokens[2].to_string();
                    s.api_base_url = url.clone();
                    s.model_name = model.clone();

                    if let Some(profile) = s.config.models.iter_mut().find(|m| m.name == "ollama") {
                        profile.url = url;
                        profile.model = model;
                    } else {
                        s.config.models.push(crate::config::ModelProfile {
                            name: "ollama".to_string(),
                            url,
                            model,
                            context_window: None,
                            engine: Some("ollama".to_string()),
                        });
                    }
                    s.config.default = "ollama".to_string();
                    crate::config::save_entire_config(&s.config);
                    s.history.push(ChatMessage::new(
                        "system",
                        "Switched to profile 'ollama' and updated its URL and model",
                    ));
                } else {
                    s.history.push(ChatMessage::new("system", "Usage:\n  /ollama list [url] - List available models\n  /ollama <url> <model> - Set 'ollama' profile URL and model"));
                }
            }
            "/change_title" => {
                if tokens.len() < 2 {
                    s.history.push(ChatMessage::new("system",
                        "Usage:\n  /change_title <title> - Rename the current session",));
                } else {
                    let new_title = tokens[1..].join(" ");
                    crate::config::save_session_title(&s.active_session_id, &new_title);
                    s.history.push(ChatMessage::new(
                        "system",
                        format!("Session title renamed to \"{}\"", new_title),));
                }
            }
            _ => {
                s.history.push(ChatMessage::new(
                    "system",
                    format!("Unknown command: {}", cmd),
                ));
            }
        }

        if matches!(cmd, "/model" | "/provider" | "/ollama") {
            spawn_context_window_detection(Arc::clone(state), client.clone());
        }

        s.input_buffer.clear();
        s.cursor_position = 0;
        return should_exit;
    }

    s.pending_queue.push(raw_input);
    s.input_buffer.clear();
    s.cursor_position = 0;

    if s.status == AppStatus::Idle {
        s.status = AppStatus::Queued;
        let client_clone = client.clone();
        let state_clone = Arc::clone(state);
        let token_clone = cancel_token.clone();
        drop(s);

        tokio::spawn(async move {
            crate::network::process_queue_orchestrator(client_clone, state_clone, token_clone)
                .await;
        });
    }
    false
}

pub fn get_filtered_cmds_len(input_buffer: &str) -> usize {
    if input_buffer.starts_with('/') && !input_buffer.contains(' ') {
        crate::app::suggestion::COMMANDS
            .iter()
            .filter(|c| c.name.starts_with(input_buffer))
            .count()
    } else {
        0
    }
}

pub fn apply_autocomplete(s: &mut AppState) {
    if s.input_buffer.starts_with('/') && !s.input_buffer.contains(' ') {
        let filtered_cmds: Vec<&crate::app::suggestion::CommandInfo> =
            crate::app::suggestion::COMMANDS
                .iter()
                .filter(|c| c.name.starts_with(&s.input_buffer))
                .collect();
        let idx = s
            .active_suggestion_index
            .unwrap_or(0)
            .min(filtered_cmds.len().saturating_sub(1));
        if !filtered_cmds.is_empty() {
            s.input_buffer = filtered_cmds[idx].name.to_string();
            s.cursor_position = s.input_buffer.len();
        }
        s.active_suggestion_index = None;
    }
}

pub fn check_memory_usage(s: &mut AppState) {
    let mut sys = System::new_all();
    sys.refresh_all();

    let pid = Pid::from(std::process::id() as usize);
    if let Some(process) = sys.process(pid) {
        let mem_mb = process.memory() / 1024 / 1024;
        s.history.push(ChatMessage::new(
            "system",
            &format!("🦀 Current Rustcode RAM usage: {} MB", mem_mb),
        ));
    } else {
        s.history.push(ChatMessage::new(
            "system",
            "Could not find current process.",
        ));
    }
}

pub fn start_new_session(s: &mut AppState) {
    crate::config::save_session_history(&s.active_session_id, &s.history);
    s.history.clear();
    s.pending_queue.clear();
    s.current_response.clear();
    s.current_token_usage = None;
    s.response_time = None;
    s.history_index = None;
    s.temp_input.clear();
    s.status = AppStatus::Idle;
    s.subagents.clear();
    s.next_subagent_id = 1;
    s.tip_index = crate::app::random_tip_index();

    // Switch to a new active session ID
    s.active_session_id = crate::config::create_new_session(&mut s.config);
    crate::config::save_session_history(&s.active_session_id, &s.history);
}

/// Fill in the active profile's context window from the provider when the
/// config doesn't have one (currently: ollama's /api/show). Silent no-op on
/// non-ollama endpoints, errors, or profiles that already have a window set.
pub fn spawn_context_window_detection(state: Arc<Mutex<AppState>>, client: reqwest::Client) {
    tokio::spawn(async move {
        let (name, url, model, engine) = {
            let s = state.lock().await;
            let name = s.config.default.clone();
            let Some(profile) = s.config.models.iter().find(|m| m.name == name) else {
                return;
            };
            if profile.context_window.is_some() {
                return;
            }
            (
                name,
                profile.url.clone(),
                profile.model.clone(),
                profile.engine.clone(),
            )
        };
        let Some(ctx) =
            crate::network::fetch_context_window(&client, &url, &model, engine.as_deref()).await
        else {
            return;
        };
        let mut s = state.lock().await;
        if let Some(profile) = s.config.models.iter_mut().find(|m| m.name == name)
            && profile.context_window.is_none()
        {
            profile.context_window = Some(ctx);
            crate::config::save_entire_config(&s.config);
            s.history.push(ChatMessage::new(
                "system",
                format!("Detected context window for '{}': {} tokens", name, ctx),
            ));
        }
    });
}

/// Parse a context window size like "262144" or "256k".
pub fn parse_token_count(input: &str) -> Option<u32> {
    let trimmed = input.trim();
    if let Some(k) = trimmed
        .strip_suffix('k')
        .or_else(|| trimmed.strip_suffix('K'))
    {
        return k.parse::<u32>().ok().and_then(|n| n.checked_mul(1024));
    }
    trimmed.parse::<u32>().ok()
}

/// Sessions available to resume: archived ones plus the live history file
/// from the previous run (only when the current chat has no real prompt yet,
/// otherwise the live file just mirrors what's already on screen).
pub fn build_session_list(s: &AppState) -> Vec<crate::config::SessionMeta> {
    let mut list = crate::config::list_sessions();
    if !crate::config::session_has_content(&s.history)
        && let Some(live) = crate::config::live_session_meta()
    {
        list.insert(0, live);
    }
    list
}

pub fn resume_latest_session(s: &mut AppState) {
    let list = build_session_list(s);
    match list.first() {
        Some(meta) => {
            let meta = meta.clone();
            load_session_into(s, &meta);
        }
        None => {
            s.history
                .push(ChatMessage::new("system", "No previous session to resume."));
        }
    }
}

pub fn load_session_into(s: &mut AppState, meta: &crate::config::SessionMeta) {
    let loaded = crate::config::load_session_file(&meta.path);
    if loaded.is_empty() {
        s.history.push(ChatMessage::new(
            "system",
            format!("Could not load session '{}'", meta.title),
        ));
        return;
    }

    // Save current active session history
    crate::config::save_session_history(&s.active_session_id, &s.history);

    // Extract session ID from the loaded path parent
    if let Some(parent) = meta.path.parent() {
        if let Some(session_id_str) = parent.file_name().and_then(|n| n.to_str()) {
            s.active_session_id = session_id_str.to_string();
            s.config.last_active_session_id = Some(s.active_session_id.clone());
            crate::config::save_entire_config(&s.config);
        }
    }

    s.history = loaded;
    s.pending_queue.clear();
    s.current_response.clear();
    s.current_token_usage = None;
    s.response_time = None;
    s.history_index = None;
    s.temp_input.clear();
    s.status = AppStatus::Idle;
    let count = s.history.len();
    s.history.push(ChatMessage::new(
        "system",
        format!("Resumed session \"{}\" ({} messages)", meta.title, count),
    ));
    crate::config::save_session_history(&s.active_session_id, &s.history);
}

pub fn copy_last_reply(s: &mut AppState) {
    let last_reply = s
        .history
        .iter()
        .rev()
        .find(|m| m.role == "assistant")
        .map(|m| m.content.clone());

    if let Some(content) = last_reply {
        if crate::clipboard::copy_to_clipboard(&content) {
            s.history.push(ChatMessage::new(
                "system",
                "Copied last assistant reply to clipboard",
            ));
        } else {
            s.history
                .push(ChatMessage::new("system", "Failed to copy to clipboard"));
        }
    } else {
        s.history.push(ChatMessage::new(
            "system",
            "No assistant reply found to copy",
        ));
    }
}

pub fn build_help_text() -> String {
    let mut help = String::from("Available Commands:\n");
    for cmd in crate::app::suggestion::COMMANDS {
        help.push_str(&format!("  {} - {}\n", cmd.name, cmd.desc));
    }
    help.push_str("\nKeys:\n");
    help.push_str("  Enter         Send prompt\n");
    help.push_str("  Shift+Enter   Insert newline\n");
    help.push_str("  Esc           Clear input or cancel generation\n");
    help.push_str("  Up/Down       Cycle history\n");
    help.push_str("  Ctrl+P        Open command picker\n");
    help.push_str("  Ctrl+V        Paste image/text from clipboard\n");
    help.push_str("  Ctrl+O        Insert newline\n");
    help.push_str("  Ctrl+L        Clear screen\n");
    help.push_str("  Alt+F/Alt+B   Move cursor word right/left\n");
    help.push_str("  Ctrl+A/Ctrl+E Move cursor to start/end of line\n");
    help.push_str("  Ctrl+U/Ctrl+W Delete line/word\n");
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
        s.config.default = profile.name.clone();
        crate::config::save_entire_config(&s.config);
        s.history.push(ChatMessage::new(
            "system",
            format!("Switched to model profile '{}'", profile.name),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::parse_token_count;

    #[test]
    fn parse_token_count_plain_and_k_suffix() {
        assert_eq!(parse_token_count("262144"), Some(262144));
        assert_eq!(parse_token_count("256k"), Some(256 * 1024));
        assert_eq!(parse_token_count("256K"), Some(256 * 1024));
        assert_eq!(parse_token_count("abc"), None);
        assert_eq!(parse_token_count(""), None);
    }

    #[tokio::test]
    async fn test_goal_command_flow() {
        use crate::app::state::AppState;
        use std::sync::Arc;
        use tokio::sync::Mutex;
        use tokio_util::sync::CancellationToken;

        let state = Arc::new(Mutex::new(AppState::new()));
        let client = reqwest::Client::new();
        let mut cancel_token = CancellationToken::new();

        // Empty goal
        {
            let mut s = state.lock().await;
            s.input_buffer = "/goal ".to_string();
        }
        let trigger = super::handle_enter(&state, &client, &mut cancel_token).await;
        assert!(!trigger);
        {
            let s = state.lock().await;
            assert!(!s.continuous_mode);
            assert!(s.history.last().unwrap().content.contains("Usage:"));
        }

        // Valid goal
        {
            let mut s = state.lock().await;
            s.input_buffer = "/goal fix build issues".to_string();
            s.history.clear();
        }
        let trigger2 = super::handle_enter(&state, &client, &mut cancel_token).await;
        assert!(trigger2);
        {
            let s = state.lock().await;
            assert!(s.continuous_mode);
            assert!(s.history.last().unwrap().content.contains("Goal: fix build issues"));
            assert!(s.history.last().unwrap().content.contains("Continuous autoloop mode is active"));
            assert!(s.input_buffer.is_empty());
        }
    }
}
