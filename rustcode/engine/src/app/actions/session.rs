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

    rustcode_core::input::get_at_word_query(input_buffer, cursor_position)
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
        rustcode_core::input::get_at_word_query(&s.input_buffer, s.cursor_position)
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
        s.show_command_panel(
            "Memory",
            format!("🦀 Current Rustcode RAM usage: {} MB", mem_mb),
        );
    } else {
        s.show_command_panel("Memory", "Could not find current process.");
    }
}

/// Outcome of one `ctrl+o` press against the collapsed tool bodies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpandOutcome {
    /// The focused entry now renders its body inline.
    Expanded(usize),
    /// The focused entry is collapsed again.
    Collapsed(usize),
    /// Nothing in the transcript is collapsible, so the press was a no-op.
    NothingToExpand,
}

/// Toggle the collapsed tool body `candidates` points at.
///
/// `candidates` are the message indices the frontend rendered with a collapsed
/// body, newest last. The last one wins, so `ctrl+o` always acts on the most
/// recent collapsed entry, and a second press collapses exactly what the first
/// expanded (#1541).
pub fn toggle_expanded_thought(s: &mut AppState, candidates: &[usize]) -> ExpandOutcome {
    let (outcome, notice) = toggle_expanded_bodies(
        &mut s.expanded_thoughts,
        &mut s.expanded_thought_focus,
        candidates,
    );
    s.set_transient_notice(notice);
    outcome
}

/// The transition itself, over just the state the collapsed bodies live in.
///
/// Split out of [`toggle_expanded_thought`] so the frontend seam can drive the
/// same press over the render-visible expand state a frontend already holds,
/// instead of needing an `AppState` to own it (frontend seam #1431). `focus` is
/// the last expanded index the session keeps in step with the set. Returns what
/// the press did plus the feedback text to surface.
pub(crate) fn toggle_expanded_bodies(
    expanded: &mut std::collections::HashSet<usize>,
    focus: &mut Option<usize>,
    candidates: &[usize],
) -> (ExpandOutcome, &'static str) {
    let Some(&target) = candidates.last() else {
        return (ExpandOutcome::NothingToExpand, "Nothing to expand");
    };
    if expanded.remove(&target) {
        *focus = None;
        (ExpandOutcome::Collapsed(target), "Collapsed tool output")
    } else {
        expanded.insert(target);
        *focus = Some(target);
        (ExpandOutcome::Expanded(target), "Expanded tool output")
    }
}

pub fn toggle_auto_confirm(s: &mut AppState) {
    s.auto_confirm = !s.auto_confirm;
    let status = if s.auto_confirm {
        "enabled"
    } else {
        "disabled"
    };
    s.set_transient_notice(format!("YOLO mode {status}"));
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
    s.clear_session_steering();
    s.invalidate_orchestrator();
    s.session_title_tool_available = false;
    s.background_wakeup_ids.clear();
    s.background_turn_context = None;
    s.last_turn_had_model_final_response = false;
    s.last_summary_history_len = None;
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
    s.expanded_thought_focus = None;
    s.enter_idle();
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
    s.pending_approval_details = None;
    s.pending_approval_batch_id = None;
    s.tool_confirmation_response = None;
    s.clear_question_chain();
    s.question_response = None;
    s.running_tools.clear();
    s.clear_live_tool_calls();
    s.stream_tracker = None;
    s.settings_picker = None;
    s.command_panel = None;
    s.show_context_modal = false;
    s.show_status_modal = false;
    s.show_stats_modal = false;
    s.show_session_modal = false;
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

/// Fill in the active profile's provider context window when the provider can
/// report one (currently: Ollama's /api/show and llama.cpp's /props). The
/// configured model window remains intact so a mismatch can be diagnosed, but
/// all request budgeting uses the safe effective minimum.
pub fn spawn_context_window_detection(state: Arc<Mutex<AppState>>, client: reqwest::Client) {
    tokio::spawn(async move {
        let (name, url, model, engine) = {
            let s = state.lock().await;
            let Some(profile) = s.active_model_profile() else {
                return;
            };
            if profile.provider_context_window.is_some() {
                return;
            }
            (
                profile.name.clone(),
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
        if let Some(profile) = s
            .config
            .models
            .iter_mut()
            .find(|profile| profile.matches_request(&url, &model))
            && profile.provider_context_window.is_none()
        {
            let mismatch = profile
                .context_window
                .is_some_and(|configured| ctx < configured);
            profile.provider_context_window = Some(ctx);
            crate::config::save_entire_config(&s.config);
            s.history.push(ChatMessage::new(
                "system",
                if mismatch {
                    format!(
                        "Detected provider context window for '{}': {} tokens (clamped below the configured model window)",
                        name, ctx
                    )
                } else {
                    format!("Detected provider context window for '{}': {} tokens", name, ctx)
                },
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
/// Defaults to current-workspace only (parent/child match); legacy sessions
/// with no workspace recorded are included and should be labeled
/// "no workspace recorded". Explicit `--resume <id>` bypasses this filter.
pub fn build_session_list_with_truncation(s: &AppState) -> (Vec<crate::config::SessionMeta>, bool) {
    build_session_list_scoped_with_truncation(s, false)
}

/// Scoped session list with an explicit `--all` escape hatch. When
/// `show_all` is true, sessions from every workspace are returned with
/// their workspace per entry.
///
/// The scope rule itself lives in the store (`SessionScope`), which is what the
/// TUI picker, ACP `session/list` and the desktop shell all list through; this
/// only chooses the variant. The desktop is the documented exception: it lists
/// through here too, but renders the result as a cross-project browser and asks
/// for the resume directory in the UI instead of filtering to one project.
pub fn build_session_list_scoped_with_truncation(
    s: &AppState,
    show_all: bool,
) -> (Vec<crate::config::SessionMeta>, bool) {
    const MAX_SESSIONS: usize = 50;
    let scope = session_list_scope(show_all);
    let (mut list, mut truncated) = crate::config::list_sessions_in_scope(MAX_SESSIONS, &scope);
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

/// The scope the session picker lists under. An undeterminable working
/// directory falls back to `All` rather than hiding every session.
pub fn session_list_scope(show_all: bool) -> crate::config::SessionScope {
    if show_all {
        return crate::config::SessionScope::All;
    }
    crate::config::SessionScope::for_current_dir()
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

/// Adopt the recorded session workspace on resume. Prefers the session's
/// isolated task worktree when it can be reattached (#1496), otherwise the
/// recorded cwd when it still exists. Missing records keep the current
/// directory. Reports whether the working directory changed.
pub fn adopt_session_workspace(s: &mut AppState, meta: &crate::config::SessionMeta) -> bool {
    let session_id = crate::config::session_id_from_path(&meta.path).unwrap_or_default();
    // Read the whole record rather than rebuilding one from the meta's embedded
    // cwd: the record is the only place `task_workspace_id` lives, and a resume
    // already opens the whole transcript, so one small extra read here costs
    // nothing and keeps the isolated-worktree reattach above working (#1533).
    let workspace = (!session_id.is_empty())
        .then(|| crate::config::load_session_workspace(&session_id))
        .flatten();
    let Some(workspace) = workspace else {
        return false;
    };
    // Reattach an isolated task worktree first so resume reuses its
    // writable roots instead of starting over in the source checkout.
    if let Some(descriptor_id) = workspace.task_workspace_id.as_deref()
        && let Some(manager) = crate::config::workspace_manager()
        && let Ok(descriptor) = manager.resume(descriptor_id)
    {
        s.workspace_root = Some(descriptor.workspace_path.clone());
        s.task_working_directory = Some(descriptor.workspace_path);
        return true;
    }
    // Only adopt existing directories; a deleted project keeps the current cwd.
    if workspace.cwd.is_dir() {
        let current = std::env::current_dir().ok();
        let changed = current.as_deref() != Some(workspace.cwd.as_path());
        s.workspace_root = Some(workspace.cwd.clone());
        s.task_working_directory = Some(workspace.cwd);
        return changed;
    }
    false
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
    // Adopt the recorded workspace when resuming across directories, like
    // the desktop "Choose project folder" flow. An explicit `--resume <id>`
    // overrides workspace scoping and lands in the session's cwd when it
    // still exists; otherwise the current directory is kept.
    let workspace_switched = adopt_session_workspace(s, meta);
    restore_segment_checkpoint(s);
    s.image_analysis_cache = crate::config::load_session_image_cache(&s.active_session_id);
    s.history_display_start = 0;
    if workspace_switched && let Some(cwd) = s.task_working_directory.as_deref() {
        s.history.push(ChatMessage::new(
            "system",
            format!("Switched to the session's workspace {}", cwd.display()),
        ));
    }
    s.history.push(ChatMessage::new(
        "system",
        format!("Resumed session \"{}\"", meta.title),
    ));
    crate::config::save_session_history(&s.active_session_id, &s.history);
    true
}

/// Restore a persisted productive segment after a restart. The restored
/// context remains idle until the user explicitly continues it.
/// Stale or foreign checkpoints are ignored.
pub(crate) fn restore_segment_checkpoint(s: &mut AppState) {
    let Some(checkpoint) = crate::config::load_segment_checkpoint(&s.active_session_id) else {
        return;
    };
    if !checkpoint.continuation_pending && !checkpoint.background_pending {
        return;
    }
    let mut context = crate::network::TurnContext::with_budgets(
        s.config.max_tool_rounds,
        s.config.max_total_tool_rounds,
    );
    if !context.restore_segment(&checkpoint, &s.active_session_id) {
        return;
    }
    s.background_turn_context = Some(Box::new(context));
}

/// Queue an explicitly requested continuation of a restored productive
/// segment. Opening or resuming a session must not call this implicitly.
pub fn queue_restored_segment(s: &mut AppState) -> bool {
    if s.background_turn_context.is_none() {
        restore_segment_checkpoint(s);
    }
    if s.background_turn_context.is_none() {
        return false;
    }

    if !s
        .pending_queue
        .iter()
        .any(|prompt| prompt == "__task_wakeup__:productive_segment")
    {
        s.pending_queue
            .insert(0, "__task_wakeup__:productive_segment".to_string());
    }
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
    copy_last_reply_with(s, crate::clipboard::copy_to_clipboard);
}

fn copy_last_reply_with(
    s: &mut AppState,
    copy: impl FnOnce(&str) -> crate::clipboard::ClipboardCopyStatus,
) {
    let last_reply = s
        .history
        .iter()
        .rev()
        .find(|m| m.role == "assistant")
        .map(|m| m.content.clone());

    if let Some(content) = last_reply {
        let clean_text = extract_code_blocks_or_content(&content);
        match copy(&clean_text) {
            crate::clipboard::ClipboardCopyStatus::Confirmed => {
                s.last_copy_text = Some((clean_text.clone(), std::time::Instant::now()));
                s.set_transient_notice("Copied code/reply to clipboard");
            }
            crate::clipboard::ClipboardCopyStatus::Requested => {
                s.set_transient_notice("Copy sent to terminal; paste to verify");
            }
            crate::clipboard::ClipboardCopyStatus::Failed => {
                s.set_transient_notice("Copy failed; try again");
            }
        }
    } else {
        s.set_transient_notice("No assistant reply found to copy");
    }
}

#[cfg(test)]
mod copy_tests {
    use super::{AppState, ChatMessage, copy_last_reply_with};
    use crate::clipboard::ClipboardCopyStatus;

    #[test]
    fn copy_command_uses_footer_notice_without_adding_history() {
        for (result, expected) in [
            (
                ClipboardCopyStatus::Confirmed,
                "Copied code/reply to clipboard",
            ),
            (
                ClipboardCopyStatus::Requested,
                "Copy sent to terminal; paste to verify",
            ),
            (ClipboardCopyStatus::Failed, "Copy failed; try again"),
        ] {
            let mut state = AppState::new();
            state
                .history
                .push(ChatMessage::new("assistant", "response"));
            copy_last_reply_with(&mut state, |text| {
                assert_eq!(text, "response");
                result
            });
            assert_eq!(state.active_transient_notice(), Some(expected));
            assert_eq!(state.history.len(), 1);
            assert_eq!(
                state.last_copy_text.is_some(),
                result == ClipboardCopyStatus::Confirmed
            );
        }
    }

    #[test]
    fn copy_command_without_reply_uses_footer_notice() {
        let mut state = AppState::new();
        copy_last_reply_with(&mut state, |_| panic!("no clipboard request expected"));
        assert_eq!(
            state.active_transient_notice(),
            Some("No assistant reply found to copy")
        );
        assert!(state.history.is_empty());
    }
}

#[cfg(test)]
mod workspace_tests {
    use super::{AppState, adopt_session_workspace};

    /// Store a real session with the given workspace record and return the meta
    /// the picker would have handed to a resume. Adoption reads the session's
    /// record rather than the meta's embedded cwd, so the tests go through the
    /// store instead of hand-building a `SessionMeta` (#1533).
    fn stored_meta(
        session_id: &str,
        workspace: Option<&std::path::Path>,
    ) -> crate::config::SessionMeta {
        let store = rustcode_session::SessionStore::new(
            crate::config::get_config_dir().expect("config dir"),
        );
        store.save_session_history(
            session_id,
            &vec![
                crate::app::ChatMessage::new("user", "prompt"),
                crate::app::ChatMessage::new("assistant", "reply"),
            ],
        );
        store
            .save_session_workspace(
                session_id,
                &crate::config::SessionWorkspace {
                    cwd: workspace
                        .map(std::path::Path::to_path_buf)
                        .unwrap_or_default(),
                    additional_directories: Vec::new(),
                    task_workspace_id: None,
                },
            )
            .expect("workspace record");
        rustcode_session::flush_history();
        store
            .session_meta_by_id(session_id)
            .expect("stored session meta")
    }

    #[test]
    fn adopting_an_existing_workspace_sets_task_directories() {
        let dir = tempfile::tempdir().expect("temp workspace");
        let mut state = AppState::new();
        let meta = stored_meta("adopt-existing", Some(dir.path()));
        assert!(adopt_session_workspace(&mut state, &meta));
        assert_eq!(state.task_working_directory, Some(dir.path().to_path_buf()));
        assert_eq!(state.workspace_root, Some(dir.path().to_path_buf()));
    }

    #[test]
    fn adopting_a_missing_workspace_keeps_current_directories() {
        let mut state = AppState::new();
        let before_task = state.task_working_directory.clone();
        let before_root = state.workspace_root.clone();
        let meta = stored_meta(
            "adopt-missing",
            Some(std::path::Path::new("/definitely/not/a/rustcode/workspace")),
        );
        assert!(!adopt_session_workspace(&mut state, &meta));
        assert_eq!(state.task_working_directory, before_task);
        assert_eq!(state.workspace_root, before_root);
    }

    #[test]
    fn legacy_sessions_without_a_workspace_record_keep_current_directories() {
        let mut state = AppState::new();
        // A session written before workspaces existed has no record at all, so
        // the meta carries no cwd and a resume stays where it is.
        let store = rustcode_session::SessionStore::new(
            crate::config::get_config_dir().expect("config dir"),
        );
        store.save_session_history(
            "adopt-legacy",
            &vec![
                crate::app::ChatMessage::new("user", "prompt"),
                crate::app::ChatMessage::new("assistant", "reply"),
            ],
        );
        rustcode_session::flush_history();
        let meta = store
            .session_meta_by_id("adopt-legacy")
            .expect("stored session meta");
        assert!(meta.workspace_cwd.is_none());
        assert!(!adopt_session_workspace(&mut state, &meta));
        assert_eq!(state.task_working_directory, None);
        assert_eq!(state.workspace_root, None);
    }
}
