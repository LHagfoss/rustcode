use crate::app::{AppState, AppStatus, ChatMessage};
use std::sync::Arc;
use sysinfo::{Pid, System};
use tokio::sync::Mutex;

const CHANGELOG_CONTENT: &str = include_str!("../../CHANGELOG.md");

pub fn build_latest_changelog() -> String {
    let mut out = String::new();
    let mut version_count = 0;

    for line in CHANGELOG_CONTENT.lines() {
        if line.starts_with("## [") {
            version_count += 1;
            if version_count > 2 {
                break;
            }
        }
        if version_count > 0 {
            out.push_str(line);
            out.push('\n');
        }
    }

    if out.trim().is_empty() {
        CHANGELOG_CONTENT
            .lines()
            .take(30)
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        out.trim().to_string()
    }
}

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
        if s.config.discord_rpc_enabled {
            let model_name = s.model_name.clone();
            s.discord_rpc
                .set_activity("Idle", &format!("Using model: {}", model_name));
        }
        s.pending_queue.clear();
    } else if !s.pending_queue.is_empty() {
        s.pending_queue.remove(0);
        if s.pending_queue.is_empty() {
            s.status = AppStatus::Idle;
            if s.config.discord_rpc_enabled {
                let model_name = s.model_name.clone();
                s.discord_rpc
                    .set_activity("Idle", &format!("Using model: {}", model_name));
            }
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
            }
            "/summarize" => {
                // summarize_session locks the state itself and runs a full
                // streaming request. handle_enter holds the lock here, so calling
                // it inline deadlocks (re-locking the same mutex) and would freeze
                // the event loop for the whole request. Release the lock, do the
                // usual input cleanup, and run it detached so the UI stays live.
                s.input_buffer.clear();
                s.cursor_position = 0;
                drop(s);
                let state_clone = Arc::clone(state);
                let client_clone = client.clone();
                tokio::spawn(async move {
                    summarize_session(&state_clone, &client_clone).await;
                });
                return false;
            }
            "/compact" => {
                s.input_buffer.clear();
                s.cursor_position = 0;
                if s.history.len() < 2 {
                    s.history.push(ChatMessage::new(
                        "system",
                        "Not enough messages to compact.",
                    ));
                    return false;
                }
                let api_base_url = s.api_base_url.clone();
                let model_name = s.model_name.clone();
                let active_session_id = s.active_session_id.clone();
                let original_history = s.history.clone();
                let mut history_to_compact = original_history.clone();
                let compaction_cancel_token = cancel_token.clone();
                drop(s);
                let state_clone = Arc::clone(state);
                let client_clone = client.clone();
                tokio::spawn(async move {
                    match crate::network::compaction::force_compact(
                        &client_clone,
                        &api_base_url,
                        &model_name,
                        &mut history_to_compact,
                        Some(&compaction_cancel_token),
                    )
                    .await
                    {
                        Ok((before, after)) => {
                            let mut s = state_clone.lock().await;
                            let live_session_id = s.active_session_id.clone();
                            if try_merge_compacted_history(
                                &live_session_id,
                                &mut s.history,
                                &active_session_id,
                                &original_history,
                                history_to_compact,
                            ) {
                                s.history.push(ChatMessage::new(
                                    "system",
                                    format!(
                                        "🧹 History compacted: reduced context from {} to {} tokens.",
                                        before, after
                                    ),
                                ));
                            } else {
                                report_stale_compaction(
                                    &live_session_id,
                                    &active_session_id,
                                    &mut s.history,
                                );
                            }
                        }
                        Err(e) => {
                            let mut s = state_clone.lock().await;
                            let live_session_id = s.active_session_id.clone();
                            if history_matches_snapshot(
                                &live_session_id,
                                &s.history,
                                &active_session_id,
                                &original_history,
                            ) {
                                s.history.push(ChatMessage::new(
                                    "system",
                                    format!("History compaction failed: {}", e),
                                ));
                            } else {
                                report_stale_compaction(
                                    &live_session_id,
                                    &active_session_id,
                                    &mut s.history,
                                );
                            }
                        }
                    }
                });
                return false;
            }
            "/quota" => {
                trigger_quota_fetch(&s, state, client);
            }
            "/update" => {
                s.input_buffer.clear();
                s.cursor_position = 0;
                drop(s);
                trigger_update(state, client);
                return false;
            }
            "/new" => {
                cancel_token.cancel();
                *cancel_token = tokio_util::sync::CancellationToken::new();
                start_new_session(&mut s);
            }
            "/delete_chat" => {
                cancel_token.cancel();
                *cancel_token = tokio_util::sync::CancellationToken::new();
                let session_id = s.active_session_id.clone();
                if let Some(dir) = crate::config::get_active_session_dir(&session_id) {
                    std::fs::remove_dir_all(&dir).ok();
                }
                start_new_session(&mut s);
            }

            "/delegate" => {
                if tokens.get(1).is_some_and(|mode| *mode == "off") {
                    s.delegation_armed = false;
                    s.delegation_active = false;
                    s.history
                        .push(ChatMessage::new("system", "Subagents disabled."));
                } else {
                    s.delegation_armed = true;
                    s.history.push(ChatMessage::new(
                        "system",
                        "Subagents enabled for the next task only. Send your task now.",
                    ));
                }
            }

            "/cancel" => {
                cancel_token.cancel();
                *cancel_token = tokio_util::sync::CancellationToken::new();
            }
            "/verbosity" => {
                if tokens.len() < 2 {
                    s.history.push(ChatMessage::new(
                        "system",
                        "Usage: /verbosity <low|high>",
                    ));
                } else {
                    match tokens[1] {
                        "low" => {
                            s.verbosity = crate::app::state::Verbosity::Low;
                            s.history.push(ChatMessage::new("system", "Verbosity set to low."));
                        }
                        "high" => {
                            s.verbosity = crate::app::state::Verbosity::High;
                            s.history.push(ChatMessage::new("system", "Verbosity set to high."));
                        }
                        _ => {
                            s.history.push(ChatMessage::new(
                                "system",
                                "Invalid verbosity level. Use 'low' or 'high'.",
                            ));
                        }
                    }
                }
            }
            "/discord" => {
                crate::config::save_entire_config(&s.config);
                let is_enabled = s.config.discord_rpc_enabled;
                s.history.push(ChatMessage::new(
                    "system",
                    format!(
                        "Switched Discord Rich Presence to {}",
                        if is_enabled { "ON" } else { "OFF" }
                    ),
                ));
                if is_enabled {
                    s.discord_rpc.set_enabled(true);
                    let model_name = s.model_name.clone();
                    s.discord_rpc
                        .set_activity("Idle", &format!("Using model: {}", model_name));
                } else {
                    s.discord_rpc.set_enabled(false);
                }
            }
            "/goal" => {
                let goal_text = tokens[1..].join(" ");
                if goal_text.trim().is_empty() {
                    s.history.push(ChatMessage::new(
                        "system",
                        "Usage: /goal <task description>",
                    ));
                } else {
                    s.delegation_active = s.delegation_armed;
                    s.delegation_armed = false;
                    s.continuous_mode = true;
                    let goal_msg = format!(
                        "Goal: {}\n\nContinuous autoloop mode is active. You must execute tools in a loop to complete the goal, and call the 'complete_task' tool when you are fully finished.",
                        goal_text
                    );
                    s.history.push(ChatMessage::new("user", goal_msg));
                    crate::config::save_history(&s.history);
                    s.input_buffer.clear();
                    s.cursor_position = 0;
                    return true;
                }
            }
            "/info" => {
                let info = build_info_text();
                s.history.push(ChatMessage::new("system", info));
            }
            "/help" => {
                let help = build_help_text();
                s.history.push(ChatMessage::new("system", help));
            }
            "/exit" | "/quit" => {
                should_exit = true;
            }
            "/skills" => {
                let skills = crate::skills::discover_skills();
                if skills.is_empty() {
                    s.history.push(ChatMessage::new(
                        "system",
                        "No skills discovered.\nPlace SKILL.md files in .rustcode/skills/ or ~/.config/rustcode/skills/",
                    ));
                } else {
                    let mut out = format!("📦 Discovered Skills ({}):\n\n", skills.len());
                    for skill in &skills {
                        out.push_str(&format!("  • {}\n", skill.name));
                        out.push_str(&format!("    Description: {}\n", skill.description));
                        out.push_str(&format!("    Path: {}\n\n", skill.path.display()));
                    }
                    s.history.push(ChatMessage::new("system", out));
                }
            }
            "/changelog" => {
                let log_text = build_latest_changelog();
                s.history.push(ChatMessage::new("assistant", log_text));
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
                    let total = crate::config::list_sessions().len();
                    s.history_picker_truncated = is_session_list_truncated(total);
                    s.show_history_picker = true;
                }
            }
            "/mcp" => {
                s.show_mcp_config = true;
                s.mcp_picker_index = 0;
                s.mcp_edit_state = None;
            }
            "/context" => {
                let default_name = s.config.default.big().to_string();
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
            "/status" => {
                trigger_quota_fetch(&s, state, client);
                let mut text = String::from("Session status");
                let user_msgs = s.history.iter().filter(|m| m.role == "user").count();
                let assistant_msgs = s.history.iter().filter(|m| m.role == "assistant").count();
                let tool_calls = s.history.iter().filter(|m| m.role == "tool").count();
                text.push_str(&format!(
                    "\nMessages: {} user · {} assistant · {} tool calls",
                    user_msgs, assistant_msgs, tool_calls
                ));
                text.push_str(&format!("\nModel: {}", s.model_name));
                text.push_str(&format!("\nSession: {}", s.active_session_id));
                text.push_str("\nQuota: fetching provider status…");
                s.history.push(ChatMessage::new("system", text));
            }
            "/usage" | "/stats" => {
                let mut text = String::from("Session usage:");
                let user_msgs = s.history.iter().filter(|m| m.role == "user").count();
                let assistant_msgs = s.history.iter().filter(|m| m.role == "assistant").count();
                let tool_calls = s.history.iter().filter(|m| m.role == "tool").count();
                text.push_str(&format!(
                    "\nMessages: {} user · {} assistant · {} tool calls",
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
                        if i > 0 && (len - i).is_multiple_of(3) {
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

            "/session" => {
                let session_info = format!(
                    "Session ID: {}\nActive model: {}",
                    s.active_session_id, s.model_name
                );
                s.history.push(ChatMessage::new("system", session_info));
            }
            "/protocol" | "/parser" => {
                if tokens.len() < 2 {
                    let active = s.active_tool_protocol();
                    let url = s.api_base_url.clone();
                    let source = if s
                        .config
                        .models
                        .iter()
                        .any(|profile| profile.url == url && profile.tool_protocol.is_some())
                    {
                        "set for this model"
                    } else if crate::config::provider_supports_function_calling(&url) {
                        "known provider with function calling"
                    } else {
                        match s.function_calling_support.get(&url) {
                            Some(true) => "probed: this endpoint accepts tool schemas",
                            Some(false) => "probed: this endpoint rejects tool schemas",
                            None => "not probed yet — send a message first",
                        }
                    };
                    let msg = format!(
                        "Tool protocol for this model: {active:?} ({source})\n\
Supported formats: json, native, apinative. '/protocol json|native|apinative' sets it for this model.\n\
(apinative = tool schema in the request's `tools` field, structured `tool_calls` back. Preferred wherever it works: a call returned as data cannot be confused with prose describing a call.)"
                    );
                    s.history.push(ChatMessage::new("system", msg));
                } else {
                    let chosen = match tokens[1].to_lowercase().as_str() {
                        "json" => Some((crate::config::ToolProtocol::Json, "JSON (```tool)")),
                        "native" => {
                            Some((crate::config::ToolProtocol::Native, "Native ([TOOL_CALLS])"))
                        }
                        "apinative" | "api" => Some((
                            crate::config::ToolProtocol::ApiNative,
                            "ApiNative (schema in request `tools`, structured `tool_calls` back)",
                        )),
                        _ => None,
                    };
                    match chosen {
                        Some((protocol, label)) => {
                            // Recorded against the model being used, so it survives
                            // model switches and outlives the session.
                            let url = s.api_base_url.clone();
                            let scoped = s
                                .config
                                .models
                                .iter_mut()
                                .find(|profile| profile.url == url)
                                .map(|profile| {
                                    profile.tool_protocol = Some(protocol);
                                    profile.name.clone()
                                });
                            if scoped.is_none() {
                                s.config.tool_protocol = protocol;
                            }
                            crate::config::save_entire_config(&s.config);
                            let scope = scoped
                                .map(|name| format!("for model '{name}'"))
                                .unwrap_or_else(|| "as the fallback for all models".to_string());
                            s.history.push(ChatMessage::new(
                                "system",
                                format!("Switched tool protocol to {label} {scope}."),
                            ));
                        }
                        None => {
                            s.history.push(ChatMessage::new(
                                "system",
                                format!(
                                    "Unknown protocol '{}'. Supported options are 'json', 'native', or 'apinative'.",
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
                text.push_str("\n\nTool execution is guarded by cancellation and loop detection; calls run sequentially.");
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
                        s.config.default.set_big(name.clone());
                        crate::config::save_entire_config(&s.config);
                        s.history.push(ChatMessage::new(
                            "system",
                            format!("Switched to model profile '{}'", name),
                        ));
                        if s.config.discord_rpc_enabled {
                            let model_name = s.model_name.clone();
                            s.discord_rpc
                                .set_activity("Idle", &format!("Using model: {}", model_name));
                        }
                    } else {
                        s.model_name = name.clone();
                        let default_name = s.config.default.big().to_string();
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
                            api_key: None,
                            env_key: None,
                            tool_protocol: None,
                        });
                    }
                    s.config.default.set_big(name.clone());
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

                    let default_name = s.config.default.big().to_string();
                    if let Some(profile) =
                        s.config.models.iter_mut().find(|m| m.name == default_name)
                    {
                        profile.url = url;
                        profile.model = model;
                    }
                    crate::config::save_entire_config(&s.config);

                    let active_default = s.config.default.big().to_string();
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
                            api_key: None,
                            env_key: None,
                            tool_protocol: None,
                        });
                    }
                    s.config.default.set_big("ollama".to_string());
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
                    s.history.push(ChatMessage::new(
                        "system",
                        "Usage:\n  /change_title <title> - Rename the current session",
                    ));
                } else {
                    let new_title = tokens[1..].join(" ");
                    crate::config::save_session_title(&s.active_session_id, &new_title);
                    s.invalidate_session_title_cache();
                    s.history.push(ChatMessage::new(
                        "system",
                        format!("Session title renamed to \"{}\"", new_title),
                    ));
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

    s.delegation_active = s.delegation_armed;
    s.delegation_armed = false;
    s.pending_queue.push(raw_input);
    s.input_buffer.clear();
    s.cursor_position = 0;

    if !s.orchestrator_running {
        s.orchestrator_running = true;
        s.status = AppStatus::Queued;
        let client_clone = client.clone();
        let state_clone = Arc::clone(state);
        let token_clone = cancel_token.clone();
        drop(s);

        tokio::spawn(async move {
            crate::network::process_queue_orchestrator(
                client_clone,
                state_clone,
                token_clone,
                Arc::new(crate::network::policy::InteractivePolicy),
            )
            .await;
        });
    }
    false
}

fn history_matches_snapshot(
    live_session_id: &str,
    live_history: &[ChatMessage],
    captured_session_id: &str,
    captured_history: &[ChatMessage],
) -> bool {
    live_session_id == captured_session_id && live_history.starts_with(captured_history)
}

fn try_merge_compacted_history(
    live_session_id: &str,
    live_history: &mut Vec<ChatMessage>,
    captured_session_id: &str,
    captured_history: &[ChatMessage],
    mut compacted_history: Vec<ChatMessage>,
) -> bool {
    if !history_matches_snapshot(
        live_session_id,
        live_history,
        captured_session_id,
        captured_history,
    ) {
        return false;
    }

    compacted_history.extend(live_history.drain(captured_history.len()..));
    *live_history = compacted_history;
    true
}

fn report_stale_compaction(
    live_session_id: &str,
    captured_session_id: &str,
    history: &mut Vec<ChatMessage>,
) {
    if live_session_id != captured_session_id {
        dbg_log!(
            "Skipping stale compaction notice: active session changed from '{}' to '{}'.",
            captured_session_id,
            live_session_id
        );
        return;
    }

    history.push(ChatMessage::new(
        "system",
        "History compaction discarded as stale: the active session or history changed while compaction was running.",
    ));
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
    } else if let Some((at_idx, at_query)) =
        crate::app::get_at_word_query(&s.input_buffer, s.cursor_position)
    {
        let files = crate::app::list_project_file_paths(&at_query);
        if !files.is_empty() {
            let idx = s
                .active_suggestion_index
                .unwrap_or(0)
                .min(files.len().saturating_sub(1));
            let selected_file = &files[idx];
            let mut new_buf = String::new();
            new_buf.push_str(&s.input_buffer[..at_idx]);
            new_buf.push_str(selected_file);
            new_buf.push(' ');
            let tail_idx = (at_idx + 1 + at_query.len()).min(s.input_buffer.len());
            new_buf.push_str(&s.input_buffer[tail_idx..]);

            s.cursor_position = at_idx + selected_file.len() + 1;
            s.input_buffer = new_buf;
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
            format!("🦀 Current Rustcode RAM usage: {} MB", mem_mb),
        ));
    } else {
        s.history.push(ChatMessage::new(
            "system",
            "Could not find current process.",
        ));
    }
}

pub fn start_new_session(s: &mut AppState) {
    if crate::config::session_has_content(&s.history) {
        crate::config::save_session_history(&s.active_session_id, &s.history);
    }
    s.history.clear();
    s.pending_queue.clear();
    s.current_response.clear();
    s.current_token_usage = None;
    s.response_time = None;
    s.history_index = None;
    s.temp_input.clear();
    s.status = AppStatus::Idle;
    s.subagents.clear();
    s.delegation_armed = false;
    s.delegation_active = false;
    s.next_subagent_id = 1;
    s.todos.clear();
    s.read_file_mtimes.clear();
    s.recent_read_calls.clear();
    s.continuous_mode = false;
    s.tip_index = crate::app::random_tip_index();

    // Switch to a new active session ID
    s.active_session_id = crate::config::create_new_session(&mut s.config);
}

/// Fill in the active profile's context window from the provider when the
/// config doesn't have one (currently: ollama's /api/show). Silent no-op on
/// non-ollama endpoints, errors, or profiles that already have a window set.
pub fn spawn_context_window_detection(state: Arc<Mutex<AppState>>, client: reqwest::Client) {
    tokio::spawn(async move {
        let (name, url, model, engine) = {
            let s = state.lock().await;
            let name = s.config.default.big().to_string();
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
            s.request_redraw();
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
    const MAX_SESSIONS: usize = 50;
    if list.len() > MAX_SESSIONS {
        list.truncate(MAX_SESSIONS);
    }
    list
}

/// Returns whether the session list was truncated at MAX_SESSIONS.
pub fn is_session_list_truncated(total_sessions: usize) -> bool {
    total_sessions > 50
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

fn append_or_update_resume_notice(history: &mut Vec<ChatMessage>, notice: String) {
    if let Some(last) = history.last_mut()
        && last.role == "system"
        && last.content.starts_with("Resumed session ")
    {
        last.content = notice;
    } else {
        history.push(ChatMessage::new("system", notice));
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

    // Save current active session history if it has content
    if crate::config::session_has_content(&s.history) {
        crate::config::save_session_history(&s.active_session_id, &s.history);
    }

    // Extract session ID from the loaded path parent
    if let Some(parent) = meta.path.parent()
        && let Some(session_id_str) = parent.file_name().and_then(|n| n.to_str())
    {
        // Flush the outgoing session's queued history before retargeting.
        crate::config::flush_history();
        s.active_session_id = session_id_str.to_string();
        s.config.last_active_session_id = Some(s.active_session_id.clone());
        crate::config::save_entire_config(&s.config);
        crate::config::set_active_session_id(&s.active_session_id);
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
    append_or_update_resume_notice(
        &mut s.history,
        format!("Resumed session \"{}\" ({} messages)", meta.title, count),
    );
    crate::config::save_session_history(&s.active_session_id, &s.history);
}

pub fn extract_code_blocks_or_content(content: &str) -> String {
    let mut code_lines = Vec::new();
    let mut in_block = false;
    for line in content.lines() {
        if line.trim_start().starts_with("```") {
            in_block = !in_block;
            continue;
        }
        if in_block {
            code_lines.push(line);
        }
    }
    if !code_lines.is_empty() {
        code_lines.join("\n")
    } else {
        content.to_string()
    }
}

pub fn copy_last_reply(s: &mut AppState) {
    let last_reply = s
        .history
        .iter()
        .rev()
        .find(|m| m.role == "assistant")
        .map(|m| m.content.clone());

    if let Some(content) = last_reply {
        let clean_text = extract_code_blocks_or_content(&content);
        if crate::clipboard::copy_to_clipboard(&clean_text) {
            s.last_copy_time = Some(std::time::Instant::now());
            s.history.push(ChatMessage::new(
                "system",
                "Copied code/reply to clipboard ✅",
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

/// Max transcript characters sent to the summarizer. Beyond this we keep the
/// most recent content so a long session still summarizes without blowing the
/// model's context.
const MAX_SUMMARY_TRANSCRIPT_CHARS: usize = 16_000;
/// Tool outputs are the bulk of a session's bytes but low signal for a summary;
/// keep only a head of each so the transcript stays small and fast.
const MAX_SUMMARY_TOOL_CHARS: usize = 300;

pub async fn summarize_session(state_arc: &Arc<Mutex<AppState>>, client: &reqwest::Client) {
    let started = std::time::Instant::now();
    let (api_base_url, model_name, transcript) = {
        let mut s = state_arc.lock().await;

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
        s.current_response.clear();

        (s.api_base_url.clone(), s.model_name.clone(), transcript)
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
        &messages,
        Arc::clone(&stream_buffer),
        true, // quiet: don't stream into the main chat view; we post the result
    )
    .await;

    let summary_content = stream_buffer.lock().await.content.clone();
    let elapsed = started.elapsed().as_secs_f32();

    let mut s = state_arc.lock().await;
    s.status = AppStatus::Idle;
    s.generation_start_time = None;
    s.current_response.clear();

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
}

pub fn build_info_text() -> String {
    format!(
        "Notice: rustcode v{}\n\n\
        Description: A terminal-based coding assistant.\n\n\
        Basic Slash Commands:\n\
        \x20 /changelog - View the latest changes.\n\
        \x20 /update    - Upgrade rustcode via Homebrew if a newer version exists.\n\
        \x20 /help      - Get help on commands and keybindings.\n\
        \x20 /tools     - List available tools for the harness.\n",
        env!("CARGO_PKG_VERSION")
    )
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
        s.config.default.set_big(profile.name.clone());
        crate::config::save_entire_config(&s.config);
        s.history.push(ChatMessage::new(
            "system",
            format!("Switched to model profile '{}'", profile.name),
        ));
    }
}

/// Check the Homebrew tap for a newer rustcode and, if one exists, run
/// `brew upgrade rustcode`. Spawns its own task so the UI stays live during the
/// network fetch and the (potentially slow) brew upgrade. Progress and results
/// are pushed into the chat history as system messages.
pub fn trigger_update(state: &Arc<Mutex<AppState>>, client: &reqwest::Client) {
    let state_clone = Arc::clone(state);
    let client_clone = client.clone();
    tokio::spawn(async move {
        {
            let mut s = state_clone.lock().await;
            s.update_check = crate::update::UpdateState::Checking;
            s.history.push(ChatMessage::new(
                "system",
                "Checking lhagfoss/tap for updates...",
            ));
        }

        let check = match crate::update::check_for_update(&client_clone).await {
            Ok(check) => check,
            Err(_) => {
                let mut s = state_clone.lock().await;
                s.update_check = crate::update::UpdateState::Failed;
                s.history.push(ChatMessage::new(
                    "system",
                    "Update check failed: couldn't read the Homebrew tap. Try: brew upgrade rustcode",
                ));
                return;
            }
        };

        let (current, latest) = match check {
            crate::update::UpdateCheck::Available { current, latest } => (current, latest),
            crate::update::UpdateCheck::UpToDate { current, latest } => {
                let mut s = state_clone.lock().await;
                s.update_check = crate::update::UpdateState::UpToDate(latest);
                s.history.push(ChatMessage::new(
                    "system",
                    format!(
                        "rustcode v{} is up to date.",
                        crate::update::format_version(current)
                    ),
                ));
                return;
            }
        };

        {
            let mut s = state_clone.lock().await;
            s.update_check = crate::update::UpdateState::Available(latest);
            s.history.push(ChatMessage::new(
                "system",
                format!(
                    "Update available!: run `brew upgrade rustcode` or /update in rustcode\n  v{} -> v{}\nRunning brew upgrade rustcode...",
                    crate::update::format_version(current),
                    crate::update::format_version(latest)
                ),
            ));
        }

        let result = tokio::task::spawn_blocking(crate::update::run_brew_upgrade).await;
        let mut s = state_clone.lock().await;
        let msg = match result {
            Ok(Ok(())) => format!(
                "Installed rustcode v{}. Restart rustcode to get the new version.",
                crate::update::format_version(latest)
            ),
            Ok(Err(e)) => {
                format!("brew upgrade failed: {e}\nRun manually: brew upgrade rustcode")
            }
            Err(e) => format!("update task error: {e}"),
        };
        s.history.push(ChatMessage::new("system", msg));
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
    });
}

fn append_codex_rate_limits(text: &mut String, rate_limits: &serde_json::Value) {
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

#[cfg(test)]
mod tests {
    use super::parse_token_count;

    async fn pending_response_server() -> (String, tokio::sync::oneshot::Receiver<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test server");
        let address = listener.local_addr().expect("test server address");
        let (accepted_tx, accepted_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (socket, _) = listener.accept().await.expect("accept request");
            accepted_tx.send(()).ok();
            std::future::pending::<()>().await;
            drop(socket);
        });
        (format!("http://{address}"), accepted_rx)
    }

    #[test]
    fn parse_token_count_plain_and_k_suffix() {
        assert_eq!(parse_token_count("262144"), Some(262144));
        assert_eq!(parse_token_count("256k"), Some(256 * 1024));
        assert_eq!(parse_token_count("256K"), Some(256 * 1024));
        assert_eq!(parse_token_count("abc"), None);
        assert_eq!(parse_token_count(""), None);
    }

    #[test]
    fn render_codex_rate_limit_windows() {
        let mut text = String::from("Session usage:");
        let limits = serde_json::json!({
            "primary": {"used_percent": 20.0, "window_minutes": 300, "resets_at": 1_700_000_000_i64},
            "secondary": {"used_percent": 50.0, "limit_window_seconds": 86400}
        });

        super::append_codex_rate_limits(&mut text, &limits);

        assert!(text.contains("ChatGPT primary (5h): 80.0% remaining"));
        assert!(text.contains("ChatGPT secondary (1d): 50.0% remaining"));
        assert!(text.contains("resets "));
    }

    #[test]
    fn resume_notice_updates_trailing_notice_instead_of_growing_history() {
        let mut history = vec![
            crate::app::ChatMessage::new("user", "hello"),
            crate::app::ChatMessage::new("system", "Resumed session \"demo\" (1 messages)"),
        ];

        super::append_or_update_resume_notice(
            &mut history,
            "Resumed session \"demo\" (2 messages)".to_string(),
        );

        assert_eq!(history.len(), 2);
        assert_eq!(
            history.last().unwrap().content,
            "Resumed session \"demo\" (2 messages)"
        );
    }

    #[test]
    fn resume_notice_is_appended_after_real_conversation_content() {
        let mut history = vec![crate::app::ChatMessage::new("user", "hello")];

        super::append_or_update_resume_notice(
            &mut history,
            "Resumed session \"demo\" (1 messages)".to_string(),
        );

        assert_eq!(history.len(), 2);
    }

    #[test]
    fn manual_compaction_discards_result_after_session_only_change() {
        let original = vec![
            crate::app::ChatMessage::new("user", "original task"),
            crate::app::ChatMessage::new("assistant", "original response"),
        ];
        let captured_history = original.clone();
        let mut live_history = original;
        let expected = live_history.clone();
        let compacted = vec![crate::app::ChatMessage::new(
            "system",
            "compacted old session",
        )];

        let applied = super::try_merge_compacted_history(
            "new-session",
            &mut live_history,
            "old-session",
            &captured_history,
            compacted,
        );

        assert!(!applied);
        assert!(live_history == expected);
    }

    #[test]
    fn manual_compaction_discards_result_after_token_usage_change() {
        let original = vec![
            crate::app::ChatMessage::new("user", "original task"),
            crate::app::ChatMessage::new("assistant", "original response"),
        ];
        let captured_history = original.clone();
        let mut live_history = original;
        live_history[1].token_usage = Some(crate::app::TokenUsage {
            prompt_tokens: 12,
            completion_tokens: 8,
            total_tokens: 20,
            cached_tokens: Some(4),
        });
        let expected = live_history.clone();
        let compacted = vec![crate::app::ChatMessage::new("system", "compacted history")];

        let applied = super::try_merge_compacted_history(
            "active-session",
            &mut live_history,
            "active-session",
            &captured_history,
            compacted,
        );

        assert!(!applied);
        assert!(live_history == expected);
    }

    #[test]
    fn manual_compaction_stale_report_skips_new_session_history() {
        let mut live_history = vec![crate::app::ChatMessage::new("user", "new session task")];
        let expected = live_history.clone();

        super::report_stale_compaction("new-session", "old-session", &mut live_history);

        assert!(live_history == expected);
    }

    #[test]
    fn manual_compaction_stale_report_preserves_same_session_history() {
        let mut live_history = vec![crate::app::ChatMessage::new(
            "assistant",
            "response completed while compaction ran",
        )];
        live_history[0].response_time_ms = Some(250);
        let expected = live_history.clone();

        super::report_stale_compaction("active-session", "active-session", &mut live_history);

        assert!(live_history[..expected.len()] == expected);
        assert_eq!(live_history.len(), expected.len() + 1);
        assert!(
            live_history
                .last()
                .unwrap()
                .content
                .contains("discarded as stale")
        );
    }

    #[test]
    fn manual_compaction_preserves_messages_appended_to_original_prefix() {
        let original = vec![
            crate::app::ChatMessage::new("user", "original task"),
            crate::app::ChatMessage::new("assistant", "original response"),
        ];
        let captured_history = original.clone();
        let mut live_history = original;
        let appended = crate::app::ChatMessage::new("user", "message appended during compaction");
        live_history.push(appended.clone());
        let compacted = vec![crate::app::ChatMessage::new("system", "compacted history")];
        let expected = vec![compacted[0].clone(), appended];

        let applied = super::try_merge_compacted_history(
            "active-session",
            &mut live_history,
            "active-session",
            &captured_history,
            compacted,
        );

        assert!(applied);
        assert!(live_history == expected);
    }

    #[tokio::test]
    async fn manual_compaction_cancellation_interrupts_detached_request() {
        use crate::app::state::AppState;
        use std::sync::Arc;
        use std::time::Duration;
        use tokio::sync::Mutex;
        use tokio_util::sync::CancellationToken;

        let (url, request_accepted) = pending_response_server().await;
        let mut app = AppState::new();
        app.api_base_url = url;
        app.model_name = "model".to_string();
        app.history = (0..8)
            .map(|index| crate::app::ChatMessage::new("user", format!("message {index}")))
            .collect();
        app.input_buffer = "/compact".to_string();
        let state = Arc::new(Mutex::new(app));
        let client = reqwest::Client::new();
        let mut cancel_token = CancellationToken::new();
        let compact_token = cancel_token.clone();

        assert!(!super::handle_enter(&state, &client, &mut cancel_token).await);
        tokio::time::timeout(Duration::from_secs(10), request_accepted)
            .await
            .expect("manual compaction request must start")
            .expect("manual compaction server must signal acceptance");

        {
            let mut app = state.lock().await;
            app.input_buffer = "/cancel".to_string();
        }
        assert!(!super::handle_enter(&state, &client, &mut cancel_token).await);
        assert!(compact_token.is_cancelled());
        assert!(!cancel_token.is_cancelled());

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if state
                    .lock()
                    .await
                    .history
                    .iter()
                    .any(|message| message.content.starts_with("History compaction failed:"))
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("cancelling the active token must stop manual compaction");
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
            assert!(
                s.history
                    .last()
                    .unwrap()
                    .content
                    .contains("Goal: fix build issues")
            );
            assert!(
                s.history
                    .last()
                    .unwrap()
                    .content
                    .contains("Continuous autoloop mode is active")
            );
            assert!(s.input_buffer.is_empty());
        }
    }
}
