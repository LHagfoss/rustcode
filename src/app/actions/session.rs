use super::*;
pub(super) fn history_matches_snapshot(
    live_session_id: &str,
    live_history: &[ChatMessage],
    captured_session_id: &str,
    captured_history: &[ChatMessage],
) -> bool {
    live_session_id == captured_session_id && live_history.starts_with(captured_history)
}

pub(super) fn try_merge_compacted_history(
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

pub(super) fn report_stale_compaction(
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
    crate::app::suggestion::filtered_commands(input_buffer).len()
}

pub fn get_completion_len(input_buffer: &str, cursor_position: usize) -> usize {
    if crate::app::suggestion::command_token(input_buffer).is_some() {
        return get_filtered_cmds_len(input_buffer);
    }

    crate::app::get_at_word_query(input_buffer, cursor_position)
        .map(|(_, query)| crate::app::list_project_file_paths(&query).len())
        .unwrap_or(0)
}

pub fn apply_autocomplete(s: &mut AppState) {
    s.dismissed_completion = None;
    if let Some(command) = crate::app::suggestion::command_token(&s.input_buffer) {
        let filtered_cmds = crate::app::suggestion::filtered_commands(&s.input_buffer);
        let idx = s
            .active_suggestion_index
            .unwrap_or(0)
            .min(filtered_cmds.len().saturating_sub(1));
        if !filtered_cmds.is_empty() {
            let replacement = filtered_cmds[idx].name;
            let command_end = command.len();
            s.input_buffer.replace_range(0..command_end, replacement);
            s.cursor_position = replacement.len();
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
    s.request_redraw();
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

pub fn toggle_auto_confirm(s: &mut AppState) {
    s.auto_confirm = !s.auto_confirm;
    let status = if s.auto_confirm {
        "enabled"
    } else {
        "disabled"
    };
    s.set_notice(format!("YOLO mode {status}"));
}

pub fn start_new_session(s: &mut AppState) {
    if crate::config::session_has_content(&s.history) {
        crate::config::save_session_history(&s.active_session_id, &s.history);
    }
    reset_active_session_state(s);
    s.tip_index = crate::app::random_tip_index();
    s.history_display_start = 0;
    s.history.clear();

    // Switch to a new active session ID
    s.active_session_id = crate::config::create_new_session(&mut s.config);
    crate::config::set_active_session_id(&s.active_session_id);
    s.history
        .push(ChatMessage::new("system", "✨ New chat started"));
    crate::config::save_session_history(&s.active_session_id, &s.history);
}

/// Drop state owned by the active conversation before another session is
/// attached.  Configuration and cross-session input history intentionally stay
/// untouched; the fields below are either model/session state or projections
/// of the currently displayed transcript.
pub(crate) fn reset_active_session_state(s: &mut AppState) {
    s.subagent_supervisor.shutdown();
    s.subagent_supervisor =
        crate::app::SubagentSupervisor::new(s.config.subagent_concurrency_limit);
    s.pending_queue.clear();
    s.background_wakeup_ids.clear();
    s.background_turn_context = None;
    s.image_analysis_cache.clear();
    s.clear_current_response();
    s.current_thought_time_ms = 0;
    s.current_thought_tokens = 0;
    s.current_thought_started_at = None;
    s.current_token_usage = None;
    s.response_time = None;
    s.generation_start_time = None;
    s.history_index = None;
    s.temp_input.clear();
    s.expanded_thoughts.clear();
    s.status = AppStatus::Idle;
    s.subagents.clear();
    s.selected_subagent_id = None;
    s.show_history_picker = false;
    s.show_model_picker = false;
    s.show_theme_picker = false;
    s.show_command_picker = false;
    s.show_subagent_picker = false;
    s.subagent_picker_index = 0;
    s.delegation_armed = false;
    s.delegation_active = false;
    s.next_subagent_id = 1;
    s.todos.clear();
    s.read_file_mtimes.clear();
    s.recent_read_calls.clear();
    s.recent_read_outputs.clear();
    s.continuous_mode = false;
    s.pending_tool_confirmation = None;
    s.tool_confirmation_response = None;
    s.pending_question = None;
    s.question_response = None;
    s.running_tools.clear();
    s.clear_live_tool_calls();
    s.stream_tracker = None;
    s.show_context_modal = false;
    s.modal_scroll_row = 0;
    s.tool_confirmation_selected = 0;
    s.history_picker_index = 0;
    s.history_picker_sessions.clear();
    s.history_picker_truncated = false;
    s.pending_delete_session_idx = None;
    s.input_buffer.clear();
    s.cursor_position = 0;
    s.active_suggestion_index = None;
    s.dismissed_completion = None;
    s.clear_selection();
    s.selected_text = None;
    s.scroll_row = 0;
    s.is_scroll_locked_to_bottom = true;
    s.last_max_scroll = 0;
    s.conversation_content_height = 0;
    s.viewport_height = 0;
    s.chat_area = None;
    s.input_text_area = None;
    s.scroll_to_bottom_btn = None;
    s.context_snapshot = None;
    s.last_copy_text = None;
    s.invalidate_session_title_cache();
    s.request_clear_screen();
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
pub fn build_session_list_with_truncation(s: &AppState) -> (Vec<crate::config::SessionMeta>, bool) {
    const MAX_SESSIONS: usize = 50;
    let (mut list, mut truncated) = crate::config::list_sessions_limited(MAX_SESSIONS);
    if !crate::config::session_has_content(&s.history)
        && let Some(live) = crate::config::live_session_meta()
        && !list.iter().any(|m| m.path == live.path)
    {
        list.insert(0, live);
        if list.len() > MAX_SESSIONS {
            list.truncate(MAX_SESSIONS);
            truncated = true;
        }
    }
    (list, truncated)
}

pub fn build_session_list(s: &AppState) -> Vec<crate::config::SessionMeta> {
    let (list, _) = build_session_list_with_truncation(s);
    list
}

/// Returns whether the session list was truncated at MAX_SESSIONS.
#[allow(dead_code)]
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

pub fn load_session_into(s: &mut AppState, meta: &crate::config::SessionMeta) -> bool {
    let mut loaded = crate::config::load_session_file(&meta.path);
    if loaded.is_empty() {
        s.history.push(ChatMessage::new(
            "system",
            format!("Could not load session '{}'", meta.title),
        ));
        return false;
    }

    // Strip legacy "Resumed session " system messages from loaded transcript
    loaded.retain(|m| !(m.role == "system" && m.content.starts_with("Resumed session ")));

    // Save current active session history if it has content
    if crate::config::session_has_content(&s.history) {
        crate::config::save_session_history(&s.active_session_id, &s.history);
    }

    // Extract session ID from the loaded path
    if let Some(session_id_str) = crate::config::session_id_from_path(&meta.path) {
        // Flush the outgoing session's queued history before retargeting.
        crate::config::flush_history();
        s.active_session_id = session_id_str;
        s.config.last_active_session_id = Some(s.active_session_id.clone());
        crate::config::save_entire_config(&s.config);
        crate::config::set_active_session_id(&s.active_session_id);
    }

    s.history.replace(loaded);
    reset_active_session_state(s);
    s.image_analysis_cache = crate::config::load_session_image_cache(&s.active_session_id);
    s.history_display_start = 0;
    s.history.push(ChatMessage::new(
        "system",
        format!("Resumed session \"{}\"", meta.title),
    ));
    crate::config::save_session_history(&s.active_session_id, &s.history);
    true
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
            s.last_copy_text = Some((clean_text.clone(), std::time::Instant::now()));
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
