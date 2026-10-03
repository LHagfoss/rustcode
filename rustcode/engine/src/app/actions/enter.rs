use super::*;
#[allow(dead_code)]
pub async fn handle_enter(
    state: &Arc<Mutex<AppState>>,
    client: &reqwest::Client,
    cancel_token: &mut tokio_util::sync::CancellationToken,
    theme_names: &dyn Fn() -> Vec<String>,
) -> bool {
    handle_enter_inner(state, client, cancel_token, None, theme_names).await
}

pub async fn handle_enter_with_ui_events(
    state: &Arc<Mutex<AppState>>,
    client: &reqwest::Client,
    cancel_token: &mut tokio_util::sync::CancellationToken,
    ui_events: crate::network::ui_adapter::AgentUiEventSender,
    theme_names: &dyn Fn() -> Vec<String>,
) -> bool {
    handle_enter_inner(state, client, cancel_token, Some(ui_events), theme_names).await
}

async fn handle_enter_inner(
    state: &Arc<Mutex<AppState>>,
    client: &reqwest::Client,
    cancel_token: &mut tokio_util::sync::CancellationToken,
    ui_events: Option<crate::network::ui_adapter::AgentUiEventSender>,
    theme_names: &dyn Fn() -> Vec<String>,
) -> bool {
    let mut s = state.lock().await;
    s.reset_suggestion_cycle();
    s.history_index = None;

    let selected_file_completion = s.active_suggestion_index.is_some()
        && rustcode_core::input::get_at_word_query(&s.input_buffer, s.cursor_position).is_some();
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
        // Commands are dispatched independently of the draft submission mode;
        // the next text draft starts in the default mode.
        s.draft_submit_mode = crate::app::state::DraftSubmitMode::Steer;
        let tokens: Vec<&str> = raw_input.split_whitespace().collect();
        if tokens.is_empty() {
            s.input_buffer.clear();
            s.cursor_position = 0;
            return false;
        }

        let cmd = tokens[0];
        s.overlays().close_all();
        let mut should_exit = false;

        match cmd {
            "/prompts" => {
                let roots = prompt_command_roots(&s);
                let content = prompt_command_catalog(&roots);
                s.show_command_panel("Prompt commands", content);
            }
            "/prompt" => {
                let roots = prompt_command_roots(&s);
                let input = s.input_buffer.clone();
                stage_prompt_command(&mut s, &input, &roots);
            }
            "/memory" => {
                let root = s.effective_workspace_root();
                match tokens.get(1).copied() {
                    None => check_memory_usage(&mut s),
                    Some(_) => {
                        if let Some(message) = crate::memory::command(root.as_deref(), &tokens[1..])
                        {
                            s.show_command_panel("Memory", message);
                        }
                    }
                }
            }
            "/clear" => {
                let _ = crate::app::session_controller::SessionController::default().clear(&mut s);
            }
            "/recap" => {
                s.input_buffer.clear();
                s.cursor_position = 0;
                drop(s);
                let state_clone = Arc::clone(state);
                let client_clone = client.clone();
                tokio::spawn(async move {
                    generate_conversation_recap(&state_clone, &client_clone, true).await;
                });
                return false;
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
                s.show_command_panel("Model quota", "Fetching model quota…");
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
                s.update_check = rustcode_core::update::UpdateState::Checking;
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
                    s.show_command_panel("Subagents", "Subagents disabled.");
                } else {
                    s.delegation_armed = true;
                    s.show_command_panel(
                        "Subagents",
                        "Subagents enabled for the next task only. Send your task now.",
                    );
                }
            }

            "/workspace" => handle_workspace_command(&mut s, &tokens),

            "/pwd" => {
                let root = s.effective_workspace_root().unwrap_or_else(|| {
                    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
                });
                s.show_command_panel(
                    "Workspace path",
                    crate::app::actions::workspace_inspection::cwd_text(&root),
                );
            }
            "/diff" => {
                let expected_workspace_root = s.effective_workspace_root();
                let root = expected_workspace_root.clone().unwrap_or_else(|| {
                    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
                });
                let session_id = s.active_session_id.clone();
                let generation =
                    s.show_command_panel_request("Git diff", "Reading workspace changes…");
                s.input_buffer.clear();
                s.cursor_position = 0;
                drop(s);
                crate::app::actions::workspace_inspection::trigger(
                    state,
                    root,
                    expected_workspace_root,
                    session_id,
                    generation,
                );
                return false;
            }

            "/cancel" => {
                cancel_token.cancel();
                *cancel_token = tokio_util::sync::CancellationToken::new();
            }
            "/ps" => {
                let text = background_terminal_list(&s.active_session_id);
                // Polling /ps while a job runs must not append one system
                // message per poll (issue #1222): collapse repeats in place.
                s.show_command_panel("Background terminals", text);
            }
            "/stop" => {
                let text = stop_background_terminals(&s.active_session_id);
                s.history.push(ChatMessage::new("system", text));
                s.request_redraw();
            }
            "/yolo" => match tokens.get(1) {
                None => {
                    s.modal_picker_index = if s.auto_confirm { 0 } else { 1 };
                    s.settings_picker = Some(crate::app::SettingsPicker::Yolo);
                }
                Some(&"on") | Some(&"enable") | Some(&"enabled") | Some(&"true") => {
                    s.auto_confirm = true;
                    s.set_transient_notice("YOLO mode enabled");
                }
                Some(&"off") | Some(&"disable") | Some(&"disabled") | Some(&"false") => {
                    s.auto_confirm = false;
                    s.set_transient_notice("YOLO mode disabled");
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
            "/sandbox" => {
                let current = s.effective_sandbox_mode();
                match tokens.get(1).copied() {
                    None => {
                        // Two spaces separate the columns so the panel renderer
                        // owns the label column: Markdown collapses the padding a
                        // `format!` width would give, and `•` keeps the current
                        // marker out of the list syntax it used to look like.
                        let modes = crate::config::SandboxMode::ALL
                            .iter()
                            .map(|mode| {
                                let marker = if *mode == current { "•" } else { " " };
                                format!(
                                    " {marker} /sandbox {}  {}",
                                    mode.as_str(),
                                    mode.description()
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                        s.show_command_panel("OS sandbox", format!(
                            "OS sandbox mode: {} ({})\n{modes}\nThe current mode is marked with •. `trusted` is the default and YOLO override: tools run with RustCode process permissions (no OS sandbox) and the shell approval policy still applies. This is a user-level setting; a project config file cannot change it.\nRestricted command failures name effective permissions and possible sandbox restrictions.",
                            current.description(), current.effective_description()
                        ));
                    }
                    Some(mode) => {
                        match crate::config::SandboxMode::ALL
                            .into_iter()
                            .find(|candidate| {
                                candidate.as_str() == mode
                                    || (mode == "unrestricted" && candidate.is_trusted())
                            }) {
                            Some(selected) => {
                                s.config.sandbox_mode = selected;
                                crate::config::save_entire_config(&s.config);
                                let effective = selected.effective_description();
                                let note = if selected.is_trusted() {
                                    " Commands run with RustCode process permissions; shell approval policy still applies separately."
                                } else {
                                    ""
                                };
                                s.show_command_panel(
                                    "OS sandbox",
                                    format!(
                                        "OS sandbox mode set to {} ({effective}){note}",
                                        selected.as_str()
                                    ),
                                );
                            }
                            None => s.show_command_panel(
                                "OS sandbox",
                                format!(
                                    "Invalid option `{mode}`. Use {}.",
                                    crate::config::SandboxMode::ALL
                                        .iter()
                                        .map(|candidate| candidate.as_str())
                                        .collect::<Vec<_>>()
                                        .join(", ")
                                ),
                            ),
                        }
                    }
                }
            }
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
                        s.settings_picker = Some(crate::app::SettingsPicker::Verbosity);
                    }
                    Some(&"low") => {
                        s.verbosity = Verbosity::Low;
                        changed = true;
                        s.show_command_panel("Output verbosity", "Verbosity set to low.");
                    }
                    Some(&"high") => {
                        s.verbosity = Verbosity::High;
                        changed = true;
                        s.show_command_panel("Output verbosity", "Verbosity set to high.");
                    }
                    Some(&"toggle") => {
                        s.verbosity = match s.verbosity {
                            Verbosity::Low => Verbosity::High,
                            Verbosity::High => Verbosity::Low,
                        };
                        changed = true;
                        let current = label(&s.verbosity).to_string();
                        s.show_command_panel(
                            "Output verbosity",
                            format!("Verbosity set to {}.", current),
                        );
                    }
                    _ => {
                        s.show_command_panel(
                            "Output verbosity",
                            "Invalid verbosity level. Use 'low', 'high', or 'toggle'.",
                        );
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
                        s.settings_picker = Some(crate::app::SettingsPicker::Thinking);
                        None
                    }
                    Some(&"on") => Some(Some(true)),
                    Some(&"off") => Some(Some(false)),
                    Some(&"default") => Some(None),
                    _ => {
                        s.show_command_panel(
                            "Thinking",
                            "Invalid option. Use 'on', 'off', or 'default'.",
                        );
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
                    s.show_command_panel("Thinking", label);
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
                        s.settings_picker = Some(crate::app::SettingsPicker::Effort);
                        None
                    }
                    Some(&"low") => Some(Some("low".to_string())),
                    Some(&"med") | Some(&"medium") => Some(Some("medium".to_string())),
                    Some(&"high") => Some(Some("high".to_string())),
                    Some(&"off") | Some(&"none") | Some(&"default") => Some(None),
                    _ => {
                        s.show_command_panel(
                            "Reasoning effort",
                            "Invalid option. Use 'low', 'medium', 'high', or 'off'.",
                        );
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
                    s.show_command_panel("Reasoning effort", label);
                }
            }
            // Theme browsing renders a picker in the terminal frontend, which
            // supplies the available names (reading theme files is a UI
            // concern; this match only needs the names to pick and persist).
            "/theme" => {
                let themes: Vec<String> = theme_names();
                match tokens.get(1) {
                    None => {
                        s.theme_picker_initial = s.config.theme.clone();
                        s.theme_picker_index = themes
                            .iter()
                            .position(|t| t.eq_ignore_ascii_case(&s.config.theme))
                            .unwrap_or(0);
                        s.show_theme_picker = true;
                    }
                    Some(&theme_name) => {
                        if let Some((idx, theme)) = themes
                            .iter()
                            .enumerate()
                            .find(|(_, t)| t.eq_ignore_ascii_case(theme_name))
                        {
                            s.config.theme = theme.clone();
                            s.theme_picker_index = idx;
                            crate::config::save_entire_config(&s.config);
                            s.show_command_panel("Theme", format!("Theme changed to '{theme}'"));
                        } else {
                            s.show_command_panel(
                                "Theme",
                                format!(
                                    "Unknown theme '{}'. Available themes: {}.",
                                    theme_name,
                                    themes.join(", ")
                                ),
                            );
                        }
                    }
                }
            }

            "/goal" => {
                let goal_text = tokens[1..].join(" ");
                if goal_text.trim().is_empty() {
                    s.show_command_panel("Goal", "Usage: /goal <task description>");
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
                s.show_command_panel("About RustCode", info);
            }
            "/help" => {
                let help = build_help_text();
                s.show_command_panel("Help", help);
            }
            "/exit" | "/quit" => {
                should_exit = true;
            }
            "/skills" => {
                let skills = crate::skills::discover_skills();
                s.show_command_panel("Skills", crate::skills::format_skill_catalog(&skills));
            }
            "/changelog" => {
                let log_text = build_latest_changelog();
                s.show_command_panel("Changelog", log_text);
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
            "/continue" => {
                let queued = crate::app::actions::session::queue_restored_segment(&mut s);
                let message = if queued {
                    "Queued the pending session work."
                } else {
                    "No pending session work is available to continue."
                };
                s.history.push(ChatMessage::new("system", message));
            }
            "/history" => {
                let (sessions, truncated) = build_session_list_with_truncation(&s);
                if sessions.is_empty() {
                    s.show_command_panel("History", "No saved sessions found.");
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
                                s.show_command_panel(
                                    "Context",
                                    format!(
                                        "Set context window for profile '{}' to {} tokens",
                                        default_name, n
                                    ),
                                );
                            } else {
                                s.show_command_panel(
                                    "Context",
                                    "No active profile to set context window on.",
                                );
                            }
                        }
                        None => {
                            s.show_command_panel(
                                "Context",
                                "Usage: /context <tokens> - e.g. /context 262144 or /context 256k",
                            );
                        }
                    }
                } else {
                    s.show_context_modal = true;
                }
            }
            "/status" => {
                s.show_status_modal = true;
            }
            "/discord" => {
                let argument = tokens.get(1).copied();
                if tokens.len() > 2
                    || matches!(argument, Some(value) if !matches!(value, "on" | "off" | "status"))
                {
                    s.show_command_panel(
                        "Discord Rich Presence",
                        "Usage: /discord [on|off|status]\nBare /discord toggles Rich Presence.",
                    );
                } else {
                    let ipc = if crate::discord_rpc::ipc_socket_detected() {
                        "detected"
                    } else {
                        "not detected (Discord may be closed)"
                    };
                    match argument {
                        Some("status") => {
                            let state = if s.config.discord_rpc_enabled {
                                "enabled"
                            } else {
                                "disabled"
                            };
                            s.show_command_panel(
                                "Discord Rich Presence",
                                format!(
                                    "Rich Presence is {state}.\nDiscord desktop IPC: {ipc}\n\nUse /discord on or /discord off to change this setting."
                                ),
                            );
                        }
                        Some("on") | Some("off") | None => {
                            let enabled = match argument {
                                Some("on") => true,
                                Some("off") => false,
                                None => !s.config.discord_rpc_enabled,
                                _ => unreachable!("Discord arguments were validated"),
                            };
                            let previous = s.config.discord_rpc_enabled;
                            if enabled != previous {
                                s.config.discord_rpc_enabled = enabled;
                                if let Err(error) = crate::config::save_discord_rpc_enabled(
                                    &s.config,
                                    s.effective_workspace_root().as_deref(),
                                ) {
                                    s.config.discord_rpc_enabled = previous;
                                    s.show_command_panel(
                                        "Discord Rich Presence",
                                        format!(
                                            "Could not save Discord Rich Presence setting: {error}"
                                        ),
                                    );
                                    s.input_buffer.clear();
                                    s.cursor_position = 0;
                                    return false;
                                }
                            }
                            let state = if enabled { "enabled" } else { "disabled" };
                            let action = if enabled == previous {
                                format!("Rich Presence is already {state}.")
                            } else {
                                format!("Rich Presence is now {state}.")
                            };
                            s.show_command_panel(
                                "Discord Rich Presence",
                                format!(
                                    "{action}\nDiscord desktop IPC: {ipc}\n\nUse /discord on or /discord off to change this setting."
                                ),
                            );
                        }
                        Some(_) => unreachable!("Discord arguments were validated"),
                    }
                }
            }
            "/usage" | "/stats" => {
                s.open_stats_modal();
            }

            "/session" => {
                s.show_session_modal = true;
            }
            "/protocol" | "/parser" => {
                if tokens.len() < 2 {
                    let active = s.active_tool_protocol();
                    s.modal_picker_index = match active {
                        crate::config::ToolProtocol::Json => 0,
                        crate::config::ToolProtocol::Native => 1,
                        crate::config::ToolProtocol::ApiNative => 2,
                    };
                    s.settings_picker = Some(crate::app::SettingsPicker::Protocol);
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
                            s.show_command_panel(
                                "Tool protocol",
                                format!("Switched tool protocol to {label} {scope}."),
                            );
                        }
                        None => {
                            s.show_command_panel("Tool protocol", format!(
                                    "Unknown protocol '{}'. Supported options are 'json', 'native', or 'apinative'.",
                                    tokens[1]
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
                s.show_command_panel("Tools", text);
            }
            "/model" | "/models" => {
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
                        s.show_command_panel(
                            "Model",
                            format!("Switched to model profile '{}'", name),
                        );
                    } else {
                        s.model_name = name.clone();
                        let default_name = s.config.default.big().to_string();
                        if let Some(profile) =
                            s.config.models.iter_mut().find(|m| m.name == default_name)
                        {
                            profile.model = name.clone();
                        }
                        crate::config::save_entire_config(&s.config);
                        s.show_command_panel(
                            "Model",
                            format!("Switched active model to '{}'", name),
                        );
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
                    s.show_command_panel(
                        "Provider",
                        format!("Created/updated profile '{}' and set as default", name),
                    );
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
                    s.show_command_panel(
                        "Provider",
                        format!(
                            "Updated active profile '{}' with URL '{}' and model '{}'",
                            active_default, active_url, active_model
                        ),
                    );
                } else {
                    s.show_command_panel("Provider", "Usage:\n  /provider <name> <url> <model> [context_window] - Create/update profile\n  /provider <url> <model> - Update active profile");
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

                    s.show_command_panel(
                        "Ollama",
                        format!("Fetching Ollama models from '{}'...", tags_url),
                    );

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
                                                s.update_command_panel(
                                                    "Ollama",
                                                    "Ollama returned no models.",
                                                );
                                            } else {
                                                s.update_command_panel(
                                                    "Ollama",
                                                    format!(
                                                        "Available Ollama models:\n  {}",
                                                        names.join("\n  ")
                                                    ),
                                                );
                                            }
                                        }
                                        Err(e) => {
                                            let mut s = state_clone.lock().await;
                                            s.update_command_panel(
                                                "Ollama",
                                                format!(
                                                    "Failed to parse Ollama tags response: {}",
                                                    e
                                                ),
                                            );
                                        }
                                    }
                                } else {
                                    let mut s = state_clone.lock().await;
                                    s.update_command_panel(
                                        "Ollama",
                                        format!("Ollama returned status code: {}", res.status()),
                                    );
                                }
                            }
                            Err(e) => {
                                let mut s = state_clone.lock().await;
                                s.update_command_panel(
                                    "Ollama",
                                    format!("Failed to fetch Ollama models: {}", e),
                                );
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
                    s.show_command_panel(
                        "Ollama",
                        "Switched to profile 'ollama' and updated its URL and model",
                    );
                } else {
                    s.show_command_panel("Ollama", "Usage:\n  /ollama list [url] - List available models\n  /ollama <url> <model> - Set 'ollama' profile URL and model");
                }
            }
            "/change_title" => {
                if tokens.len() < 2 {
                    s.show_command_panel(
                        "Session title",
                        "Usage:\n  /change_title <title> - Rename the current session",
                    );
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
                s.show_command_panel("Command", format!("Unknown command: {}", cmd));
            }
        }

        if matches!(cmd, "/model" | "/models" | "/provider" | "/ollama") {
            spawn_context_window_detection(Arc::clone(state), client.clone());
        }

        // /prompt replaces the draft with editable template text. Preserve it
        // instead of applying the normal slash-command cleanup below.
        if cmd == "/prompt" {
            return false;
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
        let owner_session_id = s.active_session_id.clone();
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
                &owner_session_id,
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
            state.enter_idle();
            state.request_redraw();
        });
        return false;
    }

    let submit_outcome = super::submit_plain_prompt(&mut s, raw_input);
    if submit_outcome != super::SubmitOutcome::Queued {
        return false;
    }

    let token_clone = cancel_token.clone();
    let client_clone = client.clone();
    let state_clone = Arc::clone(state);
    drop(s);
    let ui_events = ui_events.unwrap_or_else(|| {
        let (sender, _receiver) = crate::network::ui_adapter::AgentUiEventSender::channel();
        sender
    });
    crate::controller::spawn_observed_orchestrator(
        client_clone,
        state_clone,
        token_clone,
        ui_events,
    )
    .await;
    false
}

fn prompt_command_roots(state: &AppState) -> crate::prompt_commands::Roots {
    crate::prompt_commands::Roots {
        workspace: state.effective_workspace_root(),
        config_dir: crate::config::get_config_dir(),
    }
}

fn prompt_command_catalog(roots: &crate::prompt_commands::Roots) -> String {
    match crate::prompt_commands::list(roots) {
        Ok(commands) if commands.is_empty() => format!(
            "No prompt templates found. Add direct-child Markdown files to:\n  {}\n  {}",
            roots
                .workspace
                .as_ref()
                .map(|path| path.join(".rustcode/commands").display().to_string())
                .unwrap_or_else(|| "<workspace>/.rustcode/commands".to_owned()),
            roots
                .config_dir
                .as_ref()
                .map(|path| path.join("commands").display().to_string())
                .unwrap_or_else(|| "<config dir>/commands".to_owned())
        ),
        Ok(commands) => {
            let entries = commands
                .into_iter()
                .map(|command| {
                    let source = if roots.workspace.as_ref().is_some_and(|workspace| {
                        command
                            .path
                            .starts_with(workspace.join(".rustcode/commands"))
                    }) {
                        "workspace"
                    } else {
                        "user"
                    };
                    format!(
                        "- `/prompt {}` — {source}: `{}`",
                        command.name,
                        command.path.display()
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            format!(
                "Prompt templates (workspace files override user files with the same name):\n\n{entries}\n\nUse `/prompt <name> [arguments]` to load a template into the composer for review. `$ARGUMENTS` is replaced literally; without a placeholder, arguments are appended."
            )
        }
        Err(error) => error,
    }
}

fn stage_prompt_command(state: &mut AppState, input: &str, roots: &crate::prompt_commands::Roots) {
    let Some((name, arguments)) = crate::prompt_commands::parse_request(input) else {
        state.show_command_panel(
            "Prompt commands",
            "Usage: `/prompt <name> [arguments]`. Use `/prompts` to list templates.",
        );
        return;
    };
    if name.is_empty() {
        state.show_command_panel(
            "Prompt commands",
            "Usage: `/prompt <name> [arguments]`. Use `/prompts` to list templates.",
        );
        return;
    }
    match crate::prompt_commands::load(roots, name, arguments) {
        Ok(prompt) => {
            state.input_buffer = prompt;
            state.cursor_position = state.input_buffer.chars().count();
            state.request_redraw();
        }
        Err(error) => state.show_command_panel("Prompt commands", error),
    }
}

/// Point the session at an isolated task worktree.
///
/// The worktree becomes both the sandbox boundary and the default task scope,
/// so shell, file, search, and Git execution all run inside it without
/// repeated one-command write grants. The original checkout keeps its own
/// branch and files (#1496). The descriptor id is recorded with the session so
/// resuming reattaches to this worktree instead of creating another.
fn activate_task_workspace(
    s: &mut AppState,
    descriptor: &crate::config::WorkspaceDescriptor,
    action: &str,
) {
    s.workspace_root = Some(descriptor.workspace_path.clone());
    s.task_working_directory = Some(descriptor.workspace_path.clone());
    let _ = crate::config::save_session_workspace(
        &s.active_session_id,
        &rustcode_session::SessionWorkspace {
            cwd: descriptor.workspace_path.clone(),
            additional_directories: Vec::new(),
            task_workspace_id: Some(descriptor.id.clone()),
        },
    );
    s.history.push(ChatMessage::new(
        "system",
        format!(
            "Task worktree {action}: {} on branch {} from base {}. Shell, file, and search tools now default to it; the source checkout is unchanged.",
            descriptor.workspace_path.display(),
            descriptor.branch,
            descriptor.base_sha
        ),
    ));
}

fn handle_workspace_command(s: &mut AppState, tokens: &[&str]) {
    let Some(action) = tokens.get(1).copied() else {
        s.show_command_panel("Workspace", "Usage: /workspace create <base_sha> [branch] [name] | status | archive | cleanup confirm [delete-branch]");
        return;
    };
    let Some(manager) = crate::config::workspace_manager() else {
        s.history.push(ChatMessage::new(
            "system",
            "Workspace persistence is unavailable.",
        ));
        return;
    };
    match action {
        "create" => {
            let Some(base_sha) = tokens.get(2).copied().filter(|value| !value.is_empty()) else {
                s.history.push(ChatMessage::new(
                    "system",
                    "Usage: /workspace create <base_sha> [branch] [name]",
                ));
                return;
            };
            let source = s.effective_workspace_root();
            let Some(source) = source else {
                s.history.push(ChatMessage::new(
                    "system",
                    "Cannot determine the source workspace.",
                ));
                return;
            };
            let name = tokens.get(4).copied().unwrap_or("main-task");
            let branch = tokens.get(3).map(|value| (*value).to_string());
            // A follow-up on the same branch attaches to the existing task
            // worktree instead of creating a duplicate (#1496).
            if let Some(branch) = branch.as_deref()
                && !branch.is_empty()
                && let Ok(Some(existing)) = manager.find_active_by_branch(&source, branch)
            {
                let descriptor = match manager.resume(&existing.id) {
                    Ok(descriptor) => descriptor,
                    Err(error) => {
                        s.history.push(ChatMessage::new(
                            "system",
                            format!("Unable to reattach to the task worktree: {error}"),
                        ));
                        return;
                    }
                };
                activate_task_workspace(s, &descriptor, "reused");
                return;
            }
            let mut request = crate::config::WorkspaceRequest::for_task(
                source,
                name,
                base_sha,
                format!("session:{}", s.active_session_id),
                s.active_session_id.clone(),
                "main",
            );
            request.branch = branch;
            match manager.create(&request) {
                Ok(descriptor) => {
                    activate_task_workspace(s, &descriptor, "created");
                }
                Err(error) => s.history.push(ChatMessage::new(
                    "system",
                    format!("Unable to create isolated workspace: {error}"),
                )),
            }
        }
        "status" => {
            let Some(path) = s.workspace_root.as_deref() else {
                s.show_command_panel("Workspace", "No isolated workspace is active.");
                return;
            };
            match manager.handoff_for_workspace_path(path) {
                Ok(Some(handoff)) => s.show_command_panel("Workspace", handoff),
                Ok(None) => s.show_command_panel(
                    "Workspace",
                    format!("No RustCode workspace descriptor owns {}.", path.display()),
                ),
                Err(error) => s.show_command_panel(
                    "Workspace",
                    format!("Unable to inspect workspace: {error}"),
                ),
            }
        }
        "archive" => {
            let Some(path) = s.workspace_root.as_deref() else {
                s.history.push(ChatMessage::new(
                    "system",
                    "No isolated workspace is active.",
                ));
                return;
            };
            match manager.find_by_workspace_path(path) {
                Ok(Some(descriptor)) => match manager.cleanup(
                    &descriptor.id,
                    rustcode_session::CleanupAction::Archive,
                    false,
                ) {
                    Ok(_) => s.history.push(ChatMessage::new(
                        "system",
                        "Workspace archived and retained for review.",
                    )),
                    Err(error) => s.history.push(ChatMessage::new(
                        "system",
                        format!("Unable to archive workspace: {error}"),
                    )),
                },
                Ok(None) => s.history.push(ChatMessage::new(
                    "system",
                    "No RustCode workspace descriptor owns the active path.",
                )),
                Err(error) => s.history.push(ChatMessage::new(
                    "system",
                    format!("Unable to find workspace: {error}"),
                )),
            }
        }
        "cleanup" => {
            handle_workspace_cleanup(s, &manager, tokens);
        }
        _ => s.history.push(ChatMessage::new(
            "system",
            "Unknown workspace action. Use create, status, archive, or cleanup.",
        )),
    }
}

fn handle_workspace_cleanup(
    s: &mut AppState,
    manager: &rustcode_session::WorkspaceManager,
    tokens: &[&str],
) {
    let Some(path) = s.workspace_root.as_deref() else {
        s.history.push(ChatMessage::new(
            "system",
            "No isolated workspace is active.",
        ));
        return;
    };
    let confirmed = tokens.get(2) == Some(&"confirm");
    let delete_branch = tokens.get(3) == Some(&"delete-branch");
    match manager.find_by_workspace_path(path) {
        Ok(Some(descriptor)) => match manager.cleanup(
            &descriptor.id,
            rustcode_session::CleanupAction::Remove { delete_branch },
            confirmed,
        ) {
            Ok(_) => {
                s.workspace_root = None;
                s.task_working_directory = None;
                if let Some(previous) = crate::config::load_session_workspace(&s.active_session_id)
                {
                    let _ = crate::config::save_session_workspace(
                        &s.active_session_id,
                        &rustcode_session::SessionWorkspace {
                            task_workspace_id: None,
                            ..previous
                        },
                    );
                }
                s.history.push(ChatMessage::new(
                    "system",
                    "Isolated workspace removed. Source checkout was not changed.",
                ));
            }
            Err(error) => s.history.push(ChatMessage::new(
                "system",
                format!("Workspace cleanup was not performed: {error}"),
            )),
        },
        Ok(None) => s.history.push(ChatMessage::new(
            "system",
            "No RustCode workspace descriptor owns the active path.",
        )),
        Err(error) => s.history.push(ChatMessage::new(
            "system",
            format!("Unable to find workspace: {error}"),
        )),
    }
}

#[cfg(all(test, unix))]
mod workspace_cleanup_tests {
    use super::handle_workspace_cleanup;
    use crate::app::AppState;
    use rustcode_session::{WorkspaceManager, WorkspaceRequest};
    use std::path::Path;

    fn git(cwd: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .expect("git command should start");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    #[tokio::test]
    async fn confirmed_workspace_cleanup_restores_source_for_run_command() {
        if !crate::tools::exec::sandbox::runtime_tests_available() {
            return;
        }

        let source = tempfile::tempdir().unwrap();
        git(source.path(), &["init", "-b", "main"]);
        git(
            source.path(),
            &["config", "user.email", "test@example.invalid"],
        );
        git(source.path(), &["config", "user.name", "Test"]);
        std::fs::write(source.path().join("README.md"), "base\n").unwrap();
        git(source.path(), &["add", "."]);
        git(source.path(), &["commit", "-m", "base"]);
        let base_sha = git(source.path(), &["rev-parse", "HEAD"]);
        let persistence = tempfile::tempdir().unwrap();
        let manager = WorkspaceManager::new(persistence.path());
        let mut request = WorkspaceRequest::for_task(
            source.path(),
            "cleanup-regression",
            base_sha,
            "rustcode",
            "cleanup-session",
            "cleanup-task",
        );
        request.branch = Some("rustcode/cleanup-regression".to_owned());
        let descriptor = manager.create(&request).unwrap();

        let mut state =
            AppState::new_with_workspace_session(source.path(), Some("cleanup-session"));
        state.workspace_root = Some(descriptor.workspace_path.clone());
        handle_workspace_cleanup(&mut state, &manager, &["/workspace", "cleanup", "confirm"]);

        assert_eq!(state.workspace_root, None);
        assert_eq!(
            state.effective_workspace_root().as_deref(),
            Some(source.path())
        );
        assert!(!descriptor.workspace_path.exists());

        let shared_state = std::sync::Arc::new(tokio::sync::Mutex::new(state));
        let (output, _, _) =
            crate::network::tool_exec::confirm_and_execute_for_call_with_assessment(
                &reqwest::Client::new(),
                &shared_state,
                &tokio_util::sync::CancellationToken::new(),
                "run_command",
                &serde_json::json!({"command": "pwd"}),
                "run_command",
                true,
                None,
                None,
                Some("cleanup-regression-call"),
                None,
            )
            .await;

        assert!(output.success, "{}", output.content);
        assert!(
            output
                .content
                .contains(&source.path().display().to_string()),
            "command did not use the source after cleanup: {}",
            output.content
        );
    }

    /// `/workspace create` binds the worktree as both the sandbox root and the
    /// default task scope, and a follow-up on the same branch reuses it.
    #[tokio::test]
    async fn workspace_create_binds_task_scope_and_reuses_worktree_on_follow_up() {
        if !crate::tools::exec::sandbox::runtime_tests_available() {
            return;
        }

        let source = tempfile::tempdir().unwrap();
        git(source.path(), &["init", "-b", "main"]);
        git(
            source.path(),
            &["config", "user.email", "test@example.invalid"],
        );
        git(source.path(), &["config", "user.name", "Test"]);
        std::fs::write(source.path().join("README.md"), "base\n").unwrap();
        git(source.path(), &["add", "."]);
        git(source.path(), &["commit", "-m", "base"]);
        let base_sha = git(source.path(), &["rev-parse", "HEAD"]);
        let persistence = tempfile::tempdir().unwrap();
        let manager = WorkspaceManager::new(persistence.path());
        let source_canonical = source.path().canonicalize().unwrap();

        let mut state = AppState::new_with_workspace_session(&source_canonical, None);
        state.active_session_id = "task-flow-session".to_owned();

        let create = [
            "/workspace",
            "create",
            base_sha.as_str(),
            "feature/shared-branch",
            "shared-task",
        ];
        super::handle_workspace_command(&mut state, &create);

        let first_path = state
            .workspace_root
            .clone()
            .expect("create should activate a worktree");
        // The worktree is the default navigation scope for file/search/shell.
        assert_eq!(state.task_working_directory, Some(first_path.clone()));
        assert_ne!(first_path, source_canonical);
        assert!(
            state
                .history
                .iter()
                .any(|message| message.content.contains("Task worktree created"))
        );
        // The source checkout keeps its own branch and files.
        assert_eq!(
            git(source.path(), &["rev-parse", "--abbrev-ref", "HEAD"]),
            "main"
        );
        assert_eq!(
            std::fs::read_to_string(source.path().join("README.md")).unwrap(),
            "base\n"
        );

        // A follow-up naming the same branch attaches instead of duplicating.
        let mut follow_up = state;
        follow_up.workspace_root = Some(source_canonical.clone());
        follow_up.task_working_directory = Some(source_canonical.clone());
        super::handle_workspace_command(&mut follow_up, &create);

        assert_eq!(
            follow_up.workspace_root,
            Some(first_path.clone()),
            "follow-up should reuse the existing worktree"
        );
        assert_eq!(follow_up.task_working_directory, Some(first_path.clone()));
        assert!(
            follow_up
                .history
                .iter()
                .any(|message| message.content.contains("Task worktree reused")),
            "history: {:?}",
            follow_up
                .history
                .iter()
                .map(|message| message.content.as_str())
                .collect::<Vec<_>>()
        );
        // Exactly one worktree checkout exists for the branch.
        let worktrees = std::process::Command::new("git")
            .args(["worktree", "list", "--porcelain"])
            .current_dir(source.path())
            .output()
            .expect("git worktree list");
        let listed = String::from_utf8_lossy(&worktrees.stdout);
        assert_eq!(
            listed
                .lines()
                .filter(|line| line.contains("feature/shared-branch"))
                .count(),
            1,
            "follow-up created a duplicate worktree: {listed}"
        );
    }
}
