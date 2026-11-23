use super::*;

pub(super) async fn handle_key(
    key: KeyEvent,
    ctx: InputContext<'_>,
) -> Result<InputFlow, Box<dyn Error>> {
    let InputContext {
        terminal_runtime,
        app_state,
        client,
        current_cancel_token,
        needs_redraw,
        frame_requester,
        transcript_state,
        app_event_sender,
        agent_ui_event_sender,
        composer,
        demo_state,
        ..
    } = ctx;
    *needs_redraw = true;
    app_state.lock().await.mark_user_activity();
    let is_ctrl = key.modifiers.contains(event::KeyModifiers::CONTROL);
    let is_cmd = key.modifiers.contains(event::KeyModifiers::SUPER);

    if is_ctrl && key.code == KeyCode::Char(' ') && !app_state.lock().await.modal_open() {
        let snapshot = {
            let state = app_state.lock().await;
            ui::render_snapshot::render_snapshot(&rustcode::controller::render_state(&state))
        };
        transcript_state
            .selection
            .begin_keyboard_with_snapshot(snapshot, transcript_state.scroll_rows());
        return Ok(InputFlow::ContinueIteration);
    }
    if transcript_state.panel_selection_area.is_none()
        && transcript_state.selection.is_keyboard_mode()
    {
        if is_keyboard_range_key(key) {
            transcript_state.selection.move_keyboard(key.code);
            return Ok(InputFlow::ContinueIteration);
        }
    }

    // Ctrl+C runs before selection handlers so the chord can copy
    // and retain its double-press exit behavior. Cmd+C is copy-only.
    if is_copy_or_exit_chord(key) {
        return Ok(handle_copy_or_exit_chord(&app_state, transcript_state).await);
    }
    if is_cmd_copy_chord(key) {
        return Ok(handle_cmd_copy_chord(&app_state, transcript_state).await);
    }

    if selection_owns_key(transcript_state, key) {
        let selection = if transcript_state.panel_selection_area.is_some() {
            &mut transcript_state.panel_selection
        } else {
            &mut transcript_state.selection
        };
        selection.clear();
        return Ok(InputFlow::ContinueIteration);
    }

    // Composer selection owns Esc (#1493, #1566). The copy chord is
    // already handled above so it can also arm the exit.
    if key.code == KeyCode::Esc && app_state.lock().await.has_composer_selection() {
        app_state.lock().await.clear_composer_selection();
        return Ok(InputFlow::ContinueIteration);
    }

    let transcript_navigation = is_transcript_navigation(key);
    clear_selection_for_composer_key(transcript_state, key);

    {
        let mut s = app_state.lock().await;
        s.clear_ctrl_c_exit_arming();
    }

    if (is_ctrl || is_cmd) && (key.code == KeyCode::Char('k') || key.code == KeyCode::Char('K')) {
        let mut s = app_state.lock().await;
        s.request_clear_screen();
        *needs_redraw = true;
        return Ok(InputFlow::ContinueIteration);
    }
    if is_ctrl && (key.code == KeyCode::Char('l') || key.code == KeyCode::Char('L')) {
        let mut s = app_state.lock().await;
        s.request_clear_screen();
        *needs_redraw = true;
        return Ok(InputFlow::ContinueIteration);
    }

    {
        let selected = {
            let state = app_state.lock().await;
            state
                .show_update_prompt
                .then_some(state.update_prompt_index)
        };
        if let Some(selected) = selected {
            match key.code {
                KeyCode::Up => {
                    let mut state = app_state.lock().await;
                    state.update_prompt_index = state.update_prompt_index.saturating_sub(1);
                }
                KeyCode::Down => {
                    let mut state = app_state.lock().await;
                    state.update_prompt_index = (state.update_prompt_index + 1).min(2);
                }
                KeyCode::Enter => {
                    let decision = match selected {
                        0 => UpdateDecision::UpdateNow,
                        1 => UpdateDecision::Skip,
                        _ => UpdateDecision::SkipUntilNextVersion,
                    };
                    let _ = app_event_sender.send(AppEvent::UpdateDecision(decision));
                }
                KeyCode::Esc => {
                    let _ = app_event_sender.send(AppEvent::UpdateDecision(UpdateDecision::Skip));
                }
                _ => {}
            }
            return Ok(InputFlow::ContinueIteration);
        }
    }

    {
        let selected = {
            let s = app_state.lock().await;
            (s.status == AppStatus::AwaitingToolConfirmation && !s.user_overlay_open()).then(|| {
                let prefix = s
                    .pending_tool_confirmation
                    .as_ref()
                    .filter(|items| {
                        items.len() == 1 && items[0].rememberable_prefix.is_some()
                            || items.len() == 1 && items[0].forbidden_prefix.is_some()
                    })
                    .and_then(|items| items[0].rememberable_prefix.clone());
                let forbidden_prefix = s
                    .pending_tool_confirmation
                    .as_ref()
                    .filter(|items| items.len() == 1)
                    .and_then(|items| items[0].forbidden_prefix.clone());
                (s.tool_confirmation_selected, prefix, forbidden_prefix)
            })
        };
        if let Some((selected, prefix, forbidden_prefix)) = selected {
            if let Some(event) = ui::approval_event_for_key(
                key,
                selected,
                prefix.as_deref(),
                forbidden_prefix.as_deref(),
            ) {
                let _ = app_event_sender.send(event);
            } else {
                if is_shift_tab(key) {
                    let mut s = app_state.lock().await;
                    s.overlays().toggle_auto_confirm();
                    s.record_auto_confirm();
                } else {
                    match key.code {
                        KeyCode::Tab => {
                            let mut s = app_state.lock().await;
                            s.overlays().toggle_auto_confirm();
                            s.record_auto_confirm();
                        }
                        KeyCode::Up => {
                            let mut s = app_state.lock().await;
                            s.overlays().move_approval_selection(-1);
                        }
                        KeyCode::Down => {
                            let mut s = app_state.lock().await;
                            s.overlays().move_approval_selection(1);
                        }
                        _ => {}
                    }
                }
            }
            return Ok(InputFlow::ContinueIteration);
        }
    }

    {
        // A device asks to share this session. Plain keys answer
        // it so a stray keystroke cannot leak into the composer
        // while the terminal decides; chords keep working.
        let share_open = app_state.lock().await.pending_share_prompt.is_some();
        if share_open {
            if key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER | KeyModifiers::ALT)
            {
                // Fall through to normal handling below.
            } else {
                match key.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') => {
                        app_state.lock().await.share_decision = Some(true);
                    }
                    KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                        app_state.lock().await.share_decision = Some(false);
                    }
                    _ => {}
                }
                return Ok(InputFlow::ContinueIteration);
            }
        }
    }

    {
        let s = app_state.lock().await;
        if s.status == AppStatus::AwaitingQuestion && !s.user_overlay_open() {
            let typing = s
                .pending_question
                .as_ref()
                .map(|q| q.custom_input.is_some())
                .unwrap_or(false);
            drop(s);

            if typing {
                let action = match key.code {
                    KeyCode::Tab => {
                        app_state.lock().await.focus_question(1);
                        QuestionEditAction::Handled
                    }
                    KeyCode::BackTab => {
                        app_state.lock().await.focus_question(-1);
                        QuestionEditAction::Handled
                    }
                    KeyCode::Esc => {
                        let mut s = app_state.lock().await;
                        if let Some(q) = s.pending_question.as_mut() {
                            q.custom_input = None;
                            q.custom_cursor = 0;
                            q.custom_draft.clear();
                        }
                        QuestionEditAction::Handled
                    }
                    _ => {
                        let mut s = app_state.lock().await;
                        s.pending_question
                            .as_mut()
                            .map_or(QuestionEditAction::Handled, |q| {
                                handle_question_custom_key(q, key)
                            })
                    }
                };
                match action {
                    QuestionEditAction::Submit => {
                        let answer_event = {
                            let s = app_state.lock().await;
                            s.pending_question
                                .as_ref()
                                .map(ui::question_custom_answer_event)
                        };
                        if let Some(answer_event) = answer_event {
                            let _ = app_event_sender.send(answer_event);
                        }
                    }
                    QuestionEditAction::Paste => {
                        if let Some(text) = rustcode::clipboard::read_text_from_clipboard() {
                            let mut s = app_state.lock().await;
                            if let Some(q) = s.pending_question.as_mut() {
                                insert_question_answer_text(q, &text);
                            }
                        }
                    }
                    QuestionEditAction::Handled => {}
                }
                *needs_redraw = true;
                return Ok(InputFlow::ContinueIteration);
            }

            match key.code {
                KeyCode::Up => {
                    let mut s = app_state.lock().await;
                    if let Some(q) = s.pending_question.as_mut() {
                        q.selected = q.selected.saturating_sub(1);
                    }
                }
                KeyCode::Down => {
                    let mut s = app_state.lock().await;
                    if let Some(q) = s.pending_question.as_mut() {
                        let last = q.options.len();
                        q.selected = (q.selected + 1).min(last);
                        if q.selected == last {
                            q.activate_custom_input();
                        }
                    }
                }
                KeyCode::Tab => {
                    let mut s = app_state.lock().await;
                    s.focus_question(1);
                }
                KeyCode::BackTab => {
                    let mut s = app_state.lock().await;
                    s.focus_question(-1);
                }
                KeyCode::Char(' ') => {
                    let mut s = app_state.lock().await;
                    if let Some(q) = s.pending_question.as_mut() {
                        if q.selected == q.options.len() {
                            q.activate_custom_input();
                        } else if q.is_multi_select
                            && let Some(c) = q.chosen.get_mut(q.selected)
                        {
                            *c = !*c;
                        }
                    }
                }
                KeyCode::Char(d @ '1'..='9') => {
                    let idx = (d as usize) - ('1' as usize);
                    let mut s = app_state.lock().await;
                    if let Some(q) = s.pending_question.as_mut()
                        && idx < q.options.len()
                    {
                        q.selected = idx;
                        if q.is_multi_select {
                            if let Some(c) = q.chosen.get_mut(idx) {
                                *c = !*c;
                            }
                        } else {
                            let answer_event = ui::question_answer_event(q);
                            if let Some(answer_event) = answer_event {
                                let _ = app_event_sender.send(answer_event);
                            }
                        }
                    }
                }
                KeyCode::Char(c) => {
                    let mut s = app_state.lock().await;
                    if let Some(q) = s.pending_question.as_mut() {
                        if q.selected == q.options.len() {
                            q.activate_custom_input();
                            q.insert_char(c);
                        }
                    }
                }
                KeyCode::Enter => {
                    let mut s = app_state.lock().await;
                    let is_custom_slot = s
                        .pending_question
                        .as_ref()
                        .map(|q| q.selected == q.options.len())
                        .unwrap_or(false);
                    if is_custom_slot {
                        if let Some(q) = s.pending_question.as_mut() {
                            q.activate_custom_input();
                        }
                    } else if let Some(q) = s.pending_question.as_ref()
                        && let Some(answer_event) = ui::question_answer_event(q)
                    {
                        let _ = app_event_sender.send(answer_event);
                    }
                }
                KeyCode::Esc => {
                    let _ = app_event_sender.send(ui::question_cancel_event());
                }
                _ => {}
            }
            *needs_redraw = true;
            return Ok(InputFlow::ContinueIteration);
        }
    }

    {
        let s = app_state.lock().await;
        if s.settings_picker == Some(rustcode::controller::SettingsPicker::ScrollSpeed) {
            drop(s);
            let mut s = app_state.lock().await;
            if let Some(lines) = scroll_speed_picker_key(&mut s, key.code) {
                s.config.scroll_lines = Some(lines);
                rustcode::controller::save_config(&s.config);
                s.close_modal_status();
                s.set_transient_notice(rustcode::controller::scroll_speed_notice(lines));
            }
            *needs_redraw = true;
            return Ok(InputFlow::ContinueIteration);
        }

        if s.settings_picker == Some(rustcode::controller::SettingsPicker::Verbosity) {
            drop(s);
            match key.code {
                KeyCode::Up => {
                    let mut s = app_state.lock().await;
                    s.modal_picker_index = s.modal_picker_index.saturating_sub(1);
                }
                KeyCode::Down => {
                    let mut s = app_state.lock().await;
                    s.modal_picker_index = s
                        .modal_picker_index
                        .saturating_add(1)
                        .min(Verbosity::ALL.len() - 1);
                }
                KeyCode::Enter => {
                    let mut s = app_state.lock().await;
                    let new_verbosity = Verbosity::ALL
                        .get(s.modal_picker_index)
                        .cloned()
                        .unwrap_or_default();
                    s.verbosity = new_verbosity.clone();
                    s.config.verbosity = new_verbosity;
                    rustcode::controller::save_config(&s.config);
                    s.close_modal_status();
                }
                KeyCode::Esc => {
                    let mut s = app_state.lock().await;
                    s.close_modal_status();
                }
                _ => {}
            }
            return Ok(InputFlow::ContinueIteration);
        }

        if s.settings_picker == Some(rustcode::controller::SettingsPicker::Thinking) {
            drop(s);
            match key.code {
                KeyCode::Up => {
                    let mut s = app_state.lock().await;
                    s.modal_picker_index = s.modal_picker_index.saturating_sub(1);
                }
                KeyCode::Down => {
                    let mut s = app_state.lock().await;
                    s.modal_picker_index = s.modal_picker_index.saturating_add(1).min(2); // 0 on, 1 off, 2 default
                }
                KeyCode::Enter => {
                    let mut s = app_state.lock().await;
                    let value = match s.modal_picker_index {
                        0 => Some(true),
                        1 => Some(false),
                        _ => None,
                    };
                    if let Some(profile) = s.active_model_profile_mut() {
                        profile.enable_thinking = value;
                    }
                    rustcode::controller::save_config(&s.config);
                    s.close_modal_status();
                }
                KeyCode::Esc => {
                    let mut s = app_state.lock().await;
                    s.close_modal_status();
                }
                _ => {}
            }
            return Ok(InputFlow::ContinueIteration);
        }

        if s.settings_picker == Some(rustcode::controller::SettingsPicker::Effort) {
            drop(s);
            match key.code {
                KeyCode::Up => {
                    let mut s = app_state.lock().await;
                    s.modal_picker_index = s.modal_picker_index.saturating_sub(1);
                }
                KeyCode::Down => {
                    let mut s = app_state.lock().await;
                    let last =
                        rustcode::controller::effort_choices(s.active_model_profile().as_ref())
                            .len()
                            .saturating_sub(1);
                    s.modal_picker_index = s.modal_picker_index.saturating_add(1).min(last);
                }
                KeyCode::Enter => {
                    let mut s = app_state.lock().await;
                    let choices =
                        rustcode::controller::effort_choices(s.active_model_profile().as_ref());
                    let value = choices.get(s.modal_picker_index).cloned().flatten();
                    s.set_active_reasoning_effort(value);
                    s.close_modal_status();
                }
                KeyCode::Esc => {
                    let mut s = app_state.lock().await;
                    s.close_modal_status();
                }
                _ => {}
            }
            return Ok(InputFlow::ContinueIteration);
        }

        if s.settings_picker == Some(rustcode::controller::SettingsPicker::Protocol) {
            drop(s);
            match key.code {
                KeyCode::Up => {
                    let mut s = app_state.lock().await;
                    s.modal_picker_index = s.modal_picker_index.saturating_sub(1);
                }
                KeyCode::Down => {
                    let mut s = app_state.lock().await;
                    s.modal_picker_index = s.modal_picker_index.saturating_add(1).min(2); // 0 json, 1 native, 2 apinative
                }
                KeyCode::Enter => {
                    let mut s = app_state.lock().await;
                    let (protocol, label) = match s.modal_picker_index {
                        0 => (rustcode::config::ToolProtocol::Json, "JSON (```tool)"),
                        1 => (
                            rustcode::config::ToolProtocol::Native,
                            "Native ([TOOL_CALLS])",
                        ),
                        _ => (
                            rustcode::config::ToolProtocol::ApiNative,
                            "ApiNative (schema in request `tools`, structured `tool_calls` back)",
                        ),
                    };
                    let url = s.api_base_url.clone();
                    let scoped = s
                        .config
                        .models
                        .iter_mut()
                        .find(|profile| profile.url == url);
                    if let Some(profile) = scoped {
                        profile.tool_protocol = Some(protocol);
                    } else {
                        s.config.tool_protocol = protocol;
                    }
                    rustcode::controller::save_config(&s.config);
                    let active_model = s.model_name.clone();
                    s.set_transient_notice(format!(
                        "Switched tool protocol to {label} for model '{active_model}'."
                    ));
                    s.close_modal_status();
                }
                KeyCode::Esc => {
                    let mut s = app_state.lock().await;
                    s.close_modal_status();
                }
                _ => {}
            }
            return Ok(InputFlow::ContinueIteration);
        }

        if s.settings_picker == Some(rustcode::controller::SettingsPicker::Yolo) {
            drop(s);
            match key.code {
                KeyCode::Up => {
                    let mut s = app_state.lock().await;
                    s.modal_picker_index = s.modal_picker_index.saturating_sub(1);
                }
                KeyCode::Down => {
                    let mut s = app_state.lock().await;
                    s.modal_picker_index = s.modal_picker_index.saturating_add(1).min(1); // 0 on, 1 off
                }
                KeyCode::Enter => {
                    let mut s = app_state.lock().await;
                    let enable = s.modal_picker_index == 0;
                    s.set_auto_confirm(enable);
                    let status = if enable { "enabled" } else { "disabled" };
                    s.set_transient_notice(format!("YOLO mode {status}"));
                    s.close_modal_status();
                }
                KeyCode::Esc => {
                    let mut s = app_state.lock().await;
                    s.close_modal_status();
                }
                _ => {}
            }
            return Ok(InputFlow::ContinueIteration);
        }
    }

    let mut s = app_state.lock().await;
    if s.show_subagent_picker {
        let total = s.subagents.len() + 1;
        match subagent_picker_action(key.code) {
            Some(SubagentPickerAction::Close) => {
                s.show_subagent_picker = false;
            }
            Some(SubagentPickerAction::MoveUp) => {
                s.subagent_picker_index =
                    picker_selection_for_key(s.subagent_picker_index, total, KeyCode::Up)
                        .unwrap_or(s.subagent_picker_index);
            }
            Some(SubagentPickerAction::MoveDown) => {
                s.subagent_picker_index =
                    picker_selection_for_key(s.subagent_picker_index, total, KeyCode::Down)
                        .unwrap_or(s.subagent_picker_index);
            }
            Some(SubagentPickerAction::Select) => {
                let selected = s.subagent_picker_index.min(total.saturating_sub(1));
                let id = if selected == 0 {
                    0
                } else {
                    s.subagents[selected - 1].id
                };
                s.show_subagent_picker = false;
                drop(s);
                let _ = app_event_sender.send(AppEvent::SelectSubagent(id));
                return Ok(InputFlow::ContinueIteration);
            }
            None => {}
        }
        drop(s);
        return Ok(InputFlow::ContinueIteration);
    }

    if s.tasks_panel.is_some() {
        if let Some(input) = tasks_panel_input_for_key(key.code) {
            rustcode::controller::tasks_panel_input(&mut s, input);
        }
        return Ok(InputFlow::ContinueIteration);
    }
    if s.command_panel.is_some() {
        match key.code {
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') | KeyCode::Char('Q') => {
                s.command_panel = None;
            }
            KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown => {
                let delta = match key.code {
                    KeyCode::Up => -1,
                    KeyCode::Down => 1,
                    KeyCode::PageUp => -10,
                    _ => 10,
                };
                scroll_command_panel(
                    &mut s.modal_scroll_row,
                    transcript_state.panel_max_scroll_row,
                    delta,
                );
            }
            _ => {}
        }
        return Ok(InputFlow::ContinueIteration);
    }
    if s.show_context_modal {
        match key.code {
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') | KeyCode::Char('Q') => {
                s.show_context_modal = false;
            }
            _ => {}
        }
        drop(s);
        return Ok(InputFlow::ContinueIteration);
    }

    if s.show_status_modal {
        if matches!(
            key.code,
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') | KeyCode::Char('Q')
        ) {
            s.show_status_modal = false;
        }
        drop(s);
        return Ok(InputFlow::ContinueIteration);
    }

    if s.show_stats_modal {
        if matches!(
            key.code,
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') | KeyCode::Char('Q')
        ) {
            s.show_stats_modal = false;
        }
        drop(s);
        return Ok(InputFlow::ContinueIteration);
    }

    if s.show_session_modal {
        if matches!(
            key.code,
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') | KeyCode::Char('Q')
        ) {
            s.show_session_modal = false;
        }
        drop(s);
        return Ok(InputFlow::ContinueIteration);
    }

    if s.show_history_picker {
        // Ctrl+D triggers delete confirmation overlay
        if key.modifiers.contains(event::KeyModifiers::CONTROL) && key.code == KeyCode::Char('d') {
            let idx = s
                .history_picker_index
                .min(s.history_picker_sessions.len().saturating_sub(1));
            s.pending_delete_session_idx = Some(idx);
            drop(s);
            return Ok(InputFlow::ContinueIteration);
        }

        // Confirmation overlay for delete
        if let Some(del_idx) = s.pending_delete_session_idx {
            match key.code {
                KeyCode::Char('y') | KeyCode::Enter => {
                    let action = s
                        .history_picker_sessions
                        .get(del_idx)
                        .and_then(rustcode::app::session_controller::session_id_from_meta)
                        .map(rustcode::app::events::SessionAction::Id);
                    s.pending_delete_session_idx = None;
                    if let Some(action) = action {
                        let _ = app_event_sender.send(AppEvent::DeleteSession(action));
                    }
                }
                KeyCode::Esc | KeyCode::Char('n') => {
                    s.pending_delete_session_idx = None;
                }
                _ => {}
            }
            drop(s);
            return Ok(InputFlow::ContinueIteration);
        }

        match key.code {
            KeyCode::Esc => {
                s.show_history_picker = false;
            }
            KeyCode::Up => {
                let len = s.history_picker_sessions.len();
                s.history_picker_index =
                    picker_selection_for_key(s.history_picker_index, len, key.code)
                        .unwrap_or(s.history_picker_index);
            }
            KeyCode::Down => {
                let len = s.history_picker_sessions.len();
                s.history_picker_index =
                    picker_selection_for_key(s.history_picker_index, len, key.code)
                        .unwrap_or(s.history_picker_index);
            }
            KeyCode::Enter => {
                let idx = s
                    .history_picker_index
                    .min(s.history_picker_sessions.len().saturating_sub(1));
                if let Some(action) = s
                    .history_picker_sessions
                    .get(idx)
                    .and_then(rustcode::app::session_controller::session_id_from_meta)
                    .map(rustcode::app::events::SessionAction::Id)
                {
                    let _ = app_event_sender.send(AppEvent::ResumeSession(action));
                }
            }
            _ => {}
        }

        drop(s);
        return Ok(InputFlow::ContinueIteration);
    }

    if let Some(panel) = s.skills_panel.as_mut() {
        match key.code {
            KeyCode::Esc => s.skills_panel = None,
            KeyCode::Up | KeyCode::Down => {
                let next = picker_selection_for_key(panel.selected, panel.entries.len(), key.code)
                    .unwrap_or(panel.selected);
                panel.select(next);
                panel.notice = None;
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                // Keep the in-memory config in step so a later
                // full save cannot write the old list back.
                if let Some(disabled) = panel.toggle_selected() {
                    s.config.disabled_skills = disabled;
                }
            }
            _ => {}
        }
        drop(s);
        *needs_redraw = true;
        return Ok(InputFlow::ContinueIteration);
    }

    if s.show_mcp_config {
        let (existing_mcp_always_include, existing_mcp_client_id, existing_mcp_timeout) = s
            .mcp_edit_state
            .as_ref()
            .and_then(|edit_state| edit_state.edit_index)
            .and_then(|idx| s.config.mcp_servers.get(idx))
            .map(|server| {
                (
                    server.always_include,
                    server.client_id.clone(),
                    server.timeout_secs,
                )
            })
            .unwrap_or_default();
        if let Some(ref mut edit_state) = s.mcp_edit_state {
            if crate::ui::keymap::KeyMap::from_environment().resolve(key)
                == crate::ui::keymap::KeyAction::Paste
            {
                // Text only: an image marker has no place in a
                // server name or command.
                if let Some(text) = rustcode::clipboard::read_text_from_clipboard() {
                    edit_state.insert_str(&text);
                }
                drop(s);
                *needs_redraw = true;
                return Ok(InputFlow::ContinueIteration);
            }
            match key.code {
                KeyCode::Esc => {
                    s.mcp_edit_state = None;
                }
                KeyCode::Up => {
                    let prev = if edit_state.active_field == 0 {
                        2
                    } else {
                        edit_state.active_field - 1
                    };
                    edit_state.set_active_field(prev);
                }
                KeyCode::Down | KeyCode::Tab => {
                    let next = (edit_state.active_field + 1) % 3;
                    edit_state.set_active_field(next);
                }
                KeyCode::Enter => {
                    let name = edit_state.name_input.trim().to_string();
                    let command = edit_state.command_input.trim().to_string();
                    let args = edit_state
                        .args_input
                        .split_whitespace()
                        .map(|s| s.to_string())
                        .collect::<Vec<_>>();

                    // An http(s) value in the command field declares a
                    // remote Streamable HTTP server; anything else is
                    // spawned over stdio.
                    let remote = command.starts_with("http://") || command.starts_with("https://");
                    if !name.is_empty() && !command.is_empty() {
                        let url = remote.then(|| command.clone());
                        let new_srv = rustcode::config::McpServerConfig {
                            name: name.clone(),
                            command: if remote { String::new() } else { command },
                            // Arguments only apply to stdio servers.
                            args: if remote { Vec::new() } else { args },
                            env: std::collections::HashMap::new(),
                            url,
                            headers: std::collections::HashMap::new(),
                            client_id: existing_mcp_client_id,
                            enabled: true,
                            always_include: existing_mcp_always_include,
                            timeout_secs: existing_mcp_timeout,
                        };

                        if edit_state.is_add {
                            s.config.mcp_servers.push(new_srv);
                        } else if let Some(idx) = edit_state.edit_index
                            && idx < s.config.mcp_servers.len()
                        {
                            let old_name = s.config.mcp_servers[idx].name.clone();
                            s.config.mcp_servers[idx] = new_srv;
                            if old_name != name {
                                rustcode::mcp::shutdown_server(&old_name).await;
                            }
                        }

                        rustcode::controller::save_config(&s.config);

                        let name_clone = name.clone();
                        tokio::spawn(async move {
                            let _ = rustcode::mcp::start_server_by_name(&name_clone).await;
                        });

                        s.mcp_edit_state = None;
                    }
                }
                // Everything else edits the active field with
                // the keys every other text field uses.
                _ => {
                    let action = crate::ui::keymap::KeyMap::from_environment().resolve(key);
                    if let Some(edit) = action.text_edit(&mut [0; 4]) {
                        edit_state.edit(edit);
                    }
                }
            }
        } else {
            match key.code {
                KeyCode::Esc => {
                    s.show_mcp_config = false;
                }
                KeyCode::Up => {
                    let len = s.config.mcp_servers.len();
                    s.mcp_picker_index =
                        picker_selection_for_key(s.mcp_picker_index, len, key.code)
                            .unwrap_or(s.mcp_picker_index);
                }
                KeyCode::Down => {
                    let len = s.config.mcp_servers.len();
                    s.mcp_picker_index =
                        picker_selection_for_key(s.mcp_picker_index, len, key.code)
                            .unwrap_or(s.mcp_picker_index);
                }
                KeyCode::Char('a') | KeyCode::Char('A') => {
                    s.mcp_edit_state = Some(rustcode::app::McpEditState {
                        is_add: true,
                        edit_index: None,
                        name_input: String::new(),
                        command_input: String::new(),
                        args_input: String::new(),
                        active_field: 0,
                        cursor_pos: 0,
                    });
                }
                KeyCode::Char('e') | KeyCode::Char('E') => {
                    let idx = s.mcp_picker_index;
                    if let Some(srv) = s.config.mcp_servers.get(idx) {
                        s.mcp_edit_state = Some(rustcode::app::McpEditState {
                            is_add: false,
                            edit_index: Some(idx),
                            name_input: srv.name.clone(),
                            command_input: if srv.is_remote() {
                                srv.url.clone().unwrap_or_default()
                            } else {
                                srv.command.clone()
                            },
                            args_input: srv.args.join(" "),
                            active_field: 0,
                            cursor_pos: srv.name.len(),
                        });
                    }
                }
                KeyCode::Char('d') | KeyCode::Char('D') => {
                    let idx = s.mcp_picker_index;
                    if idx < s.config.mcp_servers.len() {
                        let removed = s.config.mcp_servers.remove(idx);
                        rustcode::controller::save_config(&s.config);
                        let name_clone = removed.name.clone();
                        tokio::spawn(async move {
                            rustcode::mcp::shutdown_server(&name_clone).await;
                        });
                        if s.mcp_picker_index >= s.config.mcp_servers.len()
                            && s.mcp_picker_index > 0
                        {
                            s.mcp_picker_index -= 1;
                        }
                    }
                }
                KeyCode::Enter => {
                    let idx = s.mcp_picker_index;
                    if let Some(srv) = s.config.mcp_servers.get_mut(idx) {
                        srv.enabled = !srv.enabled;
                        let name_clone = srv.name.clone();
                        let enabled = srv.enabled;
                        rustcode::controller::save_config(&s.config);
                        tokio::spawn(async move {
                            rustcode::controller::apply_mcp_toggle(&name_clone, enabled).await;
                        });
                    }
                }
                _ => {}
            }
        }
        drop(s);
        return Ok(InputFlow::ContinueIteration);
    }

    if s.show_model_picker {
        if crate::ui::keymap::KeyMap::from_environment().resolve(key)
            == crate::ui::keymap::KeyAction::Paste
        {
            if let Some(payload) = clipboard_paste_payload() {
                insert_picker_search_text(&mut s, PickerSearchTarget::Model, &payload);
            }
            drop(s);
            return Ok(InputFlow::ContinueIteration);
        }
        match key.code {
            KeyCode::Esc => {
                s.show_model_picker = false;
            }
            KeyCode::Up => {
                let len = rustcode::app::get_picker_items_count(&s);
                s.model_picker_index =
                    picker_selection_for_key(s.model_picker_index, len, key.code)
                        .unwrap_or(s.model_picker_index);
            }
            KeyCode::Down => {
                let len = rustcode::app::get_picker_items_count(&s);
                s.model_picker_index =
                    picker_selection_for_key(s.model_picker_index, len, key.code)
                        .unwrap_or(s.model_picker_index);
            }
            KeyCode::Enter => {
                rustcode::app::select_picker_model(&mut s);
                s.show_model_picker = false;
                rustcode::app::spawn_context_window_detection(
                    Arc::clone(&app_state),
                    client.clone(),
                );
            }
            _ => {
                handle_picker_search_key(&mut s, PickerSearchTarget::Model, key);
            }
        }
        drop(s);
        return Ok(InputFlow::ContinueIteration);
    }

    if s.show_theme_picker {
        let themes = crate::ui::theme::load_available_themes();
        let len = themes.len();
        match key.code {
            KeyCode::Esc => {
                s.config.theme = s.theme_picker_initial.clone();
                s.show_theme_picker = false;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                s.theme_picker_index = move_picker_selection(s.theme_picker_index, len, false);
                if len > 0 {
                    s.config.theme = themes[s.theme_picker_index].name.clone();
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                s.theme_picker_index = move_picker_selection(s.theme_picker_index, len, true);
                if len > 0 {
                    s.config.theme = themes[s.theme_picker_index].name.clone();
                }
            }
            KeyCode::Enter => {
                let selected = themes[s.theme_picker_index.min(len.saturating_sub(1))]
                    .name
                    .clone();
                s.config.theme = selected.clone();
                s.show_theme_picker = false;
                rustcode::controller::save_config(&s.config);
                s.set_transient_notice(format!("Theme set to '{}'", selected));
            }
            _ => {}
        }
        drop(s);
        return Ok(InputFlow::ContinueIteration);
    }

    if s.show_command_picker {
        let filtered_items = filtered_command_picker_items(&s.command_picker_search);

        if crate::ui::keymap::KeyMap::from_environment().resolve(key)
            == crate::ui::keymap::KeyAction::Paste
        {
            if let Some(payload) = clipboard_paste_payload() {
                insert_picker_search_text(&mut s, PickerSearchTarget::Command, &payload);
            }
            drop(s);
            return Ok(InputFlow::ContinueIteration);
        }

        let mut exit_flag = false;
        match key.code {
            KeyCode::Esc => {
                s.show_command_picker = false;
            }
            KeyCode::Up => {
                let len = filtered_items.len();
                s.command_picker_index =
                    picker_selection_for_key(s.command_picker_index, len, key.code)
                        .unwrap_or(s.command_picker_index);
            }
            KeyCode::Down => {
                let len = filtered_items.len();
                s.command_picker_index =
                    picker_selection_for_key(s.command_picker_index, len, key.code)
                        .unwrap_or(s.command_picker_index);
            }
            KeyCode::Enter => {
                let idx = s
                    .command_picker_index
                    .min(filtered_items.len().saturating_sub(1));
                if !filtered_items.is_empty() {
                    let item = filtered_items[idx];
                    s.show_command_picker = false;
                    if item.shortcut == "ctrl+c" {
                        exit_flag = true;
                    } else {
                        // Palette commands share slash dispatch, including panel
                        // presentation, arguments and immediate actions.
                        s.input_buffer = item.shortcut.to_owned();
                        s.cursor_position = s.input_buffer.len();
                        drop(s);
                        if open_demo_if_requested(app_state, demo_state).await {
                            *needs_redraw = true;
                            return Ok(InputFlow::ContinueIteration);
                        }
                        let should_exit = rustcode::app::handle_enter_with_ui_events(
                            app_state,
                            client,
                            current_cancel_token,
                            agent_ui_event_sender.clone(),
                            &|| {
                                crate::ui::theme::load_available_themes()
                                    .into_iter()
                                    .map(|theme| theme.name)
                                    .collect()
                            },
                        )
                        .await;
                        *needs_redraw = true;
                        return Ok(if should_exit {
                            InputFlow::Exit { update: false }
                        } else {
                            InputFlow::ContinueIteration
                        });
                    }
                } else {
                    s.show_command_picker = false;
                }
            }
            _ => {
                handle_picker_search_key(&mut s, PickerSearchTarget::Command, key);
            }
        }
        drop(s);
        if exit_flag {
            return Ok(InputFlow::Exit { update: false });
        }
        return Ok(InputFlow::ContinueIteration);
    }
    drop(s);
    if transcript_navigation {
        let page = terminal_runtime.terminal().area().height.saturating_sub(4) as usize;
        // Keep every intermediate row visible while a mouse range is growing.
        let page = if transcript_state.selection.is_dragging() {
            1
        } else {
            page.max(1)
        };
        if transcript_state.selection.is_active() {
            let direction = if matches!(key.code, KeyCode::PageUp | KeyCode::Up) {
                -1
            } else {
                1
            };
            transcript_state.selection.queue_scroll(direction, page);
            frame_requester.schedule_frame();
        } else if matches!(key.code, KeyCode::PageUp | KeyCode::Up) {
            transcript_state.scroll_up(page);
        } else {
            transcript_state.scroll_down(page);
        }
        return Ok(InputFlow::ContinueIteration);
    }
    // Escape closes transcript browsing without discarding a draft.
    if return_to_latest_for_key(transcript_state, key.code) {
        return Ok(InputFlow::ContinueIteration);
    }
    if key.code == KeyCode::Enter
        && !key.modifiers.intersects(
            event::KeyModifiers::SHIFT | event::KeyModifiers::CONTROL | event::KeyModifiers::ALT,
        )
        && open_demo_if_requested(app_state, demo_state).await
    {
        return Ok(InputFlow::ContinueIteration);
    }
    match {
        let mut state = app_state.lock().await;
        composer.handle_key(&mut state, key)
    } {
        ui::ComposerAction::Handled => {
            *needs_redraw = true;
            return Ok(InputFlow::ContinueIteration);
        }
        ui::ComposerAction::Submit => {
            if rustcode::app::handle_enter_with_ui_events(
                &app_state,
                &client,
                current_cancel_token,
                agent_ui_event_sender.clone(),
                &|| {
                    crate::ui::theme::load_available_themes()
                        .into_iter()
                        .map(|t| t.name)
                        .collect()
                },
            )
            .await
            {
                return Ok(InputFlow::Exit { update: false });
            }
            *needs_redraw = true;
            return Ok(InputFlow::ContinueIteration);
        }
        ui::ComposerAction::ClearScreen => {
            terminal_runtime.terminal().clear()?;
            return Ok(InputFlow::ContinueIteration);
        }
        ui::ComposerAction::Paste => {
            if let Some(payload) = clipboard_paste_payload() {
                let mut state = app_state.lock().await;
                insert_clipboard_paste(composer, &mut state, &payload);
            }
            *needs_redraw = true;
            return Ok(InputFlow::ContinueIteration);
        }
        ui::ComposerAction::BackgroundCommand => {
            let mut state = app_state.lock().await;
            let moved =
                rustcode::tools::move_foreground_commands_to_background(&state.active_session_id);
            state.set_transient_notice(match moved {
                0 => "No running command to move to the background",
                1 => "Moved the running command to the background",
                _ => "Moved the running commands to the background",
            });
            *needs_redraw = true;
            return Ok(InputFlow::ContinueIteration);
        }
        action @ (ui::ComposerAction::ToggleExpandAll | ui::ComposerAction::ToggleExpandStep) => {
            let step = matches!(action, ui::ComposerAction::ToggleExpandStep);
            let mut state = app_state.lock().await;
            let width = terminal_runtime.terminal().area().width;
            toggle_tool_expansion(&mut state, width, step);
            *needs_redraw = true;
            return Ok(InputFlow::ContinueIteration);
        }
        ui::ComposerAction::ToggleExpandEdits => {
            let mut state = app_state.lock().await;
            let width = terminal_runtime.terminal().area().width;
            toggle_edit_expansion(&mut state, width);
            *needs_redraw = true;
            return Ok(InputFlow::ContinueIteration);
        }
        ui::ComposerAction::Unhandled => {}
    }

    match key.code {
        KeyCode::Esc => {
            let mut s = app_state.lock().await;
            if s.dismiss_completion() {
                // Popup dismissal keeps the draft intact. Typing or moving
                // to another token makes completion eligible again.
            } else if s.sel_start.is_some() || s.sel_end.is_some() {
                s.clear_selection();
            } else if !s.input_buffer.is_empty()
                && matches!(s.status, AppStatus::Idle)
                && s.running_tools.is_empty()
            {
                s.input_buffer.clear();
                s.cursor_position = 0;
            } else {
                drop(s);
                rustcode::app::handle_escape(&app_state, current_cancel_token).await;
            }
            *needs_redraw = true;
        }
        KeyCode::Up => {
            let mut s = app_state.lock().await;
            let completion_len = s.completion_len();
            if s.active_suggestion_index.is_some() && completion_len > 0 {
                let current = s.active_suggestion_index.unwrap_or(0);
                s.active_suggestion_index = Some(if current == 0 {
                    completion_len - 1
                } else {
                    current - 1
                });
            } else {
                s.active_suggestion_index = None;
                if s.input_buffer.is_empty() || s.history_index.is_some() {
                    // With an empty buffer, Up first pulls the most
                    // recent queued prompt back for editing; only
                    // when nothing is queued does it recall history.
                    // Once recall has started, keep walking it —
                    // without this, the recalled text made the buffer
                    // non-empty and the next Up fell through to
                    // cursor movement, pinning recall on the most
                    // recent entry.
                    let pulled = s.history_index.is_none() && s.pop_queued_prompt();
                    if !pulled {
                        s.history_up();
                    }
                } else {
                    s.move_cursor_line_up();
                }
            }
        }
        KeyCode::Down => {
            let mut s = app_state.lock().await;
            let completion_len = s.completion_len();
            if s.active_suggestion_index.is_some() && completion_len > 0 {
                let current = s.active_suggestion_index.unwrap_or(0);
                s.active_suggestion_index = Some(if current + 1 >= completion_len {
                    0
                } else {
                    current + 1
                });
            } else {
                s.active_suggestion_index = None;
                if s.history_index.is_some() {
                    s.history_down();
                } else {
                    s.move_cursor_line_down();
                }
            }
        }
        KeyCode::Left => {
            let mut s = app_state.lock().await;
            // An empty composer has no cursor to move, so Left opens
            // the agent contexts the way Codex opens its agents overview.
            if key.modifiers.is_empty() && s.input_buffer.is_empty() && !s.subagents.is_empty() {
                open_overlay(&mut s, rustcode::app::events::Overlay::Subagents);
                return Ok(InputFlow::ContinueIteration);
            }
            let shift = key.modifiers.contains(event::KeyModifiers::SHIFT);
            let alt = key.modifiers.contains(event::KeyModifiers::ALT)
                || key.modifiers.contains(event::KeyModifiers::META);
            // Word motion wins whenever there is a draft; only an
            // empty composer gives Alt+arrows to agent switching.
            if alt
                && !shift
                && s.input_buffer.is_empty()
                && let Some(id) = sessions::adjacent_context_id(&s, false)
            {
                drop(s);
                let _ = app_event_sender.send(AppEvent::SelectSubagent(id));
                return Ok(InputFlow::ContinueIteration);
            }
            if shift && s.composer_selection_anchor.is_none() {
                s.composer_selection_anchor = Some(s.cursor_position);
            }
            if alt {
                s.move_cursor_word_left();
            } else {
                s.move_cursor_left();
            }
            if !shift {
                s.composer_selection_anchor = None;
            } else if s.composer_selection_anchor == Some(s.cursor_position) {
                s.composer_selection_anchor = None;
            }
        }
        KeyCode::Right => {
            let mut s = app_state.lock().await;
            if key.modifiers.is_empty()
                && let Some(id) = agent_context_return_target(&s, key.code)
            {
                drop(s);
                let _ = app_event_sender.send(AppEvent::SelectSubagent(id));
                return Ok(InputFlow::ContinueIteration);
            }
            let shift = key.modifiers.contains(event::KeyModifiers::SHIFT);
            let alt = key.modifiers.contains(event::KeyModifiers::ALT)
                || key.modifiers.contains(event::KeyModifiers::META);
            // Word motion wins whenever there is a draft; only an
            // empty composer gives Alt+arrows to agent switching.
            if alt
                && !shift
                && s.input_buffer.is_empty()
                && let Some(id) = sessions::adjacent_context_id(&s, true)
            {
                drop(s);
                let _ = app_event_sender.send(AppEvent::SelectSubagent(id));
                return Ok(InputFlow::ContinueIteration);
            }
            if shift && s.composer_selection_anchor.is_none() {
                s.composer_selection_anchor = Some(s.cursor_position);
            }
            if alt {
                s.move_cursor_word_right();
            } else {
                s.move_cursor_right();
            }
            if !shift {
                s.composer_selection_anchor = None;
            } else if s.composer_selection_anchor == Some(s.cursor_position) {
                s.composer_selection_anchor = None;
            }
        }
        KeyCode::Home => {
            let mut s = app_state.lock().await;
            let shift = key.modifiers.contains(event::KeyModifiers::SHIFT);
            if shift && s.composer_selection_anchor.is_none() {
                s.composer_selection_anchor = Some(s.cursor_position);
            }
            s.move_cursor_to_start();
            if !shift {
                s.composer_selection_anchor = None;
            } else if s.composer_selection_anchor == Some(s.cursor_position) {
                s.composer_selection_anchor = None;
            }
        }
        KeyCode::End => {
            let mut s = app_state.lock().await;
            let shift = key.modifiers.contains(event::KeyModifiers::SHIFT);
            if shift && s.composer_selection_anchor.is_none() {
                s.composer_selection_anchor = Some(s.cursor_position);
            }
            s.move_cursor_to_end();
            if !shift {
                s.composer_selection_anchor = None;
            } else if s.composer_selection_anchor == Some(s.cursor_position) {
                s.composer_selection_anchor = None;
            }
        }
        KeyCode::Char('l') if key.modifiers.contains(event::KeyModifiers::CONTROL) => {
            terminal_runtime.terminal().clear()?;
        }
        KeyCode::Enter => {
            let modifiers = key.modifiers;
            if modifiers.contains(event::KeyModifiers::SHIFT)
                || modifiers.contains(event::KeyModifiers::CONTROL)
                || modifiers.contains(event::KeyModifiers::ALT)
            {
                let mut s = app_state.lock().await;
                s.insert_char('\n');
                s.reset_suggestion_cycle();
            } else {
                if open_demo_if_requested(app_state, demo_state).await {
                    return Ok(InputFlow::ContinueIteration);
                }
                if rustcode::app::handle_enter_with_ui_events(
                    &app_state,
                    &client,
                    current_cancel_token,
                    agent_ui_event_sender.clone(),
                    &|| {
                        crate::ui::theme::load_available_themes()
                            .into_iter()
                            .map(|t| t.name)
                            .collect()
                    },
                )
                .await
                {
                    return Ok(InputFlow::Exit { update: false });
                }
            }
        }
        KeyCode::Char('v') | KeyCode::Char('V')
            if key.modifiers.contains(event::KeyModifiers::CONTROL)
                || key.modifiers.contains(event::KeyModifiers::SUPER)
                || key.modifiers.contains(event::KeyModifiers::META) =>
        {
            // Terminals without bracketed paste deliver Ctrl/Cmd+V
            // as a key. Route it through the same helper the keymap
            // paste uses so the large-paste marker cannot drift
            // between the two (#1527).
            if let Some(payload) = clipboard_paste_payload() {
                let mut s = app_state.lock().await;
                insert_clipboard_paste(composer, &mut s, &payload);
            }
        }
        KeyCode::Char('p') | KeyCode::Char('n')
            if key.modifiers.contains(event::KeyModifiers::CONTROL) =>
        {
            let mut s = app_state.lock().await;
            let completion_len = s.completion_len();
            if s.active_suggestion_index.is_some() && completion_len > 0 {
                let current = s.active_suggestion_index.unwrap_or(0);
                s.active_suggestion_index = Some(if key.code == KeyCode::Char('p') {
                    if current == 0 {
                        completion_len - 1
                    } else {
                        current - 1
                    }
                } else if current + 1 >= completion_len {
                    0
                } else {
                    current + 1
                });
            } else if key.code == KeyCode::Char('p') {
                s.show_command_picker = true;
                s.command_picker_index = 0;
                s.command_picker_search.clear();
            }
        }

        KeyCode::Char(c) => {
            let mut s = app_state.lock().await;
            let ctrl = key.modifiers.contains(event::KeyModifiers::CONTROL);
            let alt = key.modifiers.contains(event::KeyModifiers::ALT)
                || key.modifiers.contains(event::KeyModifiers::META);
            let cmd = key.modifiers.contains(event::KeyModifiers::SUPER);

            if c == '\x7f' || c == '\x08' || c == '\x17' {
                // Option+Backspace, Ctrl+W, or raw DEL on Mac
                if alt || cmd || c == '\x17' {
                    s.delete_word_backspace();
                } else {
                    s.delete_char_backspace();
                }
                s.reset_suggestion_cycle();
            } else if cmd {
                if c == 'u' {
                    s.kill_line_to_start();
                    s.reset_suggestion_cycle();
                }
            } else if (alt && c == 'b') || c == '∫' {
                // Terminals without enhanced key reporting send
                // Option+Left/Right as these word-motion keys.
                if s.input_buffer.is_empty()
                    && let Some(id) = sessions::adjacent_context_id(&s, false)
                {
                    drop(s);
                    let _ = app_event_sender.send(AppEvent::SelectSubagent(id));
                    return Ok(InputFlow::ContinueIteration);
                }
                s.move_cursor_word_left();
            } else if (alt && c == 'f') || c == 'ƒ' {
                if s.input_buffer.is_empty()
                    && let Some(id) = sessions::adjacent_context_id(&s, true)
                {
                    drop(s);
                    let _ = app_event_sender.send(AppEvent::SelectSubagent(id));
                    return Ok(InputFlow::ContinueIteration);
                }
                s.move_cursor_word_right();
            } else if (alt && c == 'd') || c == '∂' {
                s.delete_word_forward();
                s.reset_suggestion_cycle();
            } else if ctrl && c == 'j' {
                s.insert_char('\n');
                s.reset_suggestion_cycle();
            } else if ctrl && c == 'a' {
                s.move_cursor_to_start();
            } else if ctrl && c == 'e' {
                s.move_cursor_to_end();
            } else if ctrl && c == 'u' {
                s.kill_line_to_start();
                s.reset_suggestion_cycle();
            } else if ctrl && c == 'w' {
                s.delete_word_backspace();
                s.reset_suggestion_cycle();
            } else if c == '?' && !ctrl && !alt && !cmd && s.input_buffer.is_empty() {
                s.show_command_panel("Help", rustcode::app::build_help_text());
                s.request_redraw();
            } else if !ctrl && !alt && !c.is_control() {
                s.insert_char(c);
                s.reset_suggestion_cycle();
            }
        }
        KeyCode::Backspace => {
            let mut s = app_state.lock().await;
            let alt = key.modifiers.contains(event::KeyModifiers::ALT)
                || key.modifiers.contains(event::KeyModifiers::META);
            let ctrl = key.modifiers.contains(event::KeyModifiers::CONTROL);
            let cmd = key.modifiers.contains(event::KeyModifiers::SUPER);
            if cmd {
                s.kill_line_to_start();
            } else if alt || ctrl {
                s.delete_word_backspace();
            } else {
                s.delete_char_backspace();
            }
            s.reset_suggestion_cycle();
        }
        KeyCode::Delete => {
            let mut s = app_state.lock().await;
            let alt = key.modifiers.contains(event::KeyModifiers::ALT)
                || key.modifiers.contains(event::KeyModifiers::META);
            let cmd = key.modifiers.contains(event::KeyModifiers::SUPER);
            if cmd {
                s.kill_line_to_start();
            } else if alt {
                s.delete_word_forward();
            } else {
                s.delete_char_delete();
            }
            s.reset_suggestion_cycle();
        }
        _ => {}
    }
    Ok(InputFlow::ContinueLoop)
}
