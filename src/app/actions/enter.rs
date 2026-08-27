use super::*;
pub async fn handle_enter(
    state: &Arc<Mutex<AppState>>,
    client: &reqwest::Client,
    cancel_token: &mut tokio_util::sync::CancellationToken,
) -> bool {
    let mut s = state.lock().await;
    s.reset_suggestion_cycle();
    s.history_index = None;

    let selected_file_completion = s.active_suggestion_index.is_some()
        && crate::app::get_at_word_query(&s.input_buffer, s.cursor_position).is_some();
    if s.active_suggestion_index.is_some() {
        apply_autocomplete(&mut s);
    }

    // Codex treats accepting a file completion as an edit to the draft, not as
    // prompt submission. A second Enter submits once the user can see the
    // completed path in context.
    if selected_file_completion {
        return false;
    }

    let raw_input = s.input_buffer.trim().to_string();

    if raw_input.is_empty() {
        return false;
    }

    // Record every submitted input for arrow-key recall — plain text and slash
    // commands alike. Consecutive duplicates are collapsed, shell-style.
    if s.input_history.last() != Some(&raw_input) {
        s.input_history.push(raw_input.clone());
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
                let root = s
                    .workspace_root
                    .clone()
                    .or_else(|| std::env::current_dir().ok());
                match tokens.get(1).copied() {
                    None => check_memory_usage(&mut s),
                    Some(_) => {
                        if let Some(message) = crate::memory::command(root.as_deref(), &tokens[1..])
                        {
                            s.history.push(ChatMessage::new("system", message));
                        }
                    }
                }
            }
            "/clear" => {
                let _ = crate::app::session_controller::SessionController::default().clear(&mut s);
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
                let budget = {
                    let s = state_clone.lock().await;
                    s.get_history_token_budget() as usize
                };
                tokio::spawn(async move {
                    match crate::network::compaction::force_compact_with_budget(
                        &client_clone,
                        &api_base_url,
                        &model_name,
                        history_to_compact.as_mut_vec(),
                        Some(budget),
                        Some(&compaction_cancel_token),
                    )
                    .await
                    {
                        Ok((before, after)) => {
                            let mut s = state_clone.lock().await;
                            let live_session_id = s.active_session_id.clone();
                            if try_merge_compacted_history(
                                &live_session_id,
                                s.history.as_mut_vec(),
                                &active_session_id,
                                &original_history,
                                history_to_compact.into_vec(),
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
                                    s.history.as_mut_vec(),
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
                                    s.history.as_mut_vec(),
                                );
                            }
                        }
                    }
                    state_clone.lock().await.request_redraw();
                });
                return false;
            }
            "/quota" => {
                trigger_quota_fetch(&s, state, client);
            }
            "/sync" => {
                let sub = tokens.get(1).map(|s| s.to_string());
                let arg = tokens.get(2).map(|s| s.to_string());
                s.input_buffer.clear();
                s.cursor_position = 0;
                drop(s);
                trigger_sync(state, sub, arg);
                return false;
            }
            "/update" | "/upgrade" => {
                s.input_buffer.clear();
                s.cursor_position = 0;
                s.update_check = crate::update::UpdateState::Checking;
                s.set_notice("🔍 Checking for a RustCode update...");
                s.request_redraw();
                drop(s);
                trigger_update(state, client);
                return false;
            }
            "/new" => {
                cancel_token.cancel();
                *cancel_token = tokio_util::sync::CancellationToken::new();
                let _ = crate::app::session_controller::SessionController::default()
                    .start_fresh(&mut s);
            }
            "/fork" => {
                cancel_token.cancel();
                *cancel_token = tokio_util::sync::CancellationToken::new();
                if let Err(error) = crate::app::session_controller::SessionController::default()
                    .fork(&mut s, crate::app::events::SessionAction::Latest)
                {
                    s.history
                        .push(ChatMessage::new("system", error.to_string()));
                }
            }
            "/archive" => {
                if let Err(error) =
                    crate::app::session_controller::SessionController::default().archive(&mut s)
                {
                    s.history
                        .push(ChatMessage::new("system", error.to_string()));
                }
            }
            "/agents" => {
                s.show_subagent_picker = true;
                s.subagent_picker_index = 0;
            }
            "/delete_chat" => {
                cancel_token.cancel();
                *cancel_token = tokio_util::sync::CancellationToken::new();
                let _ = crate::app::session_controller::SessionController::default()
                    .delete(&mut s, crate::app::events::SessionAction::Latest);
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
            "/yolo" => match tokens.get(1) {
                None => {
                    s.modal_picker_index = if s.auto_confirm { 0 } else { 1 };
                    s.status = AppStatus::YoloPicker;
                }
                Some(&"on") | Some(&"enable") | Some(&"enabled") | Some(&"true") => {
                    s.auto_confirm = true;
                    s.set_notice("YOLO mode enabled");
                }
                Some(&"off") | Some(&"disable") | Some(&"disabled") | Some(&"false") => {
                    s.auto_confirm = false;
                    s.set_notice("YOLO mode disabled");
                }
                Some(&"toggle") => {
                    toggle_auto_confirm(&mut s);
                }
                _ => {
                    s.history.push(ChatMessage::new(
                        "system",
                        "Invalid option. Use 'on', 'off', 'enable', 'disable', or 'toggle'.",
                    ));
                }
            },
            "/verbosity" => {
                use crate::app::state::Verbosity;
                let label = |v: &Verbosity| match v {
                    Verbosity::Low => "low",
                    Verbosity::High => "high",
                };
                let mut changed = false;
                match tokens.get(1) {
                    None => {
                        s.modal_picker_index = match s.verbosity {
                            Verbosity::Low => 0,
                            Verbosity::High => 1,
                        };
                        s.status = AppStatus::VerbosityPicker;
                    }
                    Some(&"low") => {
                        s.verbosity = Verbosity::Low;
                        changed = true;
                        s.history
                            .push(ChatMessage::new("system", "Verbosity set to low."));
                    }
                    Some(&"high") => {
                        s.verbosity = Verbosity::High;
                        changed = true;
                        s.history
                            .push(ChatMessage::new("system", "Verbosity set to high."));
                    }
                    Some(&"toggle") => {
                        s.verbosity = match s.verbosity {
                            Verbosity::Low => Verbosity::High,
                            Verbosity::High => Verbosity::Low,
                        };
                        changed = true;
                        let current = label(&s.verbosity).to_string();
                        s.history.push(ChatMessage::new(
                            "system",
                            format!("Verbosity set to {}.", current),
                        ));
                    }
                    _ => {
                        s.history.push(ChatMessage::new(
                            "system",
                            "Invalid verbosity level. Use 'low', 'high', or 'toggle'.",
                        ));
                    }
                }
                if changed {
                    s.config.verbosity = s.verbosity.clone();
                    crate::config::save_entire_config(&s.config);
                }
            }
            "/thinking" => {
                let url = s.api_base_url.clone();
                let current = s
                    .config
                    .models
                    .iter()
                    .find(|p| p.url == url)
                    .and_then(|p| p.enable_thinking);
                let value = match tokens.get(1) {
                    None => {
                        s.modal_picker_index = match current {
                            Some(false) => 1,
                            _ => 0,
                        };
                        s.status = AppStatus::ThinkingPicker;
                        None
                    }
                    Some(&"on") => Some(Some(true)),
                    Some(&"off") => Some(Some(false)),
                    Some(&"default") => Some(None),
                    _ => {
                        s.history.push(ChatMessage::new(
                            "system",
                            "Invalid option. Use 'on', 'off', or 'default'.",
                        ));
                        None
                    }
                };
                if let Some(value) = value {
                    if let Some(profile) = s.config.models.iter_mut().find(|p| p.url == url) {
                        profile.enable_thinking = value;
                    }
                    crate::config::save_entire_config(&s.config);
                    let label = match value {
                        Some(true) => "Thinking forced on.",
                        Some(false) => "Thinking forced off.",
                        None => "Thinking left at server/Modelfile default.",
                    };
                    s.history.push(ChatMessage::new("system", label));
                }
            }
            "/effort" => {
                let url = s.api_base_url.clone();
                let current = s
                    .config
                    .models
                    .iter()
                    .find(|p| p.url == url)
                    .and_then(|p| p.reasoning_effort.as_deref());
                let value = match tokens.get(1) {
                    None => {
                        s.modal_picker_index = match current {
                            Some("low") => 0,
                            Some("medium") => 1,
                            Some("high") => 2,
                            _ => 3,
                        };
                        s.status = AppStatus::EffortPicker;
                        None
                    }
                    Some(&"low") => Some(Some("low".to_string())),
                    Some(&"med") | Some(&"medium") => Some(Some("medium".to_string())),
                    Some(&"high") => Some(Some("high".to_string())),
                    Some(&"off") | Some(&"none") | Some(&"default") => Some(None),
                    _ => {
                        s.history.push(ChatMessage::new(
                            "system",
                            "Invalid option. Use 'low', 'medium', 'high', or 'off'.",
                        ));
                        None
                    }
                };
                if let Some(value) = value {
                    if let Some(profile) = s.config.models.iter_mut().find(|p| p.url == url) {
                        profile.reasoning_effort = value.clone();
                    }
                    crate::config::save_entire_config(&s.config);
                    let label = match value {
                        Some(ref e) => format!("Reasoning effort set to '{e}'."),
                        None => "Reasoning effort cleared (default).".to_string(),
                    };
                    s.history.push(ChatMessage::new("system", label));
                }
            }
            "/theme" => {
                let themes = crate::ui::theme::load_available_themes();
                match tokens.get(1) {
                    None => {
                        s.theme_picker_initial = s.config.theme.clone();
                        s.theme_picker_index = themes
                            .iter()
                            .position(|t| t.name.eq_ignore_ascii_case(&s.config.theme))
                            .unwrap_or(0);
                        s.show_theme_picker = true;
                    }
                    Some(&theme_name) => {
                        if let Some((idx, theme)) = themes
                            .iter()
                            .enumerate()
                            .find(|(_, t)| t.name.eq_ignore_ascii_case(theme_name))
                        {
                            s.config.theme = theme.name.to_string();
                            s.theme_picker_index = idx;
                            crate::config::save_entire_config(&s.config);
                            s.set_notice(format!("Theme changed to '{}'", theme.name));
                        } else {
                            let names: Vec<String> =
                                themes.iter().map(|t| t.name.clone()).collect();
                            s.history.push(ChatMessage::new(
                                "system",
                                format!(
                                    "Unknown theme '{}'. Available themes: {}.",
                                    theme_name,
                                    names.join(", ")
                                ),
                            ));
                        }
                    }
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
            "/info" | "/about" => {
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
                if let Err(error) = crate::app::session_controller::SessionController::default()
                    .resume(&mut s, crate::app::events::SessionAction::Latest)
                {
                    let message = if matches!(
                        &error,
                        crate::app::session_controller::SessionError::NoSessionToResume
                    ) {
                        "No previous session to resume.".to_owned()
                    } else {
                        error.to_string()
                    };
                    s.history.push(ChatMessage::new("system", message));
                }
            }
            "/history" => {
                let (sessions, truncated) = build_session_list_with_truncation(&s);
                if sessions.is_empty() {
                    s.history
                        .push(ChatMessage::new("system", "No saved sessions found."));
                } else {
                    s.history_picker_sessions = sessions;
                    s.history_picker_index = 0;
                    s.history_picker_truncated = truncated;
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
                    s.show_context_modal = true;
                }
            }
            "/status" => {
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
                    s.modal_picker_index = match active {
                        crate::config::ToolProtocol::Json => 0,
                        crate::config::ToolProtocol::Native => 1,
                        crate::config::ToolProtocol::ApiNative => 2,
                    };
                    s.status = AppStatus::ProtocolPicker;
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
                            enable_thinking: None,
                            reasoning_effort: None,
                            max_tokens: None,
                            supports_vision: None,
                            ..Default::default()
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
                        state_clone.lock().await.request_redraw();
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
                            enable_thinking: None,
                            reasoning_effort: None,
                            max_tokens: None,
                            supports_vision: None,
                            ..Default::default()
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

    if let Some(selected_id) = s.selected_subagent_id {
        let id = crate::app::SubagentId::from_raw(selected_id);
        if let Err(error) = crate::app::SubagentController.send_input(&mut s, id, raw_input.clone())
        {
            s.history
                .push(ChatMessage::new("system", error.to_string()));
            s.request_redraw();
            s.input_buffer.clear();
            s.cursor_position = 0;
            return false;
        }
        s.status = AppStatus::Streaming;
        s.input_buffer.clear();
        s.cursor_position = 0;
        let client_clone = client.clone();
        let state_clone = Arc::clone(state);
        let token_clone = cancel_token.clone();
        drop(s);
        tokio::spawn(async move {
            let result = crate::network::run_subagent(
                &client_clone,
                &state_clone,
                &token_clone,
                selected_id,
            )
            .await;
            let status = if token_clone.is_cancelled() {
                crate::app::SubAgentStatus::Cancelled
            } else if result.is_err() {
                crate::app::SubAgentStatus::Failed
            } else {
                crate::app::SubAgentStatus::Completed
            };
            let mut state = state_clone.lock().await;
            let _ = crate::app::SubagentController.set_status(
                &mut state,
                crate::app::SubagentId::from_raw(selected_id),
                status,
            );
            state.status = AppStatus::Idle;
            state.request_redraw();
        });
        return false;
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
